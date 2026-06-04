//! Method impls for the FROZEN [`TransportHandle`](crate::api::TransportHandle)
//! façade (`design/11 §5.1`, Task 217).
//!
//! The type *declaration* — [`TransportHandle`](crate::api::TransportHandle) — and
//! the nine public method signatures live in [`crate::api`] (the
//! interface-generator convention, so `scripts/regen-interfaces.sh` indexes the
//! surface). This module holds the glue: the [`HandleInner`] state the façade
//! wraps and the thin delegations to Task 212's [`IrohTransport`].
//!
//! # Why a façade over [`IrohTransport`]
//!
//! Task 212's [`IrohTransport::start`] is a **constructor** (`async fn start(cfg,
//! key) -> Result<Self>`) and [`IrohTransport::serve`] runs the accept loop.
//! `design/11 §5.1`'s `TransportHandle::start(&self, cfg)` is a `&self` lifecycle
//! control. The handle reconciles the two: it is built once ([`HandleInner::new`])
//! holding the Core's Noise key + dispatcher, then `start` constructs the
//! [`IrohTransport`], spawns its serve loop, and parks both in a slot; `stop`
//! cancels the loop and clears the slot. The other seven methods delegate to the
//! parked [`IrohTransport`], erroring cleanly when the endpoint is not up.

use std::sync::{Arc, Mutex};

use tokio::task::JoinHandle;

use crate::api::{
    ApiDispatcher, IrohTransport, NatStats, PairingListener, RelayInfo, TransportConfig,
    WakeupPayload,
};
use crate::error::{Result, TransportError};

/// A running transport: the built [`IrohTransport`] plus the serve-loop task that
/// drives its accept loop (`design/11 §5.1`). Held in [`HandleInner::running`]
/// between `start` and `stop`.
struct Running {
    transport: Arc<IrohTransport>,
    serve_task: JoinHandle<()>,
}

/// The opaque internals [`TransportHandle`](crate::api::TransportHandle) wraps.
/// Holds what `start` needs to build the [`IrohTransport`] (the Core Noise static
/// private key + the dispatcher) and the running-transport slot the lifecycle +
/// delegating methods read.
pub struct HandleInner<D: ApiDispatcher> {
    /// The Core's persisted 32-byte X25519 Noise static private key (the value
    /// [`IrohTransport::start`] takes — kept so each `start` can rebuild the
    /// endpoint after a `stop`). The handle layer stays keychain-free; the Core
    /// owns the at-rest form.
    core_noise_static_private: [u8; 32],
    /// The Core's shared gRPC service set the serve loop hands every API stream.
    dispatcher: Arc<D>,
    /// `Some` between `start` and `stop`; `None` before the first `start` / after
    /// a `stop`. The lifecycle + delegating methods read it.
    running: Mutex<Option<Running>>,
}

impl<D: ApiDispatcher> HandleInner<D> {
    /// Build the inner state (no endpoint yet — `start` binds it).
    pub fn new(core_noise_static_private: [u8; 32], dispatcher: Arc<D>) -> Self {
        Self {
            core_noise_static_private,
            dispatcher,
            running: Mutex::new(None),
        }
    }

    /// Bring the endpoint up (`design/11 §5.1` `start`): build + bind the
    /// [`IrohTransport`] per `cfg`, spawn its serve loop, park both. A second
    /// `start` before a `stop` is a clean [`TransportError::Lifecycle`].
    pub async fn start(&self, cfg: TransportConfig) -> Result<()> {
        // Reject a double-start up front so we never bind a second endpoint.
        if self.running.lock().expect("handle lock").is_some() {
            return Err(TransportError::Lifecycle(
                "start called while the transport is already running (stop first)".into(),
            ));
        }

        let transport = Arc::new(IrohTransport::start(cfg, self.core_noise_static_private).await?);

        let serve_transport = transport.clone();
        let dispatcher = self.dispatcher.clone();
        let serve_task = tokio::spawn(async move {
            if let Err(err) = serve_transport.serve(dispatcher).await {
                tracing::warn!(%err, "transport serve loop ended with error");
            }
        });

        let mut slot = self.running.lock().expect("handle lock");
        // Re-check under the lock: a concurrent `start` may have won the race
        // between our first check and here. If so, tear our just-built endpoint
        // down rather than leaking it, and report the lifecycle error.
        if slot.is_some() {
            transport.stop();
            serve_task.abort();
            return Err(TransportError::Lifecycle(
                "start raced another start (transport already running)".into(),
            ));
        }
        *slot = Some(Running {
            transport,
            serve_task,
        });
        Ok(())
    }

    /// Tear the endpoint down (`design/11 §5.1` `stop`): cancel the serve loop
    /// (which closes the Iroh endpoint + deregisters mDNS) and drop the
    /// transport. Idempotent — a `stop` on a not-started handle is `Ok(())`.
    pub async fn stop(&self) -> Result<()> {
        let running = self.running.lock().expect("handle lock").take();
        if let Some(running) = running {
            // Signal the serve loop to stop; it closes the endpoint on its way
            // out. Await the task so `stop` is synchronous from the caller's
            // view (the endpoint is closed when `stop` returns).
            running.transport.stop();
            let _ = running.serve_task.await;
        }
        Ok(())
    }

    /// Run `f` against the running [`IrohTransport`], or a clean
    /// [`TransportError::Lifecycle`] when the endpoint is not up. The shared body
    /// of every delegating method so "not started" is one consistent error.
    fn with_transport<T>(&self, f: impl FnOnce(&IrohTransport) -> T) -> Result<T> {
        let slot = self.running.lock().expect("handle lock");
        match slot.as_ref() {
            Some(running) => Ok(f(&running.transport)),
            None => Err(TransportError::Lifecycle(
                "transport not started (call start first)".into(),
            )),
        }
    }

    /// Delegate to [`IrohTransport::listen_pairing`] (`design/11 §5.1`).
    pub fn listen_pairing(&self, token_hash: [u8; 32]) -> Result<PairingListener> {
        self.with_transport(|t| t.listen_pairing(token_hash))
    }

    /// Delegate to [`IrohTransport::close_pairing`] (`design/11 §5.1`).
    pub fn close_pairing(&self) -> Result<()> {
        self.with_transport(|t| t.close_pairing())
    }

    /// Delegate to [`IrohTransport::current_relay`] (`design/11 §5.1`).
    pub fn current_relay(&self) -> Result<RelayInfo> {
        self.with_transport(|t| t.current_relay())
    }

    /// Delegate to [`IrohTransport::switch_relay`] (`design/11 §5.1`). The
    /// `url::Url` the façade takes verbatim is rendered to the `String` shape
    /// Task 212's endpoint stores. `switch_relay`'s own `disable_remote` refusal
    /// is the inner `Result` (flattened so the façade returns one `Result`).
    pub fn switch_relay(&self, url: url::Url) -> Result<()> {
        self.with_transport(|t| t.switch_relay(url.to_string()))?
    }

    /// Delegate to [`IrohTransport::nat_stats`] (`design/11 §5.1`).
    pub fn nat_stats(&self) -> Result<NatStats> {
        self.with_transport(|t| t.nat_stats())
    }

    /// Subscribe to the transport-lifecycle telemetry broadcast (`design/11
    /// §5.3`). NOT one of the frozen nine — a companion accessor Task 212
    /// explicitly anticipated the façade re-exposing for the Phase-6 Diagnostics
    /// consumer (see [`IrohTransport::subscribe_telemetry`]).
    pub fn subscribe_telemetry(
        &self,
    ) -> Result<tokio::sync::broadcast::Receiver<crate::api::TransportTelemetry>> {
        self.with_transport(|t| t.subscribe_telemetry())
    }

    /// Take the push-hint receiver (`design/14` delivery loop drains it). NOT one
    /// of the frozen nine — a companion accessor the P5 push backend (Task 503)
    /// drains, mirroring [`IrohTransport::take_wakeup_receiver`]. `None` if
    /// already taken (or `Err` if the endpoint is not up).
    pub fn take_wakeup_receiver(
        &self,
    ) -> Result<Option<tokio::sync::mpsc::UnboundedReceiver<crate::api::WakeupHint>>> {
        self.with_transport(|t| t.take_wakeup_receiver())
    }

    /// A clone of the running Iroh endpoint (`design/11 §3.1`). NOT one of the
    /// frozen nine — a companion accessor the mDNS responder (Task 213, which
    /// publishes the endpoint's addrs) + the Desktop connect path (Task 218) read.
    /// Errors when the endpoint is not up.
    pub fn endpoint(&self) -> Result<iroh::Endpoint> {
        self.with_transport(|t| t.endpoint())
    }

    /// The Iroh endpoint id clients dial (`design/11 §3.5` — the QR's
    /// `iroh_endpoint_id`). NOT one of the frozen nine — a companion accessor for
    /// the mDNS TXT (Task 213) + the pairing QR (Task 207/219). Errors when the
    /// endpoint is not up.
    pub fn endpoint_id(&self) -> Result<iroh::EndpointId> {
        self.with_transport(|t| t.endpoint_id())
    }

    /// The Core's X25519 Noise static **public** key (the QR's responder static
    /// for the device's IK handshake, `design/12 §3.1`). NOT one of the frozen
    /// nine — a companion accessor for the pairing QR (Task 207/219). Errors when
    /// the endpoint is not up.
    pub fn core_noise_public(&self) -> Result<[u8; 32]> {
        self.with_transport(|t| t.core_noise_public())
    }

    /// Delegate to [`IrohTransport::close_sessions_for_device`] (`design/11
    /// §5.1`, the Task-209 `SessionCloser` sever). Severing an unknown device is
    /// a clean no-op success (idempotent).
    pub fn close_sessions_for_device(&self, id: &crate::api::DeviceId) -> Result<()> {
        self.with_transport(|t| t.close_sessions_for_device(id))
    }

    /// Delegate to [`IrohTransport::send_wakeup_hint`] (`design/11 §5.1`). The
    /// opaque ID-only [`WakeupPayload`] bytes are handed to the push-hint channel
    /// unchanged. The inner `send_wakeup_hint` may error if the channel is closed
    /// (flattened so the façade returns one `Result`).
    pub fn send_wakeup_hint(&self, id: crate::api::DeviceId, payload: WakeupPayload) -> Result<()> {
        self.with_transport(|t| t.send_wakeup_hint(id, payload.bytes))?
    }
}
