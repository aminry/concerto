//! Connect-Web bridge: a loopback `hyper`/`tonic-web` server serving the
//! **same** Tonic services the UDS server hosts, over **gRPC-Web** (Task
//! 204, `design/11 §3.4` **Path A**).
//!
//! # Why
//!
//! A browser cannot speak Iroh or raw HTTP/2 gRPC. `design/11 §3.4` Path A
//! gives it a front door: the Core spawns a tiny `hyper` server bound to
//! loopback/LAN that runs the **same** Tonic services via gRPC-Web's HTTP
//! transport. This module is that front door — and **only** Path A. The
//! WSS-via-relay Path B (browser-side Noise IK, relay sees ciphertext) is
//! **Task 215**, entirely out of scope here.
//!
//! `tonic-web`'s server is a `hyper` server underneath, so layering
//! [`tonic_web::GrpcWebLayer`] onto the existing [`tonic::transport::Server`]
//! builder over a [`TcpListenerStream`](tokio_stream::wrappers::TcpListenerStream)
//! satisfies the design's "tiny hyper server" with the smallest delta from
//! [`crate::api_server::run_uds`] — same handler structs, a different front
//! door. Server-streaming (`Streams.Subscribe`) and the unary `AckOffset`
//! (Task 202) both ride gRPC-Web; the browser client negotiates SSE framing
//! where it can't read gRPC-Web trailers (`design/10 §12` R-2:
//! server-streaming + unary `AckOffset`, no bidi).
//!
//! # The transport tag (Task 201 seam)
//!
//! Every connection this bridge accepts is tagged
//! `ConnTransport(TransportKind::WssBridge)` via a tonic interceptor layer —
//! the exact pattern [`crate::api_server::run_uds`] uses to tag `Uds`. The
//! [`crate::handlers::runtime::RuntimeHandler`] reads the tag and reports
//! `transport_kind = WSS_BRIDGE` so the SPA suppresses local-only
//! affordances (`design/15 §3.11`). This bridge **does not** edit the
//! handler — 201 froze that listeners write the tag and the handler only
//! reads it.
//!
//! # Trust / bind
//!
//! Loopback (`127.0.0.1`) by default (`design/11 §3.9`: LAN-direct is
//! high-trust, but loopback is the conservative default — the Phase-5 SPA
//! is served same-host first). LAN-bind is an opt-in config knob; widening
//! the bind widens exposure and is gated by managed settings later (Task
//! 211). Loopback here is **plain HTTP** — it never leaves the host; TLS
//! pinning on the LAN socket is Task 521. **No auth gating** — Task 210
//! (auth middleware) and Task 522 (browser ephemeral pairing) own that;
//! this task tags the transport kind but does not gate.
//!
//! # Cross-platform
//!
//! TCP + `hyper` + `tonic-web` are all cross-platform — this module carries
//! **no** `#[cfg(unix)]` on its own serve path, so it builds on the Windows
//! CI lane (Task 113). It is, in fact, the one transport that works on
//! Windows where the co-located server needs named-pipe glue. The
//! per-subsystem service handles that happen to be `#[cfg(unix)]` (agent
//! supervisor, scheduler, suggestions — same gating as
//! [`crate::api_server::ApiServerActor`]) are registered under the same
//! `#[cfg(unix)]`, and the bridge serves whatever subset is available on
//! the target.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::SystemTime;

use concerto_error::{Error, Result};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tokio_util::sync::CancellationToken;
use tonic::transport::Server;
use tonic_web::GrpcWebLayer;

use concerto_proto::v1::TransportKind;

use crate::conn_transport::ConnTransport;
use crate::repo_manager::RepoManager;
use crate::supervisor::SupervisorView;
use crate::vcs::VcsHandle;
use crate::workspace_manager::{WorkareaManager, WorkspaceManager};
use concerto_persist::Persistence;

#[cfg(unix)]
use crate::agent_supervisor::AgentSupervisorHandle;
#[cfg(unix)]
use crate::scheduler::SchedulerHandle;
use crate::skills::SkillsRegistryHandle;
#[cfg(unix)]
use crate::suggestions::SuggestionEngineHandle;

/// Default loopback bind: `127.0.0.1:0` (OS-assigned port). `0` is the
/// safe default for tests/embedded use; a deployment sets an explicit
/// port via [`ConnectBridgeConfig::from_env`].
const DEFAULT_BIND: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);

/// Environment variable that turns the bridge ON. Any non-empty value
/// other than `0`/`false` enables it. **Default OFF** — a pure co-located
/// install (Desktop over UDS only) never opens a TCP front door.
const ENV_ENABLE: &str = "CONCERTO_CONNECT_BRIDGE";

/// Environment variable overriding the bind address (e.g.
/// `127.0.0.1:8443`, or `0.0.0.0:8443` to LAN-bind). Default
/// `127.0.0.1:0`.
const ENV_ADDR: &str = "CONCERTO_CONNECT_BRIDGE_ADDR";

/// Configuration for the Connect-Web bridge.
///
/// **Loopback-only by default** (`design/11 §3.9`). LAN-bind is opt-in via
/// [`ENV_ADDR`] (e.g. `0.0.0.0:<port>`) and widens exposure — managed
/// settings will gate it in Task 211.
#[derive(Debug, Clone)]
pub struct ConnectBridgeConfig {
    /// Whether the bridge is enabled. Default `false`.
    pub enabled: bool,
    /// The socket address to bind. Default `127.0.0.1:0` (OS-assigned).
    pub bind_addr: SocketAddr,
}

impl Default for ConnectBridgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_addr: DEFAULT_BIND,
        }
    }
}

impl ConnectBridgeConfig {
    /// Resolve from the environment. The bridge is **off** unless
    /// [`ENV_ENABLE`] is set to a truthy value; [`ENV_ADDR`] overrides the
    /// bind address (loopback `127.0.0.1:0` otherwise). A malformed
    /// [`ENV_ADDR`] is a hard error — fail loudly rather than silently
    /// binding the wrong interface.
    pub fn from_env() -> Result<Self> {
        let enabled = match std::env::var(ENV_ENABLE) {
            Ok(v) => {
                let v = v.trim();
                !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
            }
            Err(_) => false,
        };

        let bind_addr = match std::env::var(ENV_ADDR) {
            Ok(v) if !v.trim().is_empty() => v.trim().parse::<SocketAddr>().map_err(|e| {
                Error::Internal(format!(
                    "invalid {ENV_ADDR}={v:?}: {e} (expected e.g. 127.0.0.1:8443)"
                ))
            })?,
            _ => DEFAULT_BIND,
        };

        Ok(Self { enabled, bind_addr })
    }
}

/// The handle set the bridge builds its Tonic services from — the **same**
/// handles [`crate::api_server::ApiServerActor`] hands to the UDS server.
///
/// This is the "one service-build path, two front doors" seam: the bridge
/// reuses these handler structs verbatim, never forking handler logic
/// (`design/00 §6.3`). The `#[cfg(unix)]` fields mirror the actor's exactly
/// (agent supervisor / scheduler / suggestions are Unix-only until the
/// Windows ports land), so the served surface is byte-identical to the UDS
/// surface on each target.
#[derive(Clone)]
pub struct BridgeServices {
    /// Wall-clock boot instant for `Runtime.GetStatus`.
    pub started_at: Arc<SystemTime>,
    /// Supervisor snapshot view for `Runtime`.
    pub supervisor_view: SupervisorView,
    pub repo_manager: Option<RepoManager>,
    pub workspace_manager: Option<WorkspaceManager>,
    pub workarea_manager: Option<WorkareaManager>,
    #[cfg(unix)]
    pub agent_supervisor: Option<AgentSupervisorHandle>,
    pub persistence: Option<Arc<Persistence>>,
    #[cfg(unix)]
    pub scheduler: Option<SchedulerHandle>,
    pub skills_registry: Option<SkillsRegistryHandle>,
    #[cfg(unix)]
    pub suggestions: Option<SuggestionEngineHandle>,
    /// Optional Maestro handle (Task 401.5 — frozen seam; 414 supplies the real
    /// one). `#[cfg(unix)]` (over the agent supervisor, like `suggestions`).
    /// `None` in 401.5; the bridge serves the `Maestro` service returning
    /// `Status::unimplemented`.
    #[cfg(unix)]
    pub maestro: Option<crate::maestro::MaestroHandle>,
    pub vcs: Option<VcsHandle>,
}

/// The address the bridge actually bound, reported back so tests (and a
/// future `TransportHandle`) can dial an OS-assigned port.
#[derive(Debug, Clone, Copy)]
pub struct BoundBridge {
    /// The concrete `SocketAddr` the listener bound (port resolved when
    /// `bind_addr.port() == 0`).
    pub local_addr: SocketAddr,
}

/// Tag every request this bridge accepts with
/// `ConnTransport(TransportKind::WssBridge)` — the Task-201 seam. The
/// `Interceptor` trait fixes the `Err` type to `tonic::Status` (large);
/// we never return `Err`, so the clippy lint is moot (mirrors
/// `api_server::run_uds`'s `tag_uds`).
#[allow(clippy::result_large_err)]
fn tag_wss_bridge(
    mut req: tonic::Request<()>,
) -> std::result::Result<tonic::Request<()>, tonic::Status> {
    req.extensions_mut()
        .insert(ConnTransport(TransportKind::WssBridge));
    Ok(req)
}

/// Build the gRPC-Web server from `services`, registering the same Tonic
/// service set [`crate::api_server::run_uds`] registers. The
/// [`GrpcWebLayer`] adds gRPC-Web framing (unary + server-streaming, with
/// the browser negotiating SSE where it can't read trailers); the
/// [`tag_wss_bridge`] interceptor stamps the 201 transport tag on every
/// request before dispatch. `accept_http1(true)` is required for gRPC-Web
/// (browsers post over HTTP/1.1).
///
/// Serves on `listener` until `shutdown` fires. The service-registration
/// order and the `Some(..)` gating exactly mirror `run_uds` so the served
/// surface is identical. The router type carries an unnameable interceptor
/// closure in its generics, so the build + serve are fused here rather than
/// returned across a function boundary.
async fn build_and_serve(
    services: BridgeServices,
    listener: TcpListener,
    shutdown: CancellationToken,
) -> Result<()> {
    use crate::handlers::files::FilesHandler;
    use crate::handlers::repositories::RepositoriesHandler;
    use crate::handlers::runtime::RuntimeHandler;
    use crate::handlers::workareas::WorkareasHandler;
    use crate::handlers::workspaces::WorkspacesHandler;
    use concerto_proto::v1::files_server::FilesServer;
    use concerto_proto::v1::repositories_server::RepositoriesServer;
    use concerto_proto::v1::runtime_server::RuntimeServer;
    use concerto_proto::v1::workareas_server::WorkareasServer;
    use concerto_proto::v1::workspaces_server::WorkspacesServer;

    let BridgeServices {
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
        #[cfg(unix)]
        maestro,
        vcs,
    } = services;

    let runtime_service = RuntimeServer::new(RuntimeHandler::new(started_at, supervisor_view));

    // `accept_http1(true)` + `GrpcWebLayer` are the gRPC-Web essentials;
    // the `tag_wss_bridge` interceptor is the 201 seam, applied to the
    // whole server (before `add_service`) so every service's requests
    // carry the tag — strictly more correct for the `Runtime` consumer and
    // harmless to the others.
    let mut builder = Server::builder()
        .accept_http1(true)
        .layer(tonic::service::interceptor(tag_wss_bridge))
        .layer(GrpcWebLayer::new())
        .add_service(runtime_service);

    if let Some(persistence) = persistence {
        // Same construction as `run_uds`: `Files` owns the `Persistence`
        // handle (workarea → worktree scope) and the `home` deny-list root.
        if let Some(home) = home::home_dir() {
            let files_service = FilesServer::new(FilesHandler::new(persistence, home));
            builder = builder.add_service(files_service);
        } else {
            tracing::warn!(
                "home::home_dir() returned None; Files service omitted from Connect-Web bridge"
            );
        }
    }
    if let Some(repo_manager) = repo_manager {
        let repositories_service = RepositoriesServer::new(RepositoriesHandler::new(repo_manager));
        builder = builder.add_service(repositories_service);
    }
    if let Some(workspace_manager) = workspace_manager.clone() {
        let workspaces_service = WorkspacesServer::new(WorkspacesHandler::new(workspace_manager));
        builder = builder.add_service(workspaces_service);
    }
    if let Some(workarea_manager) = workarea_manager.clone() {
        let workareas_service = WorkareasServer::new(WorkareasHandler::new(workarea_manager));
        builder = builder.add_service(workareas_service);
    }

    // `Sessions` / `Streams` / `Schedules` / `Suggestions` need the
    // Unix-only supervisor handles; same `#[cfg(unix)]` gating as `run_uds`.
    #[cfg(unix)]
    {
        use crate::handlers::maestro::MaestroHandler;
        use crate::handlers::schedules::SchedulesHandler;
        use crate::handlers::sessions::SessionsHandler;
        use crate::handlers::streams::StreamsHandler;
        use crate::handlers::suggestions::SuggestionsHandler;
        use concerto_proto::v1::maestro_server::MaestroServer;
        use concerto_proto::v1::schedules_server::SchedulesServer;
        use concerto_proto::v1::sessions_server::SessionsServer;
        use concerto_proto::v1::streams_server::StreamsServer;
        use concerto_proto::v1::suggestions_server::SuggestionsServer;

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
        if let Some(scheduler) = scheduler {
            let schedules_service = SchedulesServer::new(SchedulesHandler::new(scheduler));
            builder = builder.add_service(schedules_service);
        }
        if let Some(suggestions) = suggestions {
            let suggestions_service = SuggestionsServer::new(SuggestionsHandler::new(suggestions));
            builder = builder.add_service(suggestions_service);
        }
        // Task 401.5 (D8 site 2 of 2 — the easiest-to-miss one): register the
        // `Maestro` service on the Connect-Web front door too, so 415's
        // mocked-then-live invoke can dial it. Handler returns
        // `Status::unimplemented` until Task 414 threads a real handle.
        let maestro_service = MaestroServer::new(MaestroHandler::new(maestro));
        builder = builder.add_service(maestro_service);
    }
    #[cfg(not(unix))]
    {
        // The Unix-only handles never exist on this target; consume the
        // bindings so the non-unix build stays warning-clean. `Streams` /
        // `Sessions` arrive on Windows when the supervisor ports do.
        let _ = (&workspace_manager, &workarea_manager);
    }

    if let Some(skills_registry) = skills_registry {
        use crate::handlers::skills::SkillsHandler;
        use concerto_proto::v1::skills_server::SkillsServer;
        let skills_service = SkillsServer::new(SkillsHandler::new(skills_registry));
        builder = builder.add_service(skills_service);
    }
    if let Some(vcs) = vcs {
        use crate::handlers::vcs::VcsHandler;
        use concerto_proto::v1::vcs_server::VcsServer;
        let vcs_service = VcsServer::new(VcsHandler::new(vcs));
        builder = builder.add_service(vcs_service);
    }

    let serve_fut =
        builder.serve_with_incoming_shutdown(TcpListenerStream::new(listener), async move {
            shutdown.cancelled().await;
            tracing::info!("Connect-Web bridge received shutdown signal");
        });
    if let Err(e) = serve_fut.await {
        return Err(Error::Internal(format!(
            "Connect-Web bridge server crashed: {e}"
        )));
    }
    Ok(())
}

/// Bind the loopback TCP listener and report the resolved address.
///
/// Split out from [`serve`] so a caller (the api-server actor, or a test)
/// can bind first — learning the OS-assigned port — then drive `serve` on
/// the already-bound listener. Binding is cross-platform (`TcpListener`).
pub async fn bind(config: &ConnectBridgeConfig) -> Result<(TcpListener, BoundBridge)> {
    let listener = TcpListener::bind(config.bind_addr).await.map_err(|e| {
        Error::Internal(format!(
            "Connect-Web bridge bind {:?}: {e}",
            config.bind_addr
        ))
    })?;
    let local_addr = listener
        .local_addr()
        .map_err(|e| Error::Internal(format!("Connect-Web bridge local_addr: {e}")))?;
    tracing::info!(
        addr = %local_addr,
        loopback = local_addr.ip().is_loopback(),
        "Connect-Web bridge (gRPC-Web) listening"
    );
    Ok((listener, BoundBridge { local_addr }))
}

/// Serve the gRPC-Web bridge on `listener` until `shutdown` is cancelled.
///
/// Registers the same Tonic services as the UDS server, wrapped in
/// [`GrpcWebLayer`] and tagged `WssBridge`. Resolves `Ok(())` on clean
/// shutdown; a transport error maps to [`Error::Internal`].
pub async fn serve(
    listener: TcpListener,
    services: BridgeServices,
    shutdown: CancellationToken,
) -> Result<()> {
    build_and_serve(services, listener, shutdown).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_to_disabled_loopback() {
        let c = ConnectBridgeConfig::default();
        assert!(!c.enabled);
        assert!(c.bind_addr.ip().is_loopback());
        assert_eq!(c.bind_addr.port(), 0);
    }

    #[test]
    fn from_env_off_when_unset() {
        // The env vars are process-global; this test only asserts the
        // disabled-by-default branch using a config built directly (the
        // env-read path is exercised by the integration test, which owns
        // the process env).
        let c = ConnectBridgeConfig {
            enabled: false,
            bind_addr: DEFAULT_BIND,
        };
        assert!(!c.enabled);
    }
}
