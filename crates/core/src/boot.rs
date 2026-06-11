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

/// The **FROZEN** opt-in environment toggle that enables the Iroh transport
/// listener at boot (Task 217.5). **Default OFF** in V1.0: when unset (or not a
/// truthy value) a booted Core listens only on its UDS, byte-identical to the
/// pre-217.5 behaviour, so the existing smoke suite is unchanged. Task 220's
/// split-host smoke flips this on. Operators script against this exact spelling.
///
/// Truthy values (case-insensitive): `1`, `true`, `yes`, `on`. Anything else
/// (including unset) leaves the listener off.
pub const ENABLE_IROH_ENV: &str = "CONCERTO_ENABLE_IROH";

/// Whether the Iroh listener is enabled via [`ENABLE_IROH_ENV`].
fn iroh_listener_enabled() -> bool {
    match std::env::var(ENABLE_IROH_ENV) {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            matches!(v.as_str(), "1" | "true" | "yes" | "on")
        }
        Err(_) => false,
    }
}

/// Classify a managed-policy `default_model` as **external** (off-box public
/// provider) for the Maestro D1 privacy gate (`design/08 §3.10`). Conservative:
/// an empty/unset model is treated as local (the CLI default, which passes the
/// gate). A model whose name signals the public Anthropic/OpenAI/Google APIs is
/// external; the on-prem markers (Bedrock-VPC / Vertex / Azure-Foundry / local)
/// are NOT. This is the V1.0 heuristic over the parsed-but-otherwise-unread
/// `default_model`; the richer on-prem locality classification (412's
/// `MaestroProvider`) supersedes it when it lands.
#[cfg(unix)]
fn is_external_maestro_model(model: &str) -> bool {
    let m = model.trim().to_ascii_lowercase();
    if m.is_empty() {
        return false;
    }
    // On-prem / local markers re-enable the Maestro under enterprise privacy.
    const ONPREM_MARKERS: &[&str] = &[
        "bedrock", "vpc", "vertex", "azure", "foundry", "local", "onprem", "on-prem", "ollama",
    ];
    if ONPREM_MARKERS.iter().any(|marker| m.contains(marker)) {
        return false;
    }
    // Public-provider markers ⇒ external (the disabled case under privacy).
    const EXTERNAL_MARKERS: &[&str] =
        &["claude", "gpt", "openai", "anthropic", "gemini", "o1", "o3"];
    EXTERNAL_MARKERS.iter().any(|marker| m.contains(marker))
}

/// The live Iroh-transport seam a booted Core exposes (Task 217.5). Held by
/// [`RunningCore`] when the listener is enabled so the split-host smoke driver
/// (Task 220) + the Tier-2 loopback test can dial the endpoint, drive a pairing,
/// and observe revoke teardown. `None` when the Iroh listener is off (the
/// default).
pub struct IrohRuntime {
    /// The live transport (the endpoint + session registry). Clients dial
    /// [`IrohTransport::endpoint_id`]; the Core's Noise responder static is
    /// [`IrohTransport::core_noise_public`].
    pub transport: Arc<concerto_transport::IrohTransport>,
    /// The Core-side Noise-XX pairing responder over the `0x03` channel.
    pub pairing_responder: Arc<crate::security::iroh_pairing::IrohPairingResponder>,
}

/// The live [`SessionCloser`](crate::security::devices::SessionCloser) backed by
/// the Iroh transport (Task 217.5) — replaces `NoopSessionCloser` so
/// `DeviceManager::revoke_device` actually severs the revoked device's open Iroh
/// sessions (the 209/210/212 revoke→teardown contract, `design/12 §7.3`).
///
/// 209's FROZEN `SessionCloser::close_sessions_for_device` takes the raw 32-byte
/// cert fingerprint; we feed it through the transport's FROZEN
/// `From<[u8; 32]>` → [`concerto_transport::DeviceId`] (lowercase-hex of the
/// fingerprint) and call [`IrohTransport::close_sessions_for_device`]. Sync +
/// non-blocking, as the trait requires.
struct IrohSessionCloser {
    transport: Arc<concerto_transport::IrohTransport>,
}

impl crate::security::devices::SessionCloser for IrohSessionCloser {
    fn close_sessions_for_device(&self, device_id: [u8; 32]) {
        let id = concerto_transport::DeviceId::from(device_id);
        self.transport.close_sessions_for_device(&id);
    }
}

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
    /// The live Iroh-transport seam (Task 217.5), `Some` only when the
    /// [`ENABLE_IROH_ENV`] opt-in is set at boot. Lets the split-host smoke +
    /// the Tier-2 loopback test dial the endpoint and drive pairing/revoke.
    iroh: Option<IrohRuntime>,
}

impl RunningCore {
    /// The UDS path the gRPC server bound. Clients dial this.
    pub fn socket_path(&self) -> &std::path::Path {
        &self.socket_path
    }

    /// The live Iroh-transport seam, present only when the Iroh listener was
    /// enabled at boot ([`ENABLE_IROH_ENV`]). Clients discover the dialable
    /// endpoint id via [`concerto_transport::IrohTransport::endpoint_id`] and the
    /// Core's Noise responder static via
    /// [`concerto_transport::IrohTransport::core_noise_public`].
    pub fn iroh(&self) -> Option<&IrohRuntime> {
        self.iroh.as_ref()
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

/// Best-effort keychain timeout for the Core Noise static load (Task 206
/// pattern): a keychain that blocks (e.g. a headless macOS runner with no GUI to
/// answer a Keychain Access prompt) must not hang boot — we bound the access and
/// degrade to "Iroh off" on timeout.
const NOISE_STATIC_KEYCHAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Build + start the live [`concerto_transport::IrohTransport`] (Task 217.5).
/// Loads/creates the Core's persistent X25519 Noise static from the keychain
/// (timeout-bounded), then binds the endpoint per `remote_disabled` (LAN-only
/// when set). The serve loop is **not** started here — the caller spawns
/// `serve_iroh` over the returned transport.
async fn build_iroh_transport(
    secrets: &concerto_keychain::Secrets,
    remote_disabled: bool,
) -> Result<Arc<concerto_transport::IrohTransport>> {
    let noise_private = match tokio::time::timeout(
        NOISE_STATIC_KEYCHAIN_TIMEOUT,
        crate::security::identity::load_or_create_core_noise_static(secrets),
    )
    .await
    {
        Ok(res) => res?,
        Err(_) => {
            return Err(concerto_error::Error::Internal(
                "core noise static keychain access timed out".into(),
            ))
        }
    };

    let config = concerto_transport::TransportConfig {
        relay_url: None,
        disable_remote: remote_disabled,
        direct_addr: None,
    };
    let transport = concerto_transport::IrohTransport::start(config, noise_private)
        .await
        .map_err(|e| concerto_error::Error::Internal(format!("iroh transport start: {e}")))?;
    Ok(Arc::new(transport))
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

    // Task 302: attach the audit writer to the Repo Manager handle so the
    // §8 force-non-cone-to-cone path can emit a typed audit event. The
    // handle is rebound here (the audit writer only exists now); every
    // downstream clone of `repo_handle` (the gRPC service, the workarea
    // manager) picks up the audited handle.
    let repo_handle = repo_handle.with_audit(audit_writer.clone());

    // Task 304: inject the idle-prewarm scheduler's idle/power/net signal
    // bundle. `host_signals()` is the best-effort, macOS-first
    // implementation; it is deliberately the conservative "never prewarm"
    // bundle for V1.0 because the real idle source is the Local-API client
    // heartbeat (`design/02 §6.3`), which is a small documented follow-on.
    // The seam is injected HERE so the follow-on only swaps `host_signals()`
    // for a heartbeat-backed `IdleSignal` (and a real macOS power/net probe)
    // — the scheduler, the eager triggers, and the `PrewarmBlobs` RPC all
    // ship fully and are CI-proven against deterministic mocks. With the
    // current bundle the background scheduler stays inert; the eager
    // worktree-create + HEAD-update triggers do NOT depend on these signals
    // and fire unconditionally for blobless repos.
    let repo_handle =
        repo_handle.with_prewarm_signals(crate::repo_manager::prefetch::signals::host_signals());

    // Task 206: establish the Core's Ed25519 identity (`design/12 §3.1`).
    // Runs AFTER the audit writer (so a first-launch generation can emit
    // `CoreIdentityCreated`) and is constructed here so the issuer's signing
    // key + the shared revoked-set handle exist before any remote-auth path is
    // wired (Task 210's auth middleware consumes `validate`; Task 209 populates
    // the same revoked-set handle on revoke).
    //
    // The revoked-set handle the issuer reads and Task 209's revoke path
    // writes. Empty at boot; Task 209 will mirror the `devices` table into it.
    let revoked_set = concerto_identity::new_revoked_set();
    // Identity establishment is a best-effort boot probe, mirroring the
    // `vcs.gh_auth` / `skills.boot_refresh` probes below: a keychain failure
    // (e.g. a headless CI sandbox or a Linux box with no Secret Service)
    // logs a warning and leaves the issuer unconstructed rather than aborting
    // the whole Core. `design/12 §8` calls for *blocking* startup when the
    // keychain denies access; that hardening belongs with Task 210/211 (the
    // remote-auth path that actually consumes the issuer and can refuse remote
    // connections when no identity exists) — until then, local UDS operation
    // must not require a keychain.
    //
    // Task 207/209: when the identity is established, construct the
    // `PairingCoordinator` (pairing RPCs) AND the `DeviceManager`
    // (list/revoke/core-info RPCs) from the issuer + a `Persistence` clone + an
    // audit writer clone, behind `Arc`s so the api-server actor's factory
    // closure (which may re-run on a supervised restart) shares the SAME
    // in-memory token store. Both are injected into the gRPC `Devices` service
    // below. The `DeviceManager` shares the SAME `revoked_set` handle the
    // issuer reads, so a `RevokeDevice` insert is observed by the next
    // `validate` (Task 206) with no DB round-trip.
    //
    // Task 217.5: decide whether to bring up the Iroh transport listener. It is
    // opt-in (`CONCERTO_ENABLE_IROH`, default OFF) so the default UDS-only boot —
    // and every existing smoke capability — is byte-unchanged; Task 220's
    // split-host smoke flips it on. When on, the listener also honours managed
    // policy: `managed.json.disable_remote` (Task 211) puts the endpoint in
    // LAN-only mode (no relay, LAN connections only; mDNS unaffected).
    let iroh_enabled = iroh_listener_enabled();
    let remote_disabled = crate::security::managed::load_managed_policy(config_dir.as_path())
        .map(|p| p.remote_disabled())
        .unwrap_or(false);
    if iroh_enabled {
        tracing::info!(
            remote_disabled,
            "iroh transport listener enabled ({ENABLE_IROH_ENV})"
        );
    }

    // The `SessionCloser` seam: `NoopSessionCloser` when the Iroh listener is off
    // (a co-located UDS Core has no remote device streams to sever), replaced by
    // the live [`IrohSessionCloser`] (backed by the transport) when it is on, so
    // `DeviceManager::revoke_device` actually severs the revoked device's open
    // Iroh sessions (Task 217.5, `design/12 §7.3`).
    //
    // The live Iroh runtime (transport + pairing responder) is built inside the
    // identity arm below (it needs the Core's Noise static from the keychain) and
    // captured here so the post-gRPC-server spawn block can drive `serve_iroh` +
    // the pairing accept loop.
    let mut iroh_runtime: Option<IrohRuntime> = None;
    #[allow(clippy::type_complexity)]
    let identity_subsystems: Option<(
        Arc<crate::security::pairing::PairingCoordinator>,
        Arc<crate::security::devices::DeviceManager>,
        Arc<dyn concerto_identity::DeviceCertIssuer>,
    )> = match home::home_dir() {
        Some(core_home) => {
            let secrets = concerto_keychain::Secrets::new();
            match crate::security::identity::load_or_create_core_identity(
                &secrets,
                &core_home,
                &audit_writer,
            )
            .await
            {
                Ok(core_identity) => {
                    let core_pubkey = core_identity.public_key;
                    let core_pubkey_hex = hex::encode(core_pubkey.to_bytes());
                    let first_launch = core_identity.created;
                    // Task 210: the auth middleware needs an
                    // `Arc<dyn DeviceCertIssuer>` to validate inbound
                    // `concerto-device-cert` headers. The `PairingCoordinator`
                    // (Task 207) owns a `LocalCoreIssuer` BY VALUE and needs its
                    // `LocalCoreIssuer`-specific `core_public_key()` accessor, so
                    // it cannot share a `dyn` handle. `KeyPair` is `ZeroizeOnDrop`
                    // (not `Clone`), so rather than fork pairing.rs we build a
                    // SECOND issuer for the auth path by reloading the identity
                    // (the keychain reload returns the same key material;
                    // `created == false` so no second `CoreIdentityCreated`
                    // event fires). Both issuers share the SAME `revoked_set`
                    // handle, so a revoke is observed on the auth path too.
                    let auth_issuer: Arc<dyn concerto_identity::DeviceCertIssuer> = {
                        let auth_identity =
                            crate::security::identity::load_or_create_core_identity(
                                &secrets,
                                &core_home,
                                &audit_writer,
                            )
                            .await?;
                        Arc::new(concerto_identity::LocalCoreIssuer::new(
                            auth_identity.keypair,
                            auth_identity.public_key,
                            revoked_set.clone(),
                        ))
                    };
                    let core_issuer = concerto_identity::LocalCoreIssuer::new(
                        core_identity.keypair,
                        core_pubkey,
                        revoked_set.clone(),
                    );

                    // Task 217.5: bring up the Iroh transport (config-gated). It
                    // needs the Core's persistent X25519 Noise static (distinct
                    // from the Ed25519 identity above) — load/create it from the
                    // keychain, best-effort + timeout-bounded so a keychain-less /
                    // headless env degrades to "Iroh off" rather than hanging
                    // (Task 206 pattern). On success we get the live transport, its
                    // dialable LAN endpoint id (fed to the coordinator's QR hint),
                    // and the live `SessionCloser`.
                    let (session_closer, lan_endpoint, transport_for_runtime): (
                        Arc<dyn crate::security::devices::SessionCloser>,
                        String,
                        Option<Arc<concerto_transport::IrohTransport>>,
                    ) = if iroh_enabled {
                        match build_iroh_transport(&secrets, remote_disabled).await {
                            Ok(transport) => {
                                let endpoint_id = transport.endpoint_id().to_string();
                                tracing::info!(
                                    iroh_endpoint_id = %endpoint_id,
                                    core_noise_public = %hex::encode(transport.core_noise_public()),
                                    "iroh transport up; clients dial this endpoint id"
                                );
                                let closer: Arc<dyn crate::security::devices::SessionCloser> =
                                    Arc::new(IrohSessionCloser {
                                        transport: Arc::clone(&transport),
                                    });
                                (closer, endpoint_id, Some(transport))
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "iroh transport failed to start; falling back to UDS-only \
                                     (no remote pairing this boot)"
                                );
                                (
                                    Arc::new(crate::security::devices::NoopSessionCloser),
                                    String::new(),
                                    None,
                                )
                            }
                        }
                    } else {
                        (
                            Arc::new(crate::security::devices::NoopSessionCloser),
                            String::new(),
                            None,
                        )
                    };

                    // `lan_endpoint` carries the live Iroh endpoint id into the QR
                    // payload (the FROZEN `PairingChallenge.lan_endpoint` carrier)
                    // so a client `StartPairing` learns where to dial; `relay_hint`
                    // stays empty (relay path is Task 214/215). Empty when the Iroh
                    // listener is off — pairing is transport-agnostic.
                    let coordinator = Arc::new(crate::security::pairing::PairingCoordinator::new(
                        core_issuer,
                        Arc::clone(&persistence),
                        audit_writer.clone(),
                        lan_endpoint,
                        String::new(),
                    ));
                    // Task 209: the device manager shares the SAME revoked-set
                    // handle the issuer above reads, plus the `SessionCloser`
                    // seam, so a revoke severs the device everywhere.
                    let device_manager = crate::security::devices::DeviceManager::new(
                        Arc::clone(&persistence),
                        revoked_set.clone(),
                        core_pubkey,
                        audit_writer.clone(),
                        Arc::clone(&session_closer),
                    );
                    // Task 217.5: when the transport is live, build the Core-side
                    // Noise-XX pairing responder over it and stash the runtime so
                    // the post-server block spawns `serve_iroh` + the pairing
                    // accept loop.
                    if let Some(transport) = transport_for_runtime {
                        let pairing_responder =
                            Arc::new(crate::security::iroh_pairing::IrohPairingResponder::new(
                                Arc::clone(&transport),
                                Arc::clone(&coordinator),
                                runtime.shutdown_token(),
                            ));
                        iroh_runtime = Some(IrohRuntime {
                            transport,
                            pairing_responder,
                        });
                    }
                    tracing::info!(
                        core_pubkey = %core_pubkey_hex,
                        first_launch,
                        iroh_enabled,
                        "core device-cert issuer + pairing coordinator + device manager constructed"
                    );
                    Some((coordinator, Arc::new(device_manager), auth_issuer))
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "core identity establishment failed; remote device pairing \
                         unavailable until a keychain-backed identity exists (Task 210/211 \
                         will gate remote connections on it)"
                    );
                    None
                }
            }
        }
        None => {
            tracing::warn!("home::home_dir() returned None; skipping core identity establishment");
            None
        }
    };
    let pairing_coordinator: Option<Arc<crate::security::pairing::PairingCoordinator>> =
        identity_subsystems.as_ref().map(|(c, _, _)| Arc::clone(c));
    let auth_issuer: Option<Arc<dyn concerto_identity::DeviceCertIssuer>> =
        identity_subsystems.as_ref().map(|(_, _, i)| Arc::clone(i));
    let device_manager: Option<Arc<crate::security::devices::DeviceManager>> =
        identity_subsystems.map(|(_, d, _)| d);

    // Task 210 — CLOSE THE TASK-209 STARTUP-MIRROR GAP. The in-memory
    // `revoked_set` the auth middleware + issuer read starts EMPTY each boot;
    // Task 209 only inserts into it on a live `RevokeDevice` call. Without this
    // mirror, a device revoked in a previous run would be accepted again after a
    // Core restart until it was re-revoked (its `devices.revoked_at` is set on
    // disk, but the set has forgotten it). Re-populate the set from the table —
    // `SELECT id FROM devices WHERE revoked_at IS NOT NULL` — BEFORE the gRPC
    // server (and thus the auth path) goes live below, so a previously-revoked
    // cert stays rejected across restarts. Runs unconditionally (even when no
    // keychain identity exists) since it only touches the DB + the shared set;
    // a query failure is logged but does not abort boot (the set simply stays as
    // it was — defence in depth, never a fail-open that *adds* trust).
    match crate::security::auth::mirror_revoked_devices(&persistence, &revoked_set).await {
        Ok(0) => tracing::debug!("revoked-device mirror: no revoked devices to restore"),
        Ok(n) => tracing::info!(restored = n, "revoked-device mirror complete"),
        Err(e) => tracing::warn!(
            error = %e,
            "revoked-device mirror failed; a previously-revoked device could be \
             accepted until re-revoked this session"
        ),
    }

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

    // Task 308: the single shared per-workarea edit-mutex registry
    // (`design/04 §3.5`, `PHASE3_PLANNING §2`). Constructed exactly once
    // and `Arc::clone`d into BOTH the Workarea Manager (which reads the
    // holder for diagnostics) and the Agent Supervisor (which acquires
    // the lock around write-class tool calls). Two registries would
    // defeat the cross-session lock, so the single instance is
    // load-bearing.
    let edit_mutex_registry = Arc::new(crate::workspace_manager::EditMutexRegistry::new());

    // Task 20: spawn the Workarea Manager. The handle owns workarea
    // creation (composer-name allocation, worktree setup, `.context/`
    // skeleton) and emits `workarea.events` on its broadcast channel.
    let workarea_actor = WorkareaManagerActor::new(
        Arc::clone(&persistence),
        repo_handle.clone(),
        Arc::clone(&data_dir),
        Arc::clone(&config_dir),
    );
    // Task 308: hand the Workarea Manager an `Arc` to the SAME edit-mutex
    // registry the Agent Supervisor acquires on writes, so it can read the
    // holder for UI / diagnostics. Cross-platform (the registry type is
    // not `#[cfg(unix)]`).
    let workarea_handle = workarea_actor
        .handle()
        .with_edit_mutex_registry(Arc::clone(&edit_mutex_registry));
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
        let handle = actor
            .handle()
            .with_edit_mutex_registry(Arc::clone(&edit_mutex_registry));
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

    // Task 307: drive the workarea status FSM from Agent Supervisor session
    // events. The pump polls live sessions (1 s) and subscribes — once each
    // — to every session's `AgentEvent` stream, funnelling `Started` /
    // `AwaitingApproval` / `ApprovalResolved` / `Exited` / `Crashed` through
    // `transition_workarea`. Cancelled on root shutdown.
    #[cfg(unix)]
    workarea_handle
        .spawn_session_fsm_pump(agent_supervisor_handle.clone(), runtime.shutdown_token());

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

    // Task 318: wire the VCS handle as the Scheduler's check-runs source for
    // `wait_for_check_runs` (the gate Task 320's coordinated PR-set merge blocks
    // on). The VCS handle does not exist when the Scheduler is constructed
    // (~line 638, before this point), so we install it via a post-construction
    // setter rather than reordering the boot sequence. `VcsHandle` implements
    // `CheckRunsSource` (delegating to `get_check_runs` + subscribing to the
    // `checks.<wa>.<repo>` webhook emits for the fast-path). Unix-gated to match
    // the `#[cfg(unix)]` `scheduler_handle` above (Windows scheduler = Task 702/P7).
    #[cfg(unix)]
    scheduler_handle.set_check_runs_source(Arc::new(vcs_handle.clone()));

    // Task 320: wire the VCS handle + the Scheduler into the Workarea Manager so
    // its coordinated PR-set merge loop can drive single-PR merge/revert (via the
    // VCS handle) and block on `wait_for_check_runs` (via the Scheduler) between
    // members. `with_vcs` is cross-platform (the merge seam wraps the VcsHandle);
    // `with_scheduler` is `#[cfg(unix)]`-gated to match the unix-only Scheduler
    // (Windows scheduler = Task 702/Phase 7) — on non-unix the coordinated merge
    // RPC compiles but returns a typed "unsupported on this platform" error.
    let workarea_handle = workarea_handle.with_vcs(vcs_handle.clone());
    #[cfg(unix)]
    let workarea_handle = workarea_handle.with_scheduler(scheduler_handle.clone());

    // Task 320.5: wire the LIVE Linear/Jira issue write-back the coordinated-merge
    // success path calls (per-workspace opt-in via `workspaces.settings_json`).
    // Keychain-backed tokens (317's `VcsSecretSlot` accessors); mints nothing.
    // Cross-platform (the write-back is pure `reqwest`/rustls); the call site is
    // `#[cfg(unix)]` (the merge loop), so on Windows it is wired but unused until
    // the Windows scheduler (Task 702).
    let workarea_handle = match crate::vcs::build_issue_write_back(Arc::clone(&persistence)) {
        Ok(write_back) => workarea_handle.with_issue_write_back(write_back),
        Err(e) => {
            // A build failure (http client) is non-fatal: keep the no-op default
            // so the merge path still completes (write-back is best-effort).
            tracing::warn!(error = %e, "issue write-back build failed; using no-op default");
            workarea_handle
        }
    };

    // Task 310: resolve every workspace's three-layer settings
    // (managed > checked-in > local DB > defaults) and emit one
    // `WorkspaceSettingsResolved{workspace_id, field, value_source}` audit per
    // field, mirroring how `load_managed_policy_audited` is called once at
    // boot (`design/03 §3.13`). The per-machine opt-out config + the
    // checked-in `workspace_settings.json` / `action_prefs.toml` files live
    // under `~/.concerto/` + each repo's worktree `.concerto/`. Best-effort:
    // a resolution failure for one workspace logs + skips; it never gates boot.
    let settings_home_concerto = home_dir.join(".concerto");
    match crate::settings::resolve_and_audit_all_workspaces(
        &persistence,
        config_dir.as_path(),
        &settings_home_concerto,
        &audit_writer,
    )
    .await
    {
        Ok(n) => tracing::debug!(events = n, "workspace-settings boot resolution complete"),
        Err(e) => tracing::warn!(error = %e, "workspace-settings boot resolution failed"),
    }

    // Task 414: construct the live Maestro handle, gated on the Maestro being
    // enabled (403's `maestro_state.enabled`, §4.6) AND a managed-policy model
    // permission (D1: `enterpriseDataPrivacy=true` + an external `default_model`
    // ⇒ the Maestro LLM is disabled, design/08 §3.10). When the gate is closed
    // the handle is left `None` (no spawn, logged) and the service replies
    // `disabled_by_policy`; the `maestro.events` subject stays valid-but-empty.
    //
    // The CLI backends (Claude/Codex/Gemini, D1) are local and pass the gate;
    // the disabled case is Direct-API + external under enterprise privacy (and
    // Direct-API is itself a frozen-unwired seam per 412). The handle stitches
    // 408's routing, 409's digest over a fresh 404 summary cache, 413's
    // visibility toggle, and 414's `maestro.events` producer.
    #[cfg(unix)]
    let maestro_handle: Option<crate::maestro::MaestroHandle> = {
        // Bootstrap the `maestro_state` singleton + the `chats(kind='maestro')`
        // row (403) so `GetDigest` has a persistence anchor (409's D11 chips).
        {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            let mut w = persistence.writer().await;
            if let Err(e) =
                concerto_persist::maestro_state::ensure_initialized(&mut w, now_ms).await
            {
                tracing::warn!(error = %e, "maestro_state init failed; Maestro disabled");
            }
            if let Err(e) = concerto_persist::maestro_state::ensure_maestro_chat(
                &mut w,
                &uuid::Uuid::now_v7().to_string(),
                now_ms,
            )
            .await
            {
                tracing::warn!(error = %e, "maestro chat bootstrap failed");
            }
        }

        let enabled = concerto_persist::maestro_state::get(persistence.readers())
            .await
            .ok()
            .flatten()
            .map(|s| s.enabled)
            .unwrap_or(false);

        // The managed-policy model gate (D1). `default_model` is the org's
        // chosen Maestro model; under `enterpriseDataPrivacy` an external model
        // disables the LLM (the on-prem/local case re-enables it — Tier-3). In
        // V1.0 the practical external case is Direct-API, which is unwired (412),
        // so a CLI default passes. A model whose name signals a public provider
        // under privacy is the disabled case.
        let managed =
            crate::security::managed::load_managed_policy(config_dir.as_path()).unwrap_or_default();
        let privacy = managed.enterprise_data_privacy().unwrap_or(false);
        let model_external = managed
            .default_model()
            .map(is_external_maestro_model)
            .unwrap_or(false);
        let disabled_by_policy =
            crate::maestro::PrivacyPolicy::maestro_disabled_by_policy(privacy, model_external);

        if !enabled {
            tracing::info!(
                target: "concerto::maestro",
                reason = "disabled",
                "maestro disabled at boot (maestro_state.enabled = false)"
            );
            None
        } else if disabled_by_policy {
            tracing::info!(
                target: "concerto::maestro",
                reason = "enterprise_data_privacy",
                "maestro disabled at boot (enterpriseDataPrivacy + external default_model — D1)"
            );
            None
        } else {
            let summary_cache = Arc::new(tokio::sync::Mutex::new(
                crate::maestro::summary::SummaryCache::with_system_clock(),
            ));
            let oneshot = crate::maestro::digest::default_oneshot();
            let events = crate::maestro::MaestroEventSender::new();
            tracing::info!(target: "concerto::maestro", "maestro enabled at boot");
            Some(crate::maestro::MaestroHandle::new(
                Arc::clone(&persistence),
                workarea_handle.clone(),
                agent_supervisor_handle.clone(),
                summary_cache,
                oneshot,
                events,
            ))
        }
    };

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
    #[cfg(unix)]
    let factory_maestro_handle = maestro_handle.clone();
    let factory_vcs_handle = vcs_handle.clone();
    let factory_pairing = pairing_coordinator.clone();
    let factory_device_manager = device_manager.clone();
    let factory_auth_issuer = auth_issuer.clone();
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
                    // Task 414: the live Maestro handle (or `None` when the boot
                    // gate is closed — disabled / disabled-by-policy). Threaded
                    // through `with_managers` → `run_uds`/`add_core_services`
                    // AND the bridge `BridgeServices` (D8 two-site serve).
                    #[cfg(unix)]
                    factory_maestro_handle.clone(),
                    Some(factory_vcs_handle.clone()),
                    factory_pairing.clone(),
                    factory_device_manager.clone(),
                    factory_auth_issuer.clone(),
                )
            },
            ApiServerConfig {
                socket_path: socket_path.clone(),
            },
        )
        .await?;

    // Task 217.5: spawn the Iroh transport serve loop + the Core-side pairing
    // responder, when the listener is enabled. Runs AFTER the gRPC server spawn
    // (so the shared handler set exists) and AFTER the revoked-set mirror above
    // (so pairing/auth is never reachable before a previously-revoked device is
    // re-rejected — the boot-ordering invariant). Both are tied to the runtime
    // shutdown token so they tear down cleanly with the rest of Core (no leaked
    // endpoint). The endpoint was bound at `build_iroh_transport`; here we attach
    // the shared dispatcher + the `0x03` accept loop.
    if let Some(iroh) = &iroh_runtime {
        let shutdown = runtime.shutdown_token();

        // The IDENTICAL handler set the UDS path serves, tagged `IROH` by the
        // dispatcher's interceptor (Task 201/210). Built from the same handles —
        // one source of truth, no per-transport handler branching.
        let services = crate::api_server::CoreServiceSet {
            started_at: Arc::clone(&started_at),
            supervisor_view: supervisor_view.clone(),
            repo_manager: Some(repo_handle.clone()),
            workspace_manager: Some(workspace_handle.clone()),
            workarea_manager: Some(workarea_handle.clone()),
            #[cfg(unix)]
            agent_supervisor: Some(agent_supervisor_handle.clone()),
            persistence: Some(Arc::clone(&persistence)),
            #[cfg(unix)]
            scheduler: Some(scheduler_handle.clone()),
            skills_registry: Some(skills_handle.clone()),
            #[cfg(unix)]
            suggestions: Some(suggestions_handle.clone()),
            // Task 414: the live Maestro handle on the Iroh serve path (or
            // `None` when the boot gate is closed).
            #[cfg(unix)]
            maestro: maestro_handle.clone(),
            vcs: Some(vcs_handle.clone()),
            pairing: pairing_coordinator.clone(),
            device_manager: device_manager.clone(),
            auth_issuer: auth_issuer.clone(),
            // Wire the live Iroh transport as the Runtime NatStatsSource so
            // `GetNatStats` over the Iroh path reports the transport's real
            // per-session counters (Task 216's deferred surfacing; 217.5 boot).
            nat_stats: Some(Arc::new(crate::handlers::runtime::IrohNatStatsSource(
                Arc::clone(&iroh.transport),
            ))),
        };

        // The serve loop: `serve_iroh` runs the transport's accept loop (its own
        // internal `select!` on the transport's shutdown token), handing every API
        // stream to the shared dispatcher. A watcher task translates the runtime
        // shutdown token into `transport.stop()` (which cancels that internal
        // token + closes the endpoint cleanly — no leaked endpoint).
        // Task 315: install the Core's inbound-webhook seam BEFORE the serve loop
        // starts accepting, so any `0x04` Webhook stream is demuxed to the VCS
        // `ingest_webhook` path (idempotency → constant-time HMAC → parse →
        // targeted-invalidate) rather than dropped. The sink wraps a `VcsHandle`
        // equipped with the keychain-backed webhook-secret + re-fetch-provider
        // seams. Strictly additive: an unwired sink (or any webhook-path failure)
        // never affects the poll path.
        iroh.transport
            .set_webhook_sink(crate::vcs::build_webhook_sink(vcs_handle.clone()));

        let serve_transport = Arc::clone(&iroh.transport);
        let stop_transport = Arc::clone(&iroh.transport);
        let stop_shutdown = shutdown.clone();
        tokio::spawn(async move {
            stop_shutdown.cancelled().await;
            stop_transport.stop();
        });
        tokio::spawn(async move {
            if let Err(e) = crate::api_server::serve_iroh(serve_transport, services).await {
                tracing::warn!(error = %e, "iroh serve loop ended with error");
            }
        });

        // The pairing responder is armed on demand: each `StartPairing` (via
        // `IrohPairingResponder::start_pairing`, the seam Task 220's runtime
        // pairing-start path drives) opens the `0x03` listener for that token +
        // spawns its accept task (tied to the same shutdown token). Nothing to
        // spawn standing here — the responder is held in `iroh_runtime` ready to
        // arm.
        let _ = &iroh.pairing_responder;

        tracing::info!("iroh serve loop spawned; pairing responder armed-on-demand");
    }

    tracing::info!("concerto-core ready");

    Ok(BootOutcome::Started(RunningCore {
        runtime,
        socket_path,
        iroh: iroh_runtime,
    }))
}
