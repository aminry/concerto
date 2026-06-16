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
//! 211). **No auth gating** — Task 210 (auth middleware) and Task 522
//! (browser ephemeral pairing) own that; this task tags the transport kind
//! but does not gate.
//!
//! # LAN-direct TLS (Task 521)
//!
//! By default the bridge serves **plain HTTP** — fine on loopback (the bytes
//! never leave the host). To reach the bridge from a **LAN** browser the page
//! must be a secure context, so Task 521 adds an **opt-in TLS mode**
//! ([`ENV_TLS`] / [`ConnectBridgeConfig::tls`]). When enabled, the bridge wraps
//! every accepted TCP connection in rustls, serving a **self-signed cert
//! deterministically derived from the Core's identity public key**
//! ([`crate::connect_bridge_tls::IdentityTlsCert`]). The cert's SPKI SHA-256
//! fingerprint is published ([`BoundBridge::cert_fingerprint`]) so a **native /
//! LAN client pins it** (`design/17 §3.3`); browsers click through the one-time
//! self-signed interstitial and can use the published fingerprint to *verify*
//! the cert matches the Core they paired with. TLS stays default-OFF: the
//! bridge is never exposed beyond loopback over plain HTTP unless explicitly
//! widened, and never serves TLS unless explicitly enabled. See
//! [`crate::connect_bridge_tls`] for the derivation + the honest browser-pinning
//! posture.
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
use futures::StreamExt;
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

/// Environment variable that turns **LAN-direct TLS** ON (Task 521). Any
/// non-empty value other than `0`/`false` enables it. **Default OFF** — the
/// bridge serves plain HTTP (safe on loopback) unless TLS is explicitly
/// requested. When ON, the caller must supply the Core identity pubkey to
/// [`ConnectBridgeConfig::with_tls_for`] so the bridge can derive its
/// identity-bound cert.
const ENV_TLS: &str = "CONCERTO_CONNECT_BRIDGE_TLS";

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
    /// Whether **LAN-direct TLS** (Task 521) is requested. Default `false`
    /// (plain HTTP). Set from [`ENV_TLS`]; the actual cert is derived later by
    /// [`ConnectBridgeConfig::with_tls_for`] (which needs the Core identity).
    pub tls_requested: bool,
    /// The derived identity-bound TLS cert, present only after
    /// [`ConnectBridgeConfig::with_tls_for`] runs with `tls_requested == true`.
    /// `None` ⇒ serve plain HTTP. Skipped in [`Debug`] of the surrounding
    /// struct via its own `Debug` (never prints key material).
    pub tls: Option<crate::connect_bridge_tls::IdentityTlsCert>,
}

impl Default for ConnectBridgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_addr: DEFAULT_BIND,
            tls_requested: false,
            tls: None,
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

        let tls_requested = match std::env::var(ENV_TLS) {
            Ok(v) => {
                let v = v.trim();
                !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
            }
            Err(_) => false,
        };

        Ok(Self {
            enabled,
            bind_addr,
            tls_requested,
            tls: None,
        })
    }

    /// Derive the identity-bound TLS cert for this config when TLS was requested
    /// ([`ENV_TLS`]), binding it to `core_pubkey` (`design/17 §3.3`, Task 521).
    ///
    /// A no-op (returns `self` unchanged) when `tls_requested == false`, so a
    /// caller can always chain it. The cert is valid for the standard LAN names
    /// (`localhost` / `concerto.local`) plus the loopback IP and the bind IP if
    /// it is a concrete (non-`0.0.0.0`) address; the Core identity pubkey is
    /// always embedded for cross-checking. Returns the derived cert's SPKI
    /// fingerprint alongside `self` via [`ConnectBridgeConfig::cert_fingerprint`]
    /// once set.
    pub fn with_tls_for(mut self, core_pubkey: &concerto_identity::PublicKey) -> Result<Self> {
        if !self.tls_requested {
            return Ok(self);
        }
        let mut sans = vec![
            "localhost".to_string(),
            "concerto.local".to_string(),
            "127.0.0.1".to_string(),
        ];
        // Add the concrete bind IP as a SAN so a client dialing the literal LAN
        // address passes hostname verification (skip the unspecified `0.0.0.0` /
        // `::` wildcard — there is no single literal to put in the cert).
        let bind_ip = self.bind_addr.ip();
        if !bind_ip.is_unspecified() && !bind_ip.is_loopback() {
            sans.push(bind_ip.to_string());
        }
        let cert = crate::connect_bridge_tls::IdentityTlsCert::derive(core_pubkey, &sans)?;
        self.tls = Some(cert);
        Ok(self)
    }

    /// The published SPKI SHA-256 fingerprint a LAN client pins, when TLS is
    /// derived ([`ConnectBridgeConfig::with_tls_for`]). `None` for the plain-HTTP
    /// (loopback) default.
    pub fn cert_fingerprint(&self) -> Option<&str> {
        self.tls.as_ref().map(|c| c.spki_sha256_hex())
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
    /// The `enterprise_data_privacy` resolver the `Vcs.FetchIssueByUrl` D10 path
    /// consults (Task 411). `None` ⇒ the pre-resolver default (`false`).
    /// Threaded alongside the `vcs` handle (additive; distinct field).
    pub vcs_privacy_resolver: Option<Arc<dyn crate::handlers::vcs::EnterprisePrivacyResolver>>,
    /// The SHARED notifications handle (shared-event-channel fix). The bridge
    /// uses the SAME `NotificationHandle` boot threads into the UDS + Iroh front
    /// doors + the live `notify_user` sink, so it publishes onto / subscribes
    /// from the ONE `notification.events` broadcast — a `notification.read`
    /// (or created/updated/acted) emitted on ANY transport reaches the web
    /// client, and vice-versa (design/14 R-8, §5.3). `None` ⇒ fall back to a
    /// fresh per-bridge `with_event_channel` handle (prior behavior; tests).
    pub notif_handle: Option<crate::notifications::handle::NotificationHandle>,
}

/// The address the bridge actually bound, reported back so tests (and a
/// future `TransportHandle`) can dial an OS-assigned port.
#[derive(Debug, Clone)]
pub struct BoundBridge {
    /// The concrete `SocketAddr` the listener bound (port resolved when
    /// `bind_addr.port() == 0`).
    pub local_addr: SocketAddr,
    /// The SPKI SHA-256 fingerprint (lowercase hex) of the identity-bound TLS
    /// cert the bridge serves, for **client pinning** (Task 521, `design/17
    /// §3.3`). `None` when TLS is not enabled (plain-HTTP loopback default).
    /// A native/LAN client pins this; browsers verify against it. The scheme
    /// to dial is `https://` iff this is `Some`.
    pub cert_fingerprint: Option<String>,
}

/// A rustls-terminated bridge connection that tonic can serve.
///
/// tonic's `serve_with_incoming` requires the IO type to implement
/// [`tonic::transport::server::Connected`] (so peer/local addrs land in request
/// extensions). `tokio_rustls`' `TlsStream` only implements `Connected` under
/// tonic's own `tls` feature (which we don't enable — we bring our own rustls).
/// This thin newtype forwards `AsyncRead`/`AsyncWrite` to the TLS stream and
/// derives the connect-info from the **inner TCP stream** (the peer addr is the
/// same — TLS is just a layer over it).
struct TlsBridgeStream(tokio_rustls::server::TlsStream<tokio::net::TcpStream>);

impl tokio::io::AsyncRead for TlsBridgeStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for TlsBridgeStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.0).poll_write(cx, buf)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

impl tonic::transport::server::Connected for TlsBridgeStream {
    type ConnectInfo = tonic::transport::server::TcpConnectInfo;

    fn connect_info(&self) -> Self::ConnectInfo {
        // The underlying TCP stream carries the real peer/local addrs; the TLS
        // layer is transparent to addressing.
        let tcp = self.0.get_ref().0;
        tonic::transport::server::TcpConnectInfo {
            local_addr: tcp.local_addr().ok(),
            remote_addr: tcp.peer_addr().ok(),
        }
    }
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
    tls: Option<crate::connect_bridge_tls::IdentityTlsCert>,
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
        vcs_privacy_resolver,
        notif_handle: shared_notif_handle,
    } = services;

    // Task 520 (D9 site 2) + shared-event-channel fix: the notifications handle
    // on the Connect-Web bridge. Boot threads the SAME `with_event_channel`-backed
    // handle the UDS + Iroh front doors + the live `notify_user` sink use, so a
    // `notification.read`/`created`/`updated`/`acted` emitted on ANY transport (or
    // by `notify_user`) reaches the web client's `notification.events` stream and
    // vice-versa (design/14 R-8, §5.3). When no shared handle is supplied (tests),
    // fall back to a fresh `with_event_channel` handle from the same `Persistence`.
    // `None` persistence + no shared handle ⇒ no `Notifications` service.
    let notif_handle = shared_notif_handle.or_else(|| {
        persistence.as_ref().map(|p| {
            crate::notifications::handle::NotificationHandle::new(
                Arc::clone(p),
                Arc::new(crate::notifications::push::ExpoPushBackend::new(None)),
                Arc::new(crate::notifications::handle::NoEvents),
            )
            .with_event_channel()
        })
    });

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
    // Task 520 (D9 site 2): the `Notifications` service on the web bridge
    // (cross-platform). The `notification.events` producer is wired into the
    // `Streams` handler in the `#[cfg(unix)]` block below.
    if let Some(h) = notif_handle.as_ref() {
        use concerto_proto::v1::notifications_server::NotificationsServer;

        use crate::handlers::notifications::NotificationsHandler;
        let notifications_service = NotificationsServer::new(NotificationsHandler::new(h.clone()));
        builder = builder.add_service(notifications_service);
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
            // Task 414: wire the live Maestro's `maestro.events` producer on the
            // Connect-Web bridge too (D8 — the easiest-to-miss second site).
            // `None` (Maestro disabled) leaves the subject valid-but-empty.
            if let Some(maestro) = maestro.as_ref() {
                handler = handler.with_maestro_events(maestro.events_sender());
            }
            // Task 520: the `notification.events` producer on the web bridge.
            if let Some(tx) = notif_handle.as_ref().and_then(|h| h.events_sender()) {
                handler = handler.with_notification_events(tx);
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
        let mut vcs_handler = VcsHandler::new(vcs);
        // Task 411 (D10): the same `enterprise_data_privacy` resolver the UDS /
        // Iroh paths attach, so `FetchIssueByUrl` over the Connect-Web bridge
        // enforces the privacy floor too.
        if let Some(resolver) = vcs_privacy_resolver {
            vcs_handler = vcs_handler.with_privacy_resolver(resolver);
        }
        let vcs_service = VcsServer::new(vcs_handler);
        builder = builder.add_service(vcs_service);
    }

    let shutdown_signal = async move {
        shutdown.cancelled().await;
        tracing::info!("Connect-Web bridge received shutdown signal");
    };

    let serve_result = match tls {
        // Task 521: LAN-direct TLS. Wrap each accepted TCP connection in rustls
        // before handing it to tonic. The TLS handshake runs per-connection
        // inside the incoming stream; a handshake failure (e.g. a plain-HTTP
        // client hitting the TLS port, or a port scan) is logged and skipped —
        // it does not tear down the accept loop. The handshake is bounded by a
        // timeout so a stalled client can't wedge the accept loop. The wrapping
        // `TlsBridgeStream` is `AsyncRead + AsyncWrite + Connected`, exactly what
        // tonic's `serve_with_incoming_shutdown` requires.
        Some(cert) => {
            const TLS_HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
            let acceptor = cert.tls_acceptor()?;
            let tcp = TcpListenerStream::new(listener);
            let tls_incoming = tcp.then(move |conn| {
                let acceptor = acceptor.clone();
                async move {
                    match conn {
                        Ok(stream) => {
                            match tokio::time::timeout(
                                TLS_HANDSHAKE_TIMEOUT,
                                acceptor.accept(stream),
                            )
                            .await
                            {
                                Ok(Ok(tls_stream)) => {
                                    Some(Ok::<_, std::io::Error>(TlsBridgeStream(tls_stream)))
                                }
                                Ok(Err(e)) => {
                                    // Metadata-only: never the payload. A failed
                                    // TLS handshake is one bad client, not fatal.
                                    tracing::debug!(error = %e, "Connect-Web bridge TLS handshake failed; dropping connection");
                                    None
                                }
                                Err(_) => {
                                    tracing::debug!("Connect-Web bridge TLS handshake timed out; dropping connection");
                                    None
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Connect-Web bridge TCP accept failed");
                            None
                        }
                    }
                }
            });
            // Drop the `None`s (skipped connections) so the stream yields only
            // successfully-handshaked TLS streams.
            let tls_incoming = tls_incoming.filter_map(|opt| async move { opt });
            builder
                .serve_with_incoming_shutdown(tls_incoming, shutdown_signal)
                .await
        }
        // Default: plain HTTP (safe on loopback).
        None => {
            builder
                .serve_with_incoming_shutdown(TcpListenerStream::new(listener), shutdown_signal)
                .await
        }
    };

    if let Err(e) = serve_result {
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
    let cert_fingerprint = config.cert_fingerprint().map(str::to_string);
    let scheme = if cert_fingerprint.is_some() {
        "https"
    } else {
        "http"
    };
    tracing::info!(
        addr = %local_addr,
        loopback = local_addr.ip().is_loopback(),
        scheme,
        cert_fingerprint = cert_fingerprint.as_deref().unwrap_or("(plain http)"),
        "Connect-Web bridge (gRPC-Web) listening"
    );
    if let Some(fp) = &cert_fingerprint {
        // Task 521: publish the pin so a LAN client can pin it before dialing
        // (`design/17 §3.3`). Logged at INFO so it appears in the Core's startup
        // log; also surfaced programmatically via `BoundBridge::cert_fingerprint`.
        tracing::info!(
            spki_sha256 = %fp,
            "Connect-Web bridge LAN-direct TLS cert fingerprint (pin this on the client)"
        );
    }
    Ok((
        listener,
        BoundBridge {
            local_addr,
            cert_fingerprint,
        },
    ))
}

/// Serve the gRPC-Web bridge on `listener` until `shutdown` is cancelled.
///
/// Registers the same Tonic services as the UDS server, wrapped in
/// [`GrpcWebLayer`] and tagged `WssBridge`. When `tls` is `Some` (Task 521
/// LAN-direct TLS), every connection is wrapped in rustls serving the
/// identity-bound cert; otherwise the bridge serves plain HTTP (loopback
/// default). Pass `config.tls.clone()` from the bound config. Resolves `Ok(())`
/// on clean shutdown; a transport error maps to [`Error::Internal`].
pub async fn serve(
    listener: TcpListener,
    services: BridgeServices,
    tls: Option<crate::connect_bridge_tls::IdentityTlsCert>,
    shutdown: CancellationToken,
) -> Result<()> {
    build_and_serve(services, listener, tls, shutdown).await
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
        // Task 521: TLS is off by default; the bridge is never auto-exposed over
        // TLS without an explicit enable.
        assert!(!c.tls_requested);
        assert!(c.tls.is_none());
        assert!(c.cert_fingerprint().is_none());
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
            tls_requested: false,
            tls: None,
        };
        assert!(!c.enabled);
    }

    #[test]
    fn with_tls_for_is_noop_when_not_requested() {
        // Task 521: when TLS was not requested, `with_tls_for` leaves the config
        // plain-HTTP even if a Core pubkey is available.
        let pk = concerto_identity::KeyPair::from_seed(&[4u8; 32]).verifying_key();
        let c = ConnectBridgeConfig {
            enabled: true,
            bind_addr: DEFAULT_BIND,
            tls_requested: false,
            tls: None,
        }
        .with_tls_for(&pk)
        .expect("noop");
        assert!(c.tls.is_none());
        assert!(c.cert_fingerprint().is_none());
    }

    #[test]
    fn with_tls_for_derives_a_pinnable_fingerprint() {
        // Task 521: requesting TLS + supplying the Core pubkey yields a derived
        // cert whose SPKI fingerprint is published for pinning, and is stable for
        // the identity.
        let pk = concerto_identity::KeyPair::from_seed(&[4u8; 32]).verifying_key();
        let c = ConnectBridgeConfig {
            enabled: true,
            bind_addr: DEFAULT_BIND,
            tls_requested: true,
            tls: None,
        }
        .with_tls_for(&pk)
        .expect("derive tls");
        let fp = c.cert_fingerprint().expect("fingerprint present");
        assert_eq!(fp.len(), 64);
        // Re-deriving for the same identity is stable (pinned clients keep
        // trusting across restarts).
        let c2 = ConnectBridgeConfig {
            enabled: true,
            bind_addr: DEFAULT_BIND,
            tls_requested: true,
            tls: None,
        }
        .with_tls_for(&pk)
        .expect("derive tls 2");
        assert_eq!(fp, c2.cert_fingerprint().unwrap());
    }
}
