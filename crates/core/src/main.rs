//! `concerto-core` binary entry point.
//!
//! As of Task 11 this binary:
//!   1. Initializes logging (Task 05).
//!   2. Resolves a [`RuntimeConfig`] (data_dir + config_dir, env-overridable).
//!   3. Calls [`Runtime::start`], which acquires the single-instance lock,
//!      opens persistence (Task 08), and installs signal handlers.
//!   4. If another instance was already running, logs and exits 0.
//!   5. Otherwise blocks on [`Runtime::wait_for_shutdown`] until a signal
//!      fires (SIGTERM/SIGINT on Unix; Ctrl-C on Windows).
//!   6. Calls [`Runtime::stop`], which shuts down persistence and releases
//!      the lock.

use std::sync::Arc;

#[cfg(unix)]
use concerto_core::agent_supervisor::{AgentSupervisorActor, AgentSupervisorConfig};
use concerto_core::api_server::{ApiServerActor, ApiServerConfig};
use concerto_core::logging;
use concerto_core::repo_manager::{RepoManagerActor, RepoManagerConfig};
use concerto_core::runtime::{Runtime, RuntimeConfig, StartOutcome};
#[cfg(unix)]
use concerto_core::scheduler::{SchedulerActor, SchedulerConfig};
use concerto_core::workspace_manager::{
    WorkareaManagerActor, WorkareaManagerConfig, WorkspaceManagerActor, WorkspaceManagerConfig,
};
use concerto_error::Result;

fn main() -> std::process::ExitCode {
    // Logging is sync; install it before we hand control to tokio so the
    // runtime's own setup messages land in the log.
    let _log_guard = match logging::init() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("failed to initialize logging: {e}");
            return std::process::ExitCode::from(1);
        }
    };

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(error = %e, "failed to build tokio runtime");
            return std::process::ExitCode::from(1);
        }
    };

    match rt.block_on(run()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "concerto-core exited with error");
            std::process::ExitCode::from(1)
        }
    }
}

/// Async entry point. Errors are returned to `main`, which logs and exits
/// with a non-zero code — no `panic!`s on the startup path.
async fn run() -> Result<()> {
    tracing::info!("concerto-core starting");

    let config = RuntimeConfig::default_for_user()?;
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
            // Per design/01 §3.3: exit 0 so launchd doesn't restart us.
            // The "another instance running" log line is already emitted
            // by Runtime::start; the higher level just acknowledges.
            tracing::info!(
                other_pid = pid,
                "exiting cleanly — another instance is live"
            );
            return Ok(());
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

    // Task 19: spawn the Workspace Manager. Same pattern as the repo
    // manager — the actor's `run` parks on shutdown; the cheap-to-clone
    // handle is what the gRPC `Workspaces` service holds.
    let workspace_actor =
        WorkspaceManagerActor::new(Arc::clone(&persistence), Arc::clone(&config_dir));
    let workspace_handle = workspace_actor.handle();
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
        let host_bin = concerto_core::agent_supervisor::spawn::default_host_binary()?;
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
    match concerto_core::agent_supervisor::adopt_orphans(&agent_supervisor_handle).await {
        Ok(0) => tracing::debug!("pty hot-reconnect sweep: no surviving hosts"),
        Ok(n) => tracing::info!(adopted = n, "pty hot-reconnect sweep complete"),
        Err(e) => tracing::warn!(error = %e, "pty hot-reconnect sweep failed"),
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
                )
            },
            ApiServerConfig { socket_path },
        )
        .await?;

    tracing::info!("concerto-core ready");

    runtime.wait_for_shutdown().await?;
    tracing::info!("shutdown signal observed");

    runtime.stop().await?;
    tracing::info!("concerto-core stopped");
    Ok(())
}
