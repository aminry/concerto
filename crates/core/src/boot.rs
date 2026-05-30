//! Reusable Core boot orchestration.
//!
//! Hosts everything `main.rs::run()` used to do up to "concerto-core
//! ready": resolve config, start the [`Runtime`], spawn every
//! supervised actor + the gRPC server. Returns a [`RunningCore`] the
//! caller drives to completion. Both the daemon binary and the
//! embedded desktop path call [`start`].

use std::path::PathBuf;
use std::sync::Arc;

#[cfg(unix)]
use crate::agent_supervisor::{AgentSupervisorActor, AgentSupervisorConfig};
use crate::api_server::{ApiServerActor, ApiServerConfig};
use crate::audit::{AuditWriterTask, JsonlFileSubscriber};
use crate::repo_manager::{RepoManagerActor, RepoManagerConfig};
use crate::runtime::{Runtime, RuntimeConfig, StartOutcome};
#[cfg(unix)]
use crate::scheduler::{SchedulerActor, SchedulerConfig};
use crate::skills::{SkillsRegistryActor, SkillsRegistryConfig};
#[cfg(unix)]
use crate::suggestions::{SuggestionEngineActor, SuggestionEngineConfig};
use crate::vcs::{VcsConfig, VcsProviderActor};
use crate::workspace_manager::{
    WorkareaManagerActor, WorkareaManagerConfig, WorkspaceManagerActor, WorkspaceManagerConfig,
};
use concerto_error::Result;

/// Outcome of [`start`]. Mirrors [`StartOutcome`] so callers can react
/// to the single-instance guard (the embedded desktop path falls back
/// to dialing the live daemon on `AlreadyRunning`).
///
/// `Started` is the dominant variant by design — constructed at most
/// once per process and consumed shortly thereafter, mirroring
/// [`StartOutcome`]; boxing it would force every caller through a
/// redundant pointer dereference.
#[allow(clippy::large_enum_variant)]
pub enum BootOutcome {
    Started(RunningCore),
    AlreadyRunning { pid: u32 },
}

/// A booted, ready Core. Hold it to keep Core alive; call
/// [`RunningCore::run_until_shutdown`] to block until a shutdown signal
/// (or a cancelled [`RunningCore::shutdown_token`]) then tear down.
pub struct RunningCore {
    runtime: Runtime,
    socket_path: PathBuf,
}

impl RunningCore {
    /// The UDS path the gRPC server bound. Clients dial this.
    pub fn socket_path(&self) -> &std::path::Path {
        &self.socket_path
    }

    /// A clone of the runtime's shutdown token. Cancel it to trigger an
    /// orderly shutdown from another thread (e.g. a window-close handler).
    pub fn shutdown_token(&self) -> tokio_util::sync::CancellationToken {
        self.runtime.shutdown_token()
    }

    /// Block until shutdown is signalled, then stop the runtime
    /// (releases the PID lock, flushes audit, stops agents).
    pub async fn run_until_shutdown(self) -> Result<()> {
        self.runtime.wait_for_shutdown().await?;
        tracing::info!("shutdown signal observed");
        self.runtime.stop().await?;
        tracing::info!("concerto-core stopped");
        Ok(())
    }
}

/// Boot Core: resolve config, start the runtime, and spawn every
/// supervised actor including the gRPC server. Returns once all actors
/// are spawned; the gRPC server binds its UDS asynchronously inside its
/// own actor shortly after this returns, so the socket is not guaranteed
/// dialable the instant `start` resolves. Errors propagate;
/// `AlreadyRunning` is a non-error outcome.
pub async fn start(config: RuntimeConfig) -> Result<BootOutcome> {
    tracing::info!("concerto-core starting");

    tracing::info!(
        data_dir = %config.data_dir.display(),
        config_dir = %config.config_dir.display(),
        "resolved runtime config"
    );

    let socket_path = config.config_dir.join("core.sock");
    let repos_root = config.data_dir.join("repos");
    let data_dir = Arc::new(config.data_dir.clone());
    let config_dir = Arc::new(config.config_dir.clone());
    let mut runtime = match Runtime::start(config).await? {
        StartOutcome::Started(r) => r,
        StartOutcome::AlreadyRunning { pid } => {
            tracing::info!(other_pid = pid, "another instance is live");
            return Ok(BootOutcome::AlreadyRunning { pid });
        }
    };

    // Task 18: spawn the Repository Manager first so its handle can be
    // captured by the gRPC server's factory closure below. The actor's
    // `run` loop just idles on shutdown; the handle is the meaningful
    // surface and lives in `RepoManagerActor::new`.
    let persistence = runtime
        .supervisor()
        .expect("supervisor present at boot")
        .persistence();
    let repo_actor = RepoManagerActor::new(Arc::clone(&persistence), repos_root.clone());
    let repo_handle = repo_actor.handle();
    // The actor instance built above is consumed by the factory; the
    // handle clone above is what the gRPC service holds.
    drop(repo_actor);
    let repo_factory_persistence = Arc::clone(&persistence);
    let repo_factory_root = repos_root.clone();
    runtime
        .supervisor_mut()
        .expect("supervisor present at boot")
        .spawn::<RepoManagerActor, _>(
            move || {
                RepoManagerActor::new(
                    Arc::clone(&repo_factory_persistence),
                    repo_factory_root.clone(),
                )
            },
            RepoManagerConfig {
                repos_root: repos_root.clone(),
            },
        )
        .await?;

    // Task 44: spawn the AuditWriter task BEFORE the managers, so the
    // managers can hold a clone of the writer handle. The
    // JsonlFileSubscriber writes to `<data_dir>/audit/audit-<day>.jsonl`
    // with daily UTC rotation; the writer task fans out events to every
    // subscriber and gates shutdown on a final flush.
    let audit_dir = data_dir.join("audit");
    let jsonl_subscriber: Arc<dyn crate::audit::AuditLogSubscriber> =
        Arc::new(JsonlFileSubscriber::new(audit_dir.clone()));
    let (audit_writer, _audit_drained, _audit_join) =
        AuditWriterTask::spawn(vec![jsonl_subscriber], runtime.shutdown_token());
    tracing::info!(
        audit_dir = %audit_dir.display(),
        "audit writer ready"
    );

    // Task 19: spawn the Workspace Manager. Same pattern as the repo
    // manager — the actor's `run` parks on shutdown; the cheap-to-clone
    // handle is what the gRPC `Workspaces` service holds.
    let workspace_actor =
        WorkspaceManagerActor::new(Arc::clone(&persistence), Arc::clone(&config_dir));
    let workspace_handle = workspace_actor.handle().with_audit(audit_writer.clone());
    drop(workspace_actor);
    let workspace_factory_persistence = Arc::clone(&persistence);
    let workspace_factory_config_dir = Arc::clone(&config_dir);
    runtime
        .supervisor_mut()
        .expect("supervisor present at boot")
        .spawn::<WorkspaceManagerActor, _>(
            move || {
                WorkspaceManagerActor::new(
                    Arc::clone(&workspace_factory_persistence),
                    Arc::clone(&workspace_factory_config_dir),
                )
            },
            WorkspaceManagerConfig,
        )
        .await?;

    // Task 20: spawn the Workarea Manager. The handle owns workarea
    // creation (composer-name allocation, worktree setup, `.context/`
    // skeleton) and emits `workarea.events` on its broadcast channel.
    let workarea_actor = WorkareaManagerActor::new(
        Arc::clone(&persistence),
        repo_handle.clone(),
        Arc::clone(&data_dir),
        Arc::clone(&config_dir),
    );
    let workarea_handle = workarea_actor.handle();
    drop(workarea_actor);
    let workarea_factory_persistence = Arc::clone(&persistence);
    let workarea_factory_repo = repo_handle.clone();
    let workarea_factory_data_dir = Arc::clone(&data_dir);
    let workarea_factory_config_dir = Arc::clone(&config_dir);
    runtime
        .supervisor_mut()
        .expect("supervisor present at boot")
        .spawn::<WorkareaManagerActor, _>(
            move || {
                WorkareaManagerActor::new(
                    Arc::clone(&workarea_factory_persistence),
                    workarea_factory_repo.clone(),
                    Arc::clone(&workarea_factory_data_dir),
                    Arc::clone(&workarea_factory_config_dir),
                )
            },
            WorkareaManagerConfig,
        )
        .await?;

    // Task 22: spawn the Agent Supervisor. The handle owns session
    // creation (cookie + UDS + agent-host spawn + Hello/Ready handshake)
    // and emits `AgentEvent`s on per-session broadcast channels.
    #[cfg(unix)]
    let agent_supervisor_handle = {
        let host_bin = crate::agent_supervisor::spawn::default_host_binary()?;
        let actor = AgentSupervisorActor::new(
            Arc::clone(&persistence),
            Arc::clone(&data_dir),
            Arc::clone(&config_dir),
            host_bin.clone(),
        );
        let handle = actor.handle();
        drop(actor);
        let factory_persistence = Arc::clone(&persistence);
        let factory_data_dir = Arc::clone(&data_dir);
        let factory_config_dir = Arc::clone(&config_dir);
        let factory_host_bin = host_bin.clone();
        runtime
            .supervisor_mut()
            .expect("supervisor present at boot")
            .spawn::<AgentSupervisorActor, _>(
                move || {
                    AgentSupervisorActor::new(
                        Arc::clone(&factory_persistence),
                        Arc::clone(&factory_data_dir),
                        Arc::clone(&factory_config_dir),
                        factory_host_bin.clone(),
                    )
                },
                AgentSupervisorConfig,
            )
            .await?;
        handle
    };

    // Task 31: wire the Agent Supervisor + Workarea Manager into the
    // workarea + workspace handles so archive cascades can stop live
    // sessions and the workspace-level cascade can drive workarea
    // side effects through the workarea manager.
    #[cfg(unix)]
    let workarea_handle = workarea_handle.with_agent_supervisor(agent_supervisor_handle.clone());
    let workspace_handle = workspace_handle.with_workarea_manager(workarea_handle.clone());

    // Task 38: spawn the Scheduler. Owns the `/loop` fire wheel and the
    // expiration sweep; takes a supervisor clone so the fire path can
    // call `start_session` directly. Runs after the Agent Supervisor
    // exists (`SchedulerActor::new` requires the handle).
    #[cfg(unix)]
    let scheduler_handle = {
        let scheduler_actor =
            SchedulerActor::new(Arc::clone(&persistence), agent_supervisor_handle.clone());
        let handle = scheduler_actor.handle();
        drop(scheduler_actor);
        let factory_persistence = Arc::clone(&persistence);
        let factory_supervisor = agent_supervisor_handle.clone();
        runtime
            .supervisor_mut()
            .expect("supervisor present at boot")
            .spawn::<SchedulerActor, _>(
                move || {
                    SchedulerActor::new(
                        Arc::clone(&factory_persistence),
                        factory_supervisor.clone(),
                    )
                },
                SchedulerConfig,
            )
            .await?;
        handle
    };

    // Task 39: spawn the Skills Registry. Holds an Arc<Persistence>
    // and the user's `~/` for the personal-scope walk; the actor's
    // `run` parks on shutdown. The handle exposes list / toggle /
    // refresh as the frozen V0.1 surface.
    let home_dir = home::home_dir()
        .ok_or_else(|| concerto_error::Error::Internal("home::home_dir() returned None".into()))?;
    let skills_actor = SkillsRegistryActor::new(Arc::clone(&persistence), home_dir.clone());
    let skills_handle = skills_actor.handle();
    drop(skills_actor);
    let skills_factory_persistence = Arc::clone(&persistence);
    let skills_factory_home = home_dir.clone();
    runtime
        .supervisor_mut()
        .expect("supervisor present at boot")
        .spawn::<SkillsRegistryActor, _>(
            move || {
                SkillsRegistryActor::new(
                    Arc::clone(&skills_factory_persistence),
                    skills_factory_home.clone(),
                )
            },
            SkillsRegistryConfig,
        )
        .await?;
    // Boot-time discovery so the index reflects what's on disk before
    // the gRPC server starts accepting traffic. Errors don't gate the
    // boot — the UI still works; the user just sees an empty list
    // until they request a refresh.
    match skills_handle.refresh(None).await {
        Ok(report) => tracing::info!(
            discovered = report.discovered_count,
            errors = report.errors.len(),
            "skills.boot_refresh complete"
        ),
        Err(e) => tracing::warn!(error = %e, "skills.boot_refresh failed"),
    }

    // Task 40: spawn the Suggestion Engine. Owns the V0.1 rule engine
    // — six built-in rules + per-workarea state + dedup. The actor's
    // `run` parks on shutdown; the cheap-to-clone handle is the
    // meaningful surface. The engine attaches to live sessions via a
    // background pump (1s tick) so newly-started sessions are picked
    // up without a back-channel from the supervisor.
    #[cfg(unix)]
    let suggestions_handle = {
        let actor = SuggestionEngineActor::new(Arc::clone(&persistence));
        let handle = actor.handle();
        drop(actor);
        let factory_persistence = Arc::clone(&persistence);
        runtime
            .supervisor_mut()
            .expect("supervisor present at boot")
            .spawn::<SuggestionEngineActor, _>(
                move || SuggestionEngineActor::new(Arc::clone(&factory_persistence)),
                SuggestionEngineConfig,
            )
            .await?;
        // Spawn the session-pump background task. Cancelled when the
        // root shutdown token fires.
        let shutdown_token = runtime.shutdown_token();
        handle.spawn_session_pump(agent_supervisor_handle.clone(), shutdown_token);
        handle
    };

    // Task 31: boot-time crash adoption (`design/03 §6.5`). Scan every
    // non-archived workarea, probe `worktree_root`, transition rows
    // whose directory is missing to `'crashed'`. The user — not
    // Concerto — decides whether to restart or archive a crashed row.
    match workarea_handle.adopt_crashed_workareas().await {
        Ok(0) => tracing::debug!("crash-adoption sweep: no workareas to adopt"),
        Ok(n) => tracing::info!(adopted = n, "crash-adoption sweep complete"),
        Err(e) => tracing::warn!(error = %e, "crash-adoption sweep failed"),
    }

    // Task 36: PTY hot-reconnect sweep (`design/04 §6.4`). Scan
    // `<data_dir>/runtime/agents/*.sock` and re-attach to every
    // `concerto-agent-host` that survived the previous Core's exit.
    // Runs AFTER the supervisor actor is spawned (so the handle's
    // `sessions_map` is wired) and BEFORE the gRPC server starts
    // accepting traffic (so a `Sessions.Get` for an adopted session
    // sees the re-registered in-memory entry, not a "not found" race).
    #[cfg(unix)]
    match crate::agent_supervisor::adopt_orphans(&agent_supervisor_handle).await {
        Ok(0) => tracing::debug!("pty hot-reconnect sweep: no surviving hosts"),
        Ok(n) => tracing::info!(adopted = n, "pty hot-reconnect sweep complete"),
        Err(e) => tracing::warn!(error = %e, "pty hot-reconnect sweep failed"),
    }

    // Task 45: spawn the VCS Provider. Same actor pattern as the
    // skills registry — the actor's `run` parks on shutdown; the
    // handle holds an `Arc<Persistence>` for the cached
    // `pull_requests` rows and lazily resolves the `gh` binary on
    // first use. The probe (`gh auth status`) runs at boot but does
    // NOT gate startup: a missing or unauthenticated `gh` produces
    // a warning, and the per-RPC error surfaces the same condition
    // to the caller.
    let vcs_actor = VcsProviderActor::new(Arc::clone(&persistence));
    let vcs_handle = vcs_actor.handle();
    drop(vcs_actor);
    let vcs_factory_persistence = Arc::clone(&persistence);
    runtime
        .supervisor_mut()
        .expect("supervisor present at boot")
        .spawn::<VcsProviderActor, _>(
            move || VcsProviderActor::new(Arc::clone(&vcs_factory_persistence)),
            VcsConfig,
        )
        .await?;
    match vcs_handle.check_auth().await {
        Ok(()) => tracing::info!("vcs.gh_auth ok"),
        Err(e) => {
            tracing::warn!(error = %e, "vcs.gh_auth probe failed (UI will prompt on first use)")
        }
    }

    // Task 13: spawn the gRPC server as the next supervised actor.
    // Handles captured by the factory closure are cheap `Arc::clone`s
    // (plus a single `RepoManager::clone` / `WorkspaceManager::clone`
    // for the optional services), so a restart constructs a fresh
    // `ApiServerActor` without re-reading the wall clock or rebuilding
    // the supervisor view.
    let started_at = runtime.started_at();
    let supervisor_view = runtime
        .supervisor()
        .expect("supervisor present at boot")
        .view();
    let factory_started_at = Arc::clone(&started_at);
    let factory_view = supervisor_view.clone();
    let factory_repo_handle = repo_handle.clone();
    let factory_workspace_handle = workspace_handle.clone();
    let factory_workarea_handle = workarea_handle.clone();
    #[cfg(unix)]
    let factory_agent_handle = agent_supervisor_handle.clone();
    let factory_persistence = Arc::clone(&persistence);
    #[cfg(unix)]
    let factory_scheduler_handle = scheduler_handle.clone();
    let factory_skills_handle = skills_handle.clone();
    #[cfg(unix)]
    let factory_suggestions_handle = suggestions_handle.clone();
    let factory_vcs_handle = vcs_handle.clone();
    runtime
        .supervisor_mut()
        .expect("supervisor present at boot")
        .spawn::<ApiServerActor, _>(
            move || {
                ApiServerActor::with_managers(
                    Arc::clone(&factory_started_at),
                    factory_view.clone(),
                    Some(factory_repo_handle.clone()),
                    Some(factory_workspace_handle.clone()),
                    Some(factory_workarea_handle.clone()),
                    #[cfg(unix)]
                    Some(factory_agent_handle.clone()),
                    Some(Arc::clone(&factory_persistence)),
                    #[cfg(unix)]
                    Some(factory_scheduler_handle.clone()),
                    Some(factory_skills_handle.clone()),
                    #[cfg(unix)]
                    Some(factory_suggestions_handle.clone()),
                    Some(factory_vcs_handle.clone()),
                )
            },
            ApiServerConfig {
                socket_path: socket_path.clone(),
            },
        )
        .await?;

    tracing::info!("concerto-core ready");

    Ok(BootOutcome::Started(RunningCore {
        runtime,
        socket_path,
    }))
}
