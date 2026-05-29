//! gRPC server over a Unix Domain Socket (Task 13).
//!
//! Exposed as an [`Actor`] under the Task 12 supervision tree. The
//! actor:
//!
//! - Removes any stale socket file at `<config_dir>/core.sock` left
//!   over from a prior process (only if the path is actually a
//!   socket — never deletes arbitrary files).
//! - Binds a [`tokio::net::UnixListener`].
//! - Sets `0o600` permissions on the socket so only the owning user
//!   can connect (V0.1 trusts whoever is on the box).
//! - Hosts the generated [`RuntimeServer`] from `concerto-proto`,
//!   backed by [`RuntimeHandler`] which reads live state from the
//!   runtime.
//! - On `ctx.shutdown.cancelled()`, breaks out of `serve_with_incoming`
//!   and removes the socket file best-effort.
//!
//! Windows is not supported in V0.1 (the design carves out macOS-only
//! for the alpha; a Windows port using named pipes lands in V1.0).
//! The `#[cfg(unix)]` gate emits a clear `Internal` error on
//! non-Unix targets so the supervisor can record the failure.

use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use concerto_error::{Error, Result};

#[cfg(unix)]
use crate::agent_supervisor::AgentSupervisorHandle;
use crate::repo_manager::RepoManager;
#[cfg(unix)]
use crate::scheduler::SchedulerHandle;
use crate::skills::SkillsRegistryHandle;
#[cfg(unix)]
use crate::suggestions::SuggestionEngineHandle;
use crate::supervisor::{Actor, ActorContext, SupervisorView};
use crate::vcs::VcsHandle;
use crate::workspace_manager::{WorkareaManager, WorkspaceManager};
use concerto_persist::Persistence;

/// Configuration for [`ApiServerActor`].
#[derive(Clone)]
pub struct ApiServerConfig {
    /// Absolute path where the listener binds. Default
    /// `<config_dir>/core.sock` per `design/01 §4.1` and the
    /// locked surface in `tasks/13`.
    pub socket_path: std::path::PathBuf,
}

/// Supervised actor that owns the gRPC server.
///
/// Built once per Core boot. The supervisor's factory closure clones
/// the inner `Arc` handles on each (re)start, so a panic in `serve`
/// can be recovered without losing the started-at timestamp or
/// supervisor view.
pub struct ApiServerActor {
    started_at: Arc<SystemTime>,
    supervisor_view: SupervisorView,
    /// Optional Repository Manager handle. When `Some`, the gRPC
    /// `Repositories` service is registered alongside `Runtime`. Task 18
    /// wires this up in `main.rs`; the option type keeps the integration
    /// tests' minimal in-process Runtime construction working without
    /// requiring a fully-initialised `RepoManager`.
    repo_manager: Option<RepoManager>,
    /// Optional Workspace Manager handle. When `Some`, the gRPC
    /// `Workspaces` service is registered. Task 19 wires this up.
    workspace_manager: Option<WorkspaceManager>,
    /// Optional Workarea Manager handle. When `Some`, the gRPC
    /// `Workareas` service is registered. Task 20 wires this up.
    workarea_manager: Option<WorkareaManager>,
    /// Optional Agent Supervisor handle. When `Some` AND the workarea
    /// manager is also `Some`, the gRPC `Sessions` and `Streams`
    /// services are registered (Task 23). The Streams service piggy-backs
    /// on the workspace + workarea managers' broadcast channels for
    /// `workspace.events` / `workarea.events`.
    #[cfg(unix)]
    agent_supervisor: Option<AgentSupervisorHandle>,
    /// Optional `Persistence` handle. When `Some`, the gRPC `Projects`
    /// service is registered (Task 24) so the Desktop sidebar can list
    /// projects without hardcoding a project id. V0.1 ships read-only;
    /// creation is still seeded via direct SQL.
    persistence: Option<Arc<Persistence>>,
    /// Optional Scheduler handle. When `Some`, the gRPC `Schedules`
    /// service is registered (Task 38). Wired in `main.rs` once the
    /// Agent Supervisor exists (the Scheduler holds a supervisor
    /// handle to drive `start_session` on fire).
    #[cfg(unix)]
    scheduler: Option<SchedulerHandle>,
    /// Optional Skills Registry handle. When `Some`, the gRPC `Skills`
    /// service is registered (Task 39).
    skills_registry: Option<SkillsRegistryHandle>,
    /// Optional Suggestion Engine handle. When `Some`, the gRPC
    /// `Suggestions` service is registered (Task 40) and the
    /// `Streams` handler gains a producer for the `suggestion.events`
    /// subject.
    #[cfg(unix)]
    suggestions: Option<SuggestionEngineHandle>,
    /// Optional VCS Provider handle. When `Some`, the gRPC `Vcs`
    /// service is registered (Task 45). The handle is cheap to clone
    /// and lazily resolves the `gh` binary path on first use.
    vcs: Option<VcsHandle>,
}

impl ApiServerActor {
    /// Build a new actor without any subsystem handles. Only the
    /// `Runtime` service is exposed.
    pub fn new(started_at: Arc<SystemTime>, supervisor_view: SupervisorView) -> Self {
        Self {
            started_at,
            supervisor_view,
            repo_manager: None,
            workspace_manager: None,
            workarea_manager: None,
            #[cfg(unix)]
            agent_supervisor: None,
            persistence: None,
            #[cfg(unix)]
            scheduler: None,
            skills_registry: None,
            #[cfg(unix)]
            suggestions: None,
            vcs: None,
        }
    }

    /// Build a new actor that also hosts the `Repositories` service.
    /// Kept for back-compat with Task 18 call sites that don't yet wire
    /// the workspace manager; new call sites should prefer
    /// [`ApiServerActor::with_managers`].
    pub fn with_repo_manager(
        started_at: Arc<SystemTime>,
        supervisor_view: SupervisorView,
        repo_manager: RepoManager,
    ) -> Self {
        Self {
            started_at,
            supervisor_view,
            repo_manager: Some(repo_manager),
            workspace_manager: None,
            workarea_manager: None,
            #[cfg(unix)]
            agent_supervisor: None,
            persistence: None,
            #[cfg(unix)]
            scheduler: None,
            skills_registry: None,
            #[cfg(unix)]
            suggestions: None,
            vcs: None,
        }
    }

    /// Build a new actor that hosts every optional subsystem service.
    /// `Runtime` is always exposed; `Repositories` is registered when
    /// `repo_manager` is `Some`; `Workspaces` is registered when
    /// `workspace_manager` is `Some`; `Workareas` is registered when
    /// `workarea_manager` is `Some`. Task 19 added the workspace path;
    /// Task 20 added the workarea path.
    #[allow(clippy::too_many_arguments)]
    pub fn with_managers(
        started_at: Arc<SystemTime>,
        supervisor_view: SupervisorView,
        repo_manager: Option<RepoManager>,
        workspace_manager: Option<WorkspaceManager>,
        workarea_manager: Option<WorkareaManager>,
        #[cfg(unix)] agent_supervisor: Option<AgentSupervisorHandle>,
        persistence: Option<Arc<Persistence>>,
        #[cfg(unix)] scheduler: Option<SchedulerHandle>,
        skills_registry: Option<SkillsRegistryHandle>,
        #[cfg(unix)] suggestions: Option<SuggestionEngineHandle>,
        vcs: Option<VcsHandle>,
    ) -> Self {
        Self {
            started_at,
            supervisor_view,
            repo_manager,
            workspace_manager,
            workarea_manager,
            #[cfg(unix)]
            agent_supervisor,
            persistence,
            #[cfg(unix)]
            scheduler,
            skills_registry,
            #[cfg(unix)]
            suggestions,
            vcs,
        }
    }
}

#[async_trait]
impl Actor for ApiServerActor {
    const NAME: &'static str = "api-server";
    type Config = ApiServerConfig;

    async fn run(self, ctx: ActorContext<Self::Config>) -> Result<()> {
        #[cfg(unix)]
        {
            let socket_path = {
                let cfg = ctx.config.read().await;
                cfg.socket_path.clone()
            };
            run_uds(
                socket_path,
                self.started_at,
                self.supervisor_view,
                self.repo_manager,
                self.workspace_manager,
                self.workarea_manager,
                self.agent_supervisor,
                self.persistence,
                self.scheduler,
                self.skills_registry,
                self.suggestions,
                self.vcs,
                ctx.shutdown,
            )
            .await
        }
        #[cfg(not(unix))]
        {
            let _ = (
                self.started_at,
                self.supervisor_view,
                self.repo_manager,
                self.workspace_manager,
                self.workarea_manager,
                self.persistence,
                self.skills_registry,
                self.vcs,
                ctx.shutdown,
                ctx.config,
            );
            // `suggestions` is `#[cfg(unix)]`; the non-unix branch does
            // not need to drop it explicitly.
            // `scheduler` is not present on non-unix targets — the
            // field is `#[cfg(unix)]`.
            Err(Error::Internal(format!(
                "UDS gRPC server not supported on {} in V0.1; Windows named-pipe support lands in V1.0",
                std::env::consts::OS
            )))
        }
    }
}

#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
async fn run_uds(
    socket_path: std::path::PathBuf,
    started_at: Arc<SystemTime>,
    supervisor_view: SupervisorView,
    repo_manager: Option<RepoManager>,
    workspace_manager: Option<WorkspaceManager>,
    workarea_manager: Option<WorkareaManager>,
    agent_supervisor: Option<AgentSupervisorHandle>,
    persistence: Option<Arc<Persistence>>,
    scheduler: Option<SchedulerHandle>,
    skills_registry: Option<SkillsRegistryHandle>,
    suggestions: Option<SuggestionEngineHandle>,
    vcs: Option<VcsHandle>,
    shutdown: tokio_util::sync::CancellationToken,
) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    use tokio::net::UnixListener;
    use tokio_stream::wrappers::UnixListenerStream;
    use tonic::transport::Server;

    use crate::handlers::projects::ProjectsHandler;
    use crate::handlers::repositories::RepositoriesHandler;
    use crate::handlers::runtime::RuntimeHandler;
    use crate::handlers::schedules::SchedulesHandler;
    use crate::handlers::sessions::SessionsHandler;
    use crate::handlers::skills::SkillsHandler;
    use crate::handlers::streams::StreamsHandler;
    use crate::handlers::suggestions::SuggestionsHandler;
    use crate::handlers::vcs::VcsHandler;
    use crate::handlers::workareas::WorkareasHandler;
    use crate::handlers::workspaces::WorkspacesHandler;
    use concerto_proto::v1::projects_server::ProjectsServer;
    use concerto_proto::v1::repositories_server::RepositoriesServer;
    use concerto_proto::v1::runtime_server::RuntimeServer;
    use concerto_proto::v1::schedules_server::SchedulesServer;
    use concerto_proto::v1::sessions_server::SessionsServer;
    use concerto_proto::v1::skills_server::SkillsServer;
    use concerto_proto::v1::streams_server::StreamsServer;
    use concerto_proto::v1::suggestions_server::SuggestionsServer;
    use concerto_proto::v1::vcs_server::VcsServer;
    use concerto_proto::v1::workareas_server::WorkareasServer;
    use concerto_proto::v1::workspaces_server::WorkspacesServer;

    // Ensure the parent directory exists; the locked layout puts the
    // socket inside `<config_dir>`, which the runtime creates on boot,
    // but we tolerate a fresh tempdir for tests.
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Remove a stale socket file if the prior Core crashed before
    // cleaning up. Be paranoid: only unlink the path if it actually
    // is a socket — never blow away an arbitrary file the user might
    // have left at `<config_dir>/core.sock`.
    if socket_path.exists() {
        let md = std::fs::metadata(&socket_path)?;
        if is_socket(&md) {
            tracing::info!(
                socket = %socket_path.display(),
                "removing stale UDS socket from prior run"
            );
            std::fs::remove_file(&socket_path)?;
        } else {
            return Err(Error::Internal(format!(
                "socket path {} exists and is not a socket; refusing to overwrite",
                socket_path.display()
            )));
        }
    }

    let listener = UnixListener::bind(&socket_path)?;
    // Tighten permissions to owner-only. The default umask usually
    // produces 0o755 for new sockets which is too lax.
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
    tracing::info!(
        socket = %socket_path.display(),
        mode = "0600",
        "gRPC server listening on UDS"
    );

    // RAII guard removes the socket file on every exit path — clean
    // shutdown, internal error, OR the future being dropped because
    // the supervisor cancelled us in its outer `select!`. This is the
    // ONLY reliable cleanup hook: the supervisor races
    // `stop.cancelled()` against `run_fut`, and the cancellation
    // branch drops the future without giving it a chance to run
    // graceful cleanup code.
    let _cleanup = SocketCleanupGuard {
        path: socket_path.clone(),
    };

    let handler = RuntimeHandler::new(started_at, supervisor_view);
    let runtime_service = RuntimeServer::new(handler);

    let mut builder = Server::builder().add_service(runtime_service);
    if let Some(persistence) = persistence {
        let projects_service = ProjectsServer::new(ProjectsHandler::new(persistence));
        builder = builder.add_service(projects_service);
    }
    if let Some(repo_manager) = repo_manager {
        let repositories_service = RepositoriesServer::new(RepositoriesHandler::new(repo_manager));
        builder = builder.add_service(repositories_service);
    }
    // The Workspace + Workarea managers may also back `Streams` /
    // `Sessions`, so register the existing services from clones and keep
    // the originals available for the Task 23 services below.
    if let Some(workspace_manager) = workspace_manager.clone() {
        let workspaces_service = WorkspacesServer::new(WorkspacesHandler::new(workspace_manager));
        builder = builder.add_service(workspaces_service);
    }
    if let Some(workarea_manager) = workarea_manager.clone() {
        let workareas_service = WorkareasServer::new(WorkareasHandler::new(workarea_manager));
        builder = builder.add_service(workareas_service);
    }
    // Task 23: `Sessions` requires the agent supervisor + workarea
    // manager (to resolve the workarea's worktree root as the agent's
    // cwd). `Streams` requires all three of the agent supervisor +
    // workspace + workarea managers to back the four V0.1 subjects.
    if let (Some(supervisor), Some(workarea_mgr)) =
        (agent_supervisor.as_ref(), workarea_manager.as_ref())
    {
        let persistence = supervisor.persistence();
        let sessions_service = SessionsServer::new(SessionsHandler::new(
            supervisor.clone(),
            persistence,
            workarea_mgr.clone(),
        ));
        builder = builder.add_service(sessions_service);
    }
    if let (Some(supervisor), Some(workspace_mgr), Some(workarea_mgr)) =
        (agent_supervisor, workspace_manager, workarea_manager)
    {
        let mut handler = StreamsHandler::new(supervisor, workspace_mgr, workarea_mgr);
        if let Some(suggestions) = suggestions.clone() {
            handler = handler.with_suggestions(suggestions);
        }
        let streams_service = StreamsServer::new(handler);
        builder = builder.add_service(streams_service);
    }
    // Task 38: `Schedules` only needs the scheduler handle. It is wired
    // independently of the supervisor/workarea managers so a stripped-
    // down test Core can still expose the schedule surface.
    if let Some(scheduler) = scheduler {
        let schedules_service = SchedulesServer::new(SchedulesHandler::new(scheduler));
        builder = builder.add_service(schedules_service);
    }
    // Task 39: `Skills` registry. Independent of every other manager
    // — discovery walks the filesystem, the toggle path writes
    // `skills_index` directly, and the in-process broadcast channel
    // for `skill.*` events is V1.0.
    if let Some(skills_registry) = skills_registry {
        let skills_service = SkillsServer::new(SkillsHandler::new(skills_registry));
        builder = builder.add_service(skills_service);
    }
    // Task 40: `Suggestions` registry. Independent of every other
    // manager — the engine consumes `session.events` via per-session
    // subscriptions; the gRPC surface just exposes the chip list and
    // outcome-record stub.
    if let Some(suggestions) = suggestions {
        let suggestions_service = SuggestionsServer::new(SuggestionsHandler::new(suggestions));
        builder = builder.add_service(suggestions_service);
    }
    // Task 45: `Vcs` provider integration via `gh` CLI shell-out.
    // Independent of every other manager — the handle owns its own
    // (lazy) `gh` path resolution + an `Arc<Persistence>` for the
    // `pull_requests` cache.
    if let Some(vcs) = vcs {
        let vcs_service = VcsServer::new(VcsHandler::new(vcs));
        builder = builder.add_service(vcs_service);
    }

    let serve_fut =
        builder.serve_with_incoming_shutdown(UnixListenerStream::new(listener), async move {
            shutdown.cancelled().await;
            tracing::info!("gRPC server received shutdown signal");
        });

    // `serve_with_incoming_shutdown` resolves on shutdown OR error.
    // Map a transport error into our internal error type; clean
    // shutdown returns `Ok(())`. The `SocketCleanupGuard` removes the
    // file regardless of which branch we take.
    if let Err(e) = serve_fut.await {
        return Err(Error::Internal(format!("gRPC server crashed: {e}")));
    }

    // Give the tonic background tasks a few ms to flush their last
    // frames before the supervisor's drain timeout kicks in. Well
    // below `SHUTDOWN_DRAIN_BUDGET` (10s).
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok(())
}

/// RAII socket-file cleanup. Removes the socket on every drop path —
/// normal `Ok` return, internal `Err` return, AND future cancellation
/// when the supervisor's outer `select!` races us on shutdown.
#[cfg(unix)]
struct SocketCleanupGuard {
    path: std::path::PathBuf,
}

#[cfg(unix)]
impl Drop for SocketCleanupGuard {
    fn drop(&mut self) {
        remove_socket_best_effort(&self.path);
    }
}

#[cfg(unix)]
fn is_socket(md: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::FileTypeExt;
    md.file_type().is_socket()
}

#[cfg(unix)]
fn remove_socket_best_effort(path: &std::path::Path) {
    if path.exists() {
        match std::fs::remove_file(path) {
            Ok(_) => tracing::debug!(socket = %path.display(), "removed UDS socket"),
            Err(e) => tracing::warn!(
                socket = %path.display(),
                error = %e,
                "failed to remove UDS socket during cleanup"
            ),
        }
    }
}
