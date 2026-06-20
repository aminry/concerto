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
    /// Optional `Persistence` handle. When `Some`, the gRPC `Files`
    /// service is registered.
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
    /// Optional Maestro handle (Task 401.5 — frozen seam; 414 supplies the
    /// real one). `#[cfg(unix)]` (over the agent supervisor, like
    /// `suggestions`). `None` in 401.5; the `Maestro` service is served but
    /// returns `Status::unimplemented`.
    #[cfg(unix)]
    maestro: Option<crate::maestro::MaestroHandle>,
    /// Optional VCS Provider handle. When `Some`, the gRPC `Vcs`
    /// service is registered (Task 45). The handle is cheap to clone
    /// and lazily resolves the `gh` binary path on first use.
    vcs: Option<VcsHandle>,
    /// Optional pairing coordinator (Task 207). When `Some`, the gRPC
    /// `Devices` service is registered (the two pairing RPCs). Built once at
    /// boot from the Core's keychain-backed identity + issuer; the token store
    /// it owns lives behind the `Arc` so a restart of this actor preserves
    /// any in-flight tokens. `None` on a Core that could not establish its
    /// identity (no keychain), which leaves remote pairing unavailable.
    pairing: Option<Arc<crate::security::pairing::PairingCoordinator>>,
    /// Optional device manager (Task 209). Constructed alongside `pairing` at
    /// boot (both need the Core identity); shares the revoked-set handle the
    /// issuer reads and the `SessionCloser` seam. Threaded into the same
    /// `Devices` service so its `ListDevices`/`RevokeDevice`/`GetCoreInfo` RPCs
    /// are served next to the pairing RPCs. `None` whenever `pairing` is `None`.
    device_manager: Option<Arc<crate::security::devices::DeviceManager>>,
    /// Optional device-cert issuer for the Task-210 auth middleware. `Some`
    /// whenever the Core established a keychain-backed identity at boot (the
    /// same condition as `pairing`/`device_manager`). The cert-validation path
    /// of the auth interceptor validates inbound `concerto-device-cert` headers
    /// against it; `None` leaves the cert path refusing every remote connection
    /// (`auth.invalid_cert`) while the UDS peer-uid fast path still works
    /// (kernel attestation needs no issuer).
    auth_issuer: Option<Arc<dyn concerto_identity::DeviceCertIssuer>>,
    /// Optional `enterprise_data_privacy` resolver the `Vcs.FetchIssueByUrl` D10
    /// path consults (Task 411). Built at boot from the managed policy +
    /// persistence + opt-out config and attached to the `VcsHandler` (and the
    /// bridge's) via `with_privacy_resolver`. `None` ⇒ the pre-resolver default
    /// (`false`), preserving prior behavior on a Core that does not wire it.
    vcs_privacy_resolver: Option<Arc<dyn crate::handlers::vcs::EnterprisePrivacyResolver>>,
    /// Optional Core identity public key (Task 521). When `Some` AND
    /// `CONCERTO_CONNECT_BRIDGE_TLS` is set, the Connect-Web bridge derives an
    /// identity-bound self-signed TLS cert from it and serves LAN-direct TLS,
    /// publishing the SPKI fingerprint for client pinning (`design/17 §3.3`).
    /// `None` (or TLS not requested) ⇒ the bridge serves plain HTTP (loopback
    /// default). Set at boot via [`ApiServerActor::with_core_pubkey`] from the
    /// same keychain-loaded identity the issuer uses.
    core_pubkey: Option<concerto_identity::PublicKey>,
    /// The SHARED notifications handle (shared-event-channel fix). Built ONCE in
    /// boot via `with_event_channel` and attached via
    /// [`ApiServerActor::with_notif_handle`]; the actor clones it into BOTH the
    /// UDS `run_uds` path AND the Connect-Web bridge's `BridgeServices`, and boot
    /// clones the same handle into the Iroh `CoreServiceSet` + the live
    /// `notify_user` sink. So every front door shares ONE `notification.events`
    /// broadcast and cross-device read/created/updated/acted sync works
    /// (design/14 R-8, §5.3). `None` ⇒ each path falls back to its own
    /// `with_event_channel` handle (prior behavior; tests + runtime-only).
    notif_handle: Option<crate::notifications::handle::NotificationHandle>,
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
            #[cfg(unix)]
            maestro: None,
            vcs: None,
            pairing: None,
            device_manager: None,
            auth_issuer: None,
            vcs_privacy_resolver: None,
            core_pubkey: None,
            notif_handle: None,
        }
    }

    /// Attach the `enterprise_data_privacy` resolver the `Vcs.FetchIssueByUrl`
    /// D10 fix consults (Task 411). Additive builder; `boot.rs` chains it after
    /// `with_managers`. Returns `self` for chaining.
    pub fn with_vcs_privacy_resolver(
        mut self,
        resolver: Arc<dyn crate::handlers::vcs::EnterprisePrivacyResolver>,
    ) -> Self {
        self.vcs_privacy_resolver = Some(resolver);
        self
    }

    /// Attach the Core identity public key so the Connect-Web bridge can derive
    /// an identity-bound LAN-direct TLS cert (Task 521) when
    /// `CONCERTO_CONNECT_BRIDGE_TLS` is set. Additive builder; `boot.rs` chains
    /// it from the same keychain-loaded identity the issuer is built from.
    /// Without it, a TLS-requested bridge logs a warning and falls back to plain
    /// HTTP (it never serves a non-identity-bound cert).
    pub fn with_core_pubkey(mut self, core_pubkey: concerto_identity::PublicKey) -> Self {
        self.core_pubkey = Some(core_pubkey);
        self
    }

    /// Attach the SHARED notifications handle (shared-event-channel fix). Built
    /// ONCE in boot via `with_event_channel`; the actor clones it into BOTH the
    /// UDS `run_uds` path and the Connect-Web bridge's `BridgeServices` so both
    /// front doors publish onto / subscribe from the SAME `notification.events`
    /// broadcast (the same one boot also threads into the Iroh `CoreServiceSet` +
    /// the live `notify_user` sink). Additive builder; without it each path
    /// keeps its prior per-transport `with_event_channel` handle.
    pub fn with_notif_handle(
        mut self,
        handle: crate::notifications::handle::NotificationHandle,
    ) -> Self {
        self.notif_handle = Some(handle);
        self
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
            #[cfg(unix)]
            maestro: None,
            vcs: None,
            pairing: None,
            device_manager: None,
            auth_issuer: None,
            vcs_privacy_resolver: None,
            core_pubkey: None,
            notif_handle: None,
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
        #[cfg(unix)] maestro: Option<crate::maestro::MaestroHandle>,
        vcs: Option<VcsHandle>,
        pairing: Option<Arc<crate::security::pairing::PairingCoordinator>>,
        device_manager: Option<Arc<crate::security::devices::DeviceManager>>,
        auth_issuer: Option<Arc<dyn concerto_identity::DeviceCertIssuer>>,
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
            #[cfg(unix)]
            maestro,
            vcs,
            pairing,
            device_manager,
            auth_issuer,
            vcs_privacy_resolver: None,
            core_pubkey: None,
            notif_handle: None,
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

            // Task 204: the Connect-Web bridge is a second front door onto
            // the *same* handler set — a loopback gRPC-Web server, opt-in
            // via `CONCERTO_CONNECT_BRIDGE` (default OFF so a pure
            // co-located install never opens a TCP port). Build its
            // `BridgeServices` from clones of the same handles before
            // `run_uds` consumes the originals, then run both serve loops
            // concurrently under the one shutdown token.
            let mut bridge_cfg = crate::connect_bridge::ConnectBridgeConfig::from_env()?;
            // Task 521: when LAN-direct TLS is requested AND the Core identity is
            // available, derive the identity-bound cert now (so the published
            // fingerprint is known before any client connects). If TLS is
            // requested but the identity is absent (a Core that could not reach
            // its keychain), warn and fall back to plain HTTP — we never serve a
            // non-identity-bound cert.
            if bridge_cfg.enabled && bridge_cfg.tls_requested {
                match &self.core_pubkey {
                    Some(pk) => {
                        bridge_cfg = bridge_cfg.with_tls_for(pk)?;
                    }
                    None => {
                        tracing::warn!(
                            "CONCERTO_CONNECT_BRIDGE_TLS is set but the Core identity is \
                             unavailable; serving the Connect-Web bridge over plain HTTP. \
                             LAN-direct TLS needs a keychain-backed Core identity (Task 521)."
                        );
                    }
                }
            }
            let bridge = if bridge_cfg.enabled {
                let services = crate::connect_bridge::BridgeServices {
                    started_at: Arc::clone(&self.started_at),
                    supervisor_view: self.supervisor_view.clone(),
                    repo_manager: self.repo_manager.clone(),
                    workspace_manager: self.workspace_manager.clone(),
                    workarea_manager: self.workarea_manager.clone(),
                    agent_supervisor: self.agent_supervisor.clone(),
                    persistence: self.persistence.clone(),
                    scheduler: self.scheduler.clone(),
                    skills_registry: self.skills_registry.clone(),
                    suggestions: self.suggestions.clone(),
                    maestro: self.maestro.clone(),
                    vcs: self.vcs.clone(),
                    vcs_privacy_resolver: self.vcs_privacy_resolver.clone(),
                    // Shared-event-channel fix: the bridge shares the SAME
                    // notifications handle (and thus the SAME `notification.events`
                    // broadcast) as the UDS path below + the Iroh path + the live
                    // `notify_user` sink, so cross-device read/created/updated/acted
                    // sync works over the web front door too (design/14 R-8, §5.3).
                    notif_handle: self.notif_handle.clone(),
                };
                let tls = bridge_cfg.tls.clone();
                let (listener, bound) = crate::connect_bridge::bind(&bridge_cfg).await?;
                tracing::info!(
                    addr = %bound.local_addr,
                    tls = bound.cert_fingerprint.is_some(),
                    cert_fingerprint = bound.cert_fingerprint.as_deref().unwrap_or("(plain http)"),
                    "Connect-Web bridge enabled alongside UDS server"
                );
                Some((listener, services, tls))
            } else {
                None
            };

            let uds_fut = run_uds(
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
                self.maestro,
                self.vcs,
                self.pairing,
                self.device_manager,
                self.auth_issuer,
                self.vcs_privacy_resolver,
                self.notif_handle,
                ctx.shutdown.clone(),
            );

            match bridge {
                Some((listener, services, tls)) => {
                    let bridge_fut =
                        crate::connect_bridge::serve(listener, services, tls, ctx.shutdown.clone());
                    // Both serve loops resolve on the shared shutdown token;
                    // surface the first error from either front door.
                    let (uds_res, bridge_res) = tokio::join!(uds_fut, bridge_fut);
                    uds_res?;
                    bridge_res?;
                    Ok(())
                }
                None => uds_fut.await,
            }
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
                self.pairing,
                self.device_manager,
                self.auth_issuer,
                self.vcs_privacy_resolver,
                self.core_pubkey,
                self.notif_handle,
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
    maestro: Option<crate::maestro::MaestroHandle>,
    vcs: Option<VcsHandle>,
    pairing: Option<Arc<crate::security::pairing::PairingCoordinator>>,
    device_manager: Option<Arc<crate::security::devices::DeviceManager>>,
    auth_issuer: Option<Arc<dyn concerto_identity::DeviceCertIssuer>>,
    vcs_privacy_resolver: Option<Arc<dyn crate::handlers::vcs::EnterprisePrivacyResolver>>,
    notif_handle: Option<crate::notifications::handle::NotificationHandle>,
    shutdown: tokio_util::sync::CancellationToken,
) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    use tokio::net::UnixListener;
    use tokio_stream::wrappers::UnixListenerStream;
    use tonic::transport::Server;

    use concerto_proto::v1::TransportKind;

    use crate::conn_transport::ConnTransport;

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

    // Tag every request that arrives on this UDS listener with
    // `ConnTransport(Uds)` so `RuntimeHandler::get_server_capabilities`
    // reports the live transport kind (Task 201), THEN run the Task-210 auth
    // middleware. This is the seam every listener writes: Task 212's Iroh
    // listener (`serve_iroh` below) applies the same interceptor with
    // `TransportKind::Iroh`, and Task 204's WSS bridge with `WssBridge` — none
    // of them edit the handler. The interceptor layer is applied to the whole
    // server (before `add_service`) so the tag + auth are present on every
    // service, not just `Runtime`. On Windows the co-located named-pipe listener
    // maps to `Uds` too (see `crate::conn_transport`).
    //
    // The Task-210 auth interceptor reads the tag (`Uds` → kernel-attested
    // peer-uid fast path producing the local-uds pseudo-cert `DeviceContext`;
    // `Iroh`/`WssBridge` → validate the `concerto-device-cert` header against
    // the boot issuer). Tagging happens FIRST so the auth step sees `Uds` and
    // takes the peer-uid branch.
    let auth = crate::security::auth::AuthInterceptor::new(auth_issuer.clone());
    // The interceptor returns `Result<_, tonic::Status>` — `Status` is large, so
    // clippy's `result_large_err` fires; this is the fixed shape the tonic
    // `Interceptor` trait requires (the prior `tag_uds` carried the same allow).
    #[allow(clippy::result_large_err)]
    let auth_interceptor =
        move |mut req: tonic::Request<()>| -> std::result::Result<tonic::Request<()>, tonic::Status> {
            req.extensions_mut()
                .insert(ConnTransport(TransportKind::Uds));
            auth.authenticate(req)
        };

    // The UDS path and the Iroh path (`serve_iroh`) register the IDENTICAL
    // service set via `add_core_services`; the ONLY difference is this
    // interceptor's injected `TransportKind`. See `CoreServiceSet`.
    let services = CoreServiceSet {
        started_at,
        supervisor_view,
        repo_manager,
        workspace_manager,
        workarea_manager,
        agent_supervisor,
        persistence,
        scheduler,
        skills_registry,
        suggestions,
        maestro,
        vcs,
        vcs_privacy_resolver,
        pairing,
        device_manager,
        auth_issuer,
        // The UDS path has no remote transport: keep the handler's `NoNatStats`
        // default (empty counters). The Iroh serve path supplies `Some(..)`.
        nat_stats: None,
        // The SHARED notifications handle threaded from boot (same broadcast as
        // the Iroh + bridge + `notify_user` sink).
        notif_handle,
    };
    let builder = add_core_services(
        Server::builder().layer(tonic::service::interceptor(auth_interceptor)),
        services,
    )?;

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

/// The full set of subsystem handles every transport's gRPC server registers,
/// bundled so the **identical** `add_service(..)` chain is shared by both the
/// UDS path (`run_uds`) and the Iroh path (`serve_iroh`) without duplicating the
/// per-service registration logic (Task 212). Mirrors `connect_bridge`'s
/// `BridgeServices`, extended with the `Devices`-backing handles + the auth
/// issuer the Iroh path also needs.
///
/// Every field is `Clone`-able (managers are `Clone`; the rest are `Arc`/option
/// of `Arc`), so the Iroh dispatcher can build a fresh `Router` per accepted
/// connection from a cloned set — one gRPC connection == one Iroh bidi stream.
/// The `#[cfg(unix)]` handles match `ApiServerActor`'s own gating; on Windows the
/// Iroh server simply serves the cross-platform subset, exactly as the bridge
/// does.
#[derive(Clone)]
pub struct CoreServiceSet {
    pub started_at: Arc<SystemTime>,
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
    /// The live Maestro handle (Task 401.5 — frozen seam; 414 threads the real
    /// one). `#[cfg(unix)]` because `MaestroHandle` lives in the `#[cfg(unix)]`
    /// `maestro` module (over the agent supervisor, like `suggestions`).
    /// `None` everywhere in 401.5 ⇒ the `Maestro` service is registered but
    /// returns `Status::unimplemented`.
    #[cfg(unix)]
    pub maestro: Option<crate::maestro::MaestroHandle>,
    pub vcs: Option<VcsHandle>,
    /// The `enterprise_data_privacy` resolver the `Vcs.FetchIssueByUrl` D10 path
    /// consults (Task 411). `None` ⇒ the pre-resolver default (`false`).
    /// Threaded alongside the `vcs` handle (additive; distinct field).
    pub vcs_privacy_resolver: Option<Arc<dyn crate::handlers::vcs::EnterprisePrivacyResolver>>,
    pub pairing: Option<Arc<crate::security::pairing::PairingCoordinator>>,
    pub device_manager: Option<Arc<crate::security::devices::DeviceManager>>,
    pub auth_issuer: Option<Arc<dyn concerto_identity::DeviceCertIssuer>>,
    /// The live NAT-telemetry source the `Runtime.GetNatStats` RPC reads (Task
    /// 216/217.5). `Some(IrohNatStatsSource(transport))` on the Iroh serve path
    /// so the booted transport's real counters surface; `None` on the UDS path
    /// (no remote transport), where the handler keeps `NoNatStats` (empty).
    pub nat_stats: Option<Arc<dyn crate::handlers::runtime::NatStatsSource>>,
    /// The SHARED notifications handle — constructed ONCE in boot (via
    /// `with_event_channel`) and cloned into every front door (UDS + Iroh + the
    /// Connect-Web bridge + the live `notify_user` sink) so they all publish onto
    /// and subscribe from the SAME `notification.events` broadcast. Cloning a
    /// `NotificationHandle` preserves the same `events_tx` sender + event sink, so
    /// a `notification.read`/`created`/`updated`/`acted` emitted by any transport
    /// reaches `notification.events` subscribers on EVERY transport (design/14
    /// R-8, §5.3). `None` ⇒ no `Notifications` service (the runtime-only / no-
    /// persistence case). Built from the shared `Persistence`; the prior per-
    /// transport `with_event_channel` construction fragmented the bus.
    pub notif_handle: Option<crate::notifications::handle::NotificationHandle>,
}

impl CoreServiceSet {
    /// A minimal service set exposing **only** the `Runtime` service — used by
    /// the Task-212 Iroh-transport end-to-end test/smoke (which asserts
    /// `GetServerCapabilities.transport_kind == IROH` over a real `serve_iroh`)
    /// and by a stripped-down `serve_iroh` caller. Every optional subsystem is
    /// `None`; `auth_issuer` is `None` so the cert path admits the loopback
    /// double's injected metadata path. Task 217's façade builds the full set
    /// from boot handles instead.
    pub fn runtime_only(started_at: Arc<SystemTime>, supervisor_view: SupervisorView) -> Self {
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
            #[cfg(unix)]
            maestro: None,
            vcs: None,
            vcs_privacy_resolver: None,
            pairing: None,
            device_manager: None,
            auth_issuer: None,
            nat_stats: None,
            notif_handle: None,
        }
    }
}

/// Apply the **shared** `add_service(..)` chain — the single source of truth for
/// which gRPC services every transport exposes — onto an interceptor-layered
/// `Server` builder, returning the configured `Router`. The ONLY per-transport
/// difference is the interceptor `L` (which injects the transport's
/// `ConnTransport` tag) and the incoming stream the caller serves the `Router`
/// over; the handler set is identical (Task 212, `design/10 §3.4` "the schema
/// does not branch by transport").
fn add_core_services<L>(
    mut server: tonic::transport::server::Server<L>,
    services: CoreServiceSet,
) -> Result<tonic::transport::server::Router<L>>
where
    L: Clone,
{
    use concerto_proto::v1::devices_server::DevicesServer;
    use concerto_proto::v1::files_server::FilesServer;
    use concerto_proto::v1::repositories_server::RepositoriesServer;
    use concerto_proto::v1::runtime_server::RuntimeServer;
    use concerto_proto::v1::skills_server::SkillsServer;
    use concerto_proto::v1::vcs_server::VcsServer;
    use concerto_proto::v1::workareas_server::WorkareasServer;
    use concerto_proto::v1::workspaces_server::WorkspacesServer;

    use crate::handlers::devices::DevicesHandler;
    use crate::handlers::files::FilesHandler;
    use crate::handlers::repositories::RepositoriesHandler;
    use crate::handlers::runtime::RuntimeHandler;
    use crate::handlers::skills::SkillsHandler;
    use crate::handlers::vcs::VcsHandler;
    use crate::handlers::workareas::WorkareasHandler;
    use crate::handlers::workspaces::WorkspacesHandler;

    let CoreServiceSet {
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
        pairing,
        device_manager,
        auth_issuer: _auth_issuer,
        nat_stats,
        notif_handle: shared_notif_handle,
    } = services;

    // Task 507 + shared-event-channel fix: the notifications handle. BOTH
    // transports (UDS + Iroh) run this single chain, and boot now constructs ONE
    // `with_event_channel`-backed handle and threads a clone into the
    // `CoreServiceSet` (`notif_handle`) so the `Notifications` service + the
    // `notification.events` producer on every transport share ONE broadcast — a
    // `notification.read`/`created`/`updated`/`acted` emitted on one transport
    // (or by the live `notify_user` sink) reaches subscribers on EVERY transport
    // (design/14 R-8, §5.3). When no shared handle is supplied (the runtime-only
    // smoke path / tests), fall back to the prior per-call construction from the
    // shared `Persistence`. `None` persistence + no shared handle ⇒ no service.
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

    // Build the Runtime handler, attaching the live transport-backed NAT-stats
    // source only when present (the Iroh serve path). With `None` (the UDS path)
    // the handler keeps its `NoNatStats` default — `GetNatStats` stays answerable
    // with empty counters and UDS behavior is unchanged (Task 216/217.5).
    let runtime_handler = RuntimeHandler::new(started_at, supervisor_view);
    let runtime_handler = match nat_stats {
        Some(source) => runtime_handler.with_nat_stats(source),
        None => runtime_handler,
    };
    let runtime_service = RuntimeServer::new(runtime_handler);
    let mut builder = server.add_service(runtime_service);

    if let Some(persistence) = persistence {
        let home = home::home_dir().ok_or_else(|| {
            Error::Internal("home::home_dir() returned None; cannot scope Files allow-list".into())
        })?;
        let files_service = FilesServer::new(FilesHandler::new(persistence, home));
        builder = builder.add_service(files_service);
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
    // Task 507: the `Notifications` service (cross-platform — `NotificationHandle`
    // needs only persist + push). The `notification.events` producer is wired
    // into the `Streams` handler in the `#[cfg(unix)]` block below.
    if let Some(h) = notif_handle.as_ref() {
        use concerto_proto::v1::notifications_server::NotificationsServer;

        use crate::handlers::notifications::NotificationsHandler;
        let notifications_service = NotificationsServer::new(NotificationsHandler::new(h.clone()));
        builder = builder.add_service(notifications_service);
    }
    // `Sessions` + `Streams` need the `#[cfg(unix)]` agent supervisor; on a
    // non-unix target these handles don't exist, so the services are simply
    // absent (the Iroh server serves the cross-platform subset).
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
        if let (Some(supervisor), Some(workspace_mgr), Some(workarea_mgr)) = (
            agent_supervisor.clone(),
            workspace_manager,
            workarea_manager,
        ) {
            let mut handler = StreamsHandler::new(supervisor, workspace_mgr, workarea_mgr);
            if let Some(suggestions) = suggestions.clone() {
                handler = handler.with_suggestions(suggestions);
            }
            // Task 316: wire the VCS aggregator's `checks.<wa>.<repo>` event
            // broadcast as the producer for the `checks.*` subject.
            if let Some(vcs) = vcs.as_ref() {
                handler = handler.with_vcs_events(vcs.checks_sender());
            }
            // Task 414: wire the live Maestro's `maestro.events` producer so the
            // `maestro.events` subject streams real events. `None` (Maestro
            // disabled) leaves the subject valid-but-empty.
            if let Some(maestro) = maestro.as_ref() {
                handler = handler.with_maestro_events(maestro.events_sender());
            }
            // Task 507: wire this transport's notifications handle as the
            // `notification.events` producer so the subject streams live
            // created/updated/read/acted events.
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
        // Task 401.5 (D8 site 1 of 2): the `Maestro` service is ALWAYS
        // registered (so 415 can dial it), with a possibly-`None` handle. The
        // handler returns `Status::unimplemented` until Task 414 threads a real
        // `MaestroHandle` here (the `maestro: None` → `Some(handle)` flip). The
        // second site is `connect_bridge::build_and_serve`.
        let maestro_service = MaestroServer::new(MaestroHandler::new(maestro));
        builder = builder.add_service(maestro_service);
    }
    if let Some(skills_registry) = skills_registry {
        let skills_service = SkillsServer::new(SkillsHandler::new(skills_registry));
        builder = builder.add_service(skills_service);
    }
    if let Some(vcs) = vcs {
        let mut vcs_handler = VcsHandler::new(vcs);
        // Task 316: the "Send to agent" sink needs the `#[cfg(unix)]` agent
        // supervisor. On a non-unix Core the sink stays `None` and
        // `SendThreadToAgent` returns `UNIMPLEMENTED`.
        #[cfg(unix)]
        if let Some(supervisor) = agent_supervisor.clone() {
            use crate::handlers::vcs::AgentSupervisorSink;
            vcs_handler = vcs_handler
                .with_session_sink(std::sync::Arc::new(AgentSupervisorSink::new(supervisor)));
        }
        // Task 411 (D10): attach the `enterprise_data_privacy` resolver so
        // `FetchIssueByUrl` enforces the per-workspace / managed-floor privacy
        // gate instead of the hardcoded `false`.
        if let Some(resolver) = vcs_privacy_resolver {
            vcs_handler = vcs_handler.with_privacy_resolver(resolver);
        }
        let vcs_service = VcsServer::new(vcs_handler);
        builder = builder.add_service(vcs_service);
    }
    if let (Some(pairing), Some(device_manager)) = (pairing, device_manager) {
        let devices_service = DevicesServer::new(DevicesHandler::new(pairing, device_manager));
        builder = builder.add_service(devices_service);
    }

    Ok(builder)
}

/// The Iroh-transport gRPC dispatcher (Task 212). Holds a [`CoreServiceSet`] and
/// the boot auth issuer; for every accepted Iroh API stream the transport's
/// serve loop hands it a Noise-wrapped duplex, over which it builds the **same**
/// `add_core_services` router as the UDS path — the only difference being the
/// interceptor injects `ConnTransport(TransportKind::Iroh)` so the 210 auth path
/// validates the `concerto-device-cert` header and 201 caps report `IROH`, with
/// **no per-transport handler branching**.
///
/// One `serve_connection` call == one gRPC connection == one Iroh bidi stream
/// (the spike's gotcha #2). The 64 MiB message limits ride the `add_service`
/// registrations + the transport's adapter limits.
struct IrohDispatcher {
    services: CoreServiceSet,
}

impl concerto_transport::ApiDispatcher for IrohDispatcher {
    fn serve_connection(
        &self,
        io: concerto_transport::NoiseDuplex,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = std::result::Result<(), concerto_transport::TransportError>,
                > + Send,
        >,
    > {
        // No observer: keep the connection on its accept-time endpoint-id key.
        // Used by callers that do not need the fingerprint binding; the real
        // serve loop drives `serve_connection_observed` (below) instead.
        self.serve_with_observer(io, None)
    }

    /// Serve one Iroh gRPC connection AND report the validated device
    /// **fingerprint** into `observer` on the first authenticated request, so the
    /// transport's serve loop re-keys the naturally-accepted session from its
    /// accept-time endpoint-id key onto the fingerprint (Task 217.5). This is the
    /// seam that closes the deferred fingerprint↔session binding: `RevokeDevice`
    /// keys `close_sessions_for_device` on the fingerprint, so without this the
    /// revoked device's live session would not be severed.
    fn serve_connection_observed(
        &self,
        io: concerto_transport::NoiseDuplex,
        observer: concerto_transport::AuthObserver,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = std::result::Result<(), concerto_transport::TransportError>,
                > + Send,
        >,
    > {
        self.serve_with_observer(io, Some(observer))
    }
}

impl IrohDispatcher {
    /// Shared body of [`ApiDispatcher::serve_connection`] /
    /// [`ApiDispatcher::serve_connection_observed`]: build the SAME
    /// `add_core_services` router the UDS path uses, with the `Iroh`-tagging auth
    /// interceptor. When `observer` is `Some`, the interceptor additionally
    /// reports the validated device fingerprint into it (fire-once) so the serve
    /// loop binds the session to the fingerprint (Task 217.5).
    fn serve_with_observer(
        &self,
        io: concerto_transport::NoiseDuplex,
        observer: Option<concerto_transport::AuthObserver>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = std::result::Result<(), concerto_transport::TransportError>,
                > + Send,
        >,
    > {
        // Clone the service set so each connection gets a fresh router.
        let services = self.services.clone();
        Box::pin(async move {
            use crate::conn_transport::ConnTransport;
            use concerto_proto::v1::TransportKind;
            use tonic::transport::Server;

            // Tag `Iroh` then run the SAME auth interceptor the UDS path uses —
            // the auth step takes the cert-validation branch for `Iroh`.
            let auth = crate::security::auth::AuthInterceptor::new(services.auth_issuer.clone());
            #[allow(clippy::result_large_err)]
            let interceptor = move |mut req: tonic::Request<()>| -> std::result::Result<
                tonic::Request<()>,
                tonic::Status,
            > {
                req.extensions_mut()
                    .insert(ConnTransport(TransportKind::Iroh));
                let req = auth.authenticate(req)?;
                // The cert path injected the authenticated `DeviceContext`
                // (`device_id` IS the cert fingerprint). Report it to the
                // per-connection observer so the serve loop re-keys this
                // naturally-accepted session onto the fingerprint (Task 217.5).
                // Fire-once is enforced inside `AuthObserver`, so doing this on
                // every request is cheap and idempotent.
                if let Some(observer) = observer.as_ref() {
                    if let Some(ctx) = crate::security::auth::device_context(&req) {
                        observer.observe(concerto_transport::DeviceId::from(ctx.device_id));
                    }
                }
                Ok(req)
            };

            let builder = add_core_services(
                Server::builder().layer(tonic::service::interceptor(interceptor)),
                services,
            )
            .map_err(|e| {
                concerto_transport::TransportError::Adapter(format!(
                    "building iroh gRPC router: {e}"
                ))
            })?;

            // One gRPC connection over the single Noise-wrapped duplex (one Iroh
            // bidi stream). `serve_with_incoming` over a single-element stream is
            // the "QUIC stream pool for gRPC" shape (`design/11 §3.3`).
            let incoming = futures::stream::once(async move { Ok::<_, std::io::Error>(io) });
            builder.serve_with_incoming(incoming).await.map_err(|e| {
                concerto_transport::TransportError::Adapter(format!(
                    "iroh serve_with_incoming: {e}"
                ))
            })
        })
    }
}

/// Serve the Core's gRPC services over an Iroh transport (Task 212), the Iroh
/// twin of [`run_uds`]. Builds the [`IrohDispatcher`] from the same handle set
/// and drives the transport's accept/serve loop until the transport is stopped
/// (which the caller wires to `ctx.shutdown.cancelled()` exactly like the UDS
/// path). The transport is pre-`start`ed by the caller (it owns the Iroh
/// endpoint + the Core's Noise static); this function only attaches the shared
/// dispatcher and runs the loop.
///
/// Cross-platform: Iroh is QUIC, so unlike `run_uds` this is **not**
/// `#[cfg(unix)]`-gated and builds on the Windows CI lane (Task 113). The actor
/// wiring that constructs the `IrohTransport` from boot config + spawns this is
/// deferred to Task 217's `TransportHandle` (the façade); this function is the
/// internal entry that façade drives, and the Tier-2 loopback double drives the
/// transport + an equivalent dispatcher directly without a keychain-touching
/// boot.
pub async fn serve_iroh(
    transport: std::sync::Arc<concerto_transport::IrohTransport>,
    services: CoreServiceSet,
) -> Result<()> {
    let dispatcher = std::sync::Arc::new(IrohDispatcher { services });
    transport
        .serve(dispatcher)
        .await
        .map_err(|e| Error::Internal(format!("iroh transport serve loop: {e}")))
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
