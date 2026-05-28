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

use crate::repo_manager::RepoManager;
use crate::supervisor::{Actor, ActorContext, SupervisorView};
use crate::workspace_manager::WorkspaceManager;

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
        }
    }

    /// Build a new actor that hosts every optional subsystem service.
    /// `Runtime` is always exposed; `Repositories` is registered when
    /// `repo_manager` is `Some`; `Workspaces` is registered when
    /// `workspace_manager` is `Some`. Task 19 added the workspace path.
    pub fn with_managers(
        started_at: Arc<SystemTime>,
        supervisor_view: SupervisorView,
        repo_manager: Option<RepoManager>,
        workspace_manager: Option<WorkspaceManager>,
    ) -> Self {
        Self {
            started_at,
            supervisor_view,
            repo_manager,
            workspace_manager,
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
                ctx.shutdown,
                ctx.config,
            );
            Err(Error::Internal(format!(
                "UDS gRPC server not supported on {} in V0.1; Windows named-pipe support lands in V1.0",
                std::env::consts::OS
            )))
        }
    }
}

#[cfg(unix)]
async fn run_uds(
    socket_path: std::path::PathBuf,
    started_at: Arc<SystemTime>,
    supervisor_view: SupervisorView,
    repo_manager: Option<RepoManager>,
    workspace_manager: Option<WorkspaceManager>,
    shutdown: tokio_util::sync::CancellationToken,
) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    use tokio::net::UnixListener;
    use tokio_stream::wrappers::UnixListenerStream;
    use tonic::transport::Server;

    use crate::handlers::repositories::RepositoriesHandler;
    use crate::handlers::runtime::RuntimeHandler;
    use crate::handlers::workspaces::WorkspacesHandler;
    use concerto_proto::v1::repositories_server::RepositoriesServer;
    use concerto_proto::v1::runtime_server::RuntimeServer;
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
    if let Some(repo_manager) = repo_manager {
        let repositories_service = RepositoriesServer::new(RepositoriesHandler::new(repo_manager));
        builder = builder.add_service(repositories_service);
    }
    if let Some(workspace_manager) = workspace_manager {
        let workspaces_service = WorkspacesServer::new(WorkspacesHandler::new(workspace_manager));
        builder = builder.add_service(workspaces_service);
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
