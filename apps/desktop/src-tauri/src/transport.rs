//! The transport-agnostic [`CoreClient`] trait + its impls (`design/15 §3.2`).
//!
//! The Tauri command proxy (`commands.rs`) talks **only** to this trait; it no
//! longer dials a UDS socket directly. Two impls back it:
//!
//! - [`UdsCoreClient`] — co-located, peer-UID auth, tonic over a Unix domain
//!   socket. Wraps the lazy process-wide channel + reset-on-error strategy from
//!   [`crate::core_client`]; no behaviour change for the local path.
//! - [`IrohCoreClient`] (feature `iroh-transport`) — split-host, device-cert
//!   auth, tonic over Iroh via Task 217's `TransportHandle` client side / Task
//!   212's hand-rolled adapter. Presents the stored `SignedDeviceCert` in
//!   request metadata. **No** `tonic-iroh-transport`.
//!
//! Both impls resolve a `tonic::transport::Channel` and route the same Tonic
//! service calls through the shared [`dispatch_over_channel`] /
//! [`subscribe_over_channel`] logic, so the per-RPC mapping lives in exactly one
//! place regardless of transport.

use std::path::PathBuf;

use serde_json::Value;
use tokio::task::JoinHandle;
use tonic::transport::Channel;

use crate::core_client::CoreClientError;
use crate::rpc;

/// A renderer-facing subscription id (the handle the renderer hands back to
/// `concerto_unsubscribe`). Adapts `design/15 §3.2`'s `SubscriptionId` to the
/// existing `commands.rs` registry, which keys spawned forwarder tasks by this
/// string.
pub type SubscriptionId = String;

/// The event-forwarding sink a [`CoreClient::start_stream`] drives (`design/15
/// §3.2`'s `StreamSink`). Each decoded stream frame is handed to `emit`, which
/// forwards it onto the Tauri event bus. Returning `false` from `emit` ends the
/// stream (the bus rejected the event). Kept as a boxed `Fn` so the shell's
/// `AppHandle` emit closure plugs straight in without the trait depending on
/// Tauri types.
#[derive(Clone)]
pub struct StreamSink {
    emit: std::sync::Arc<dyn Fn(&Value) -> bool + Send + Sync>,
}

impl StreamSink {
    /// Build a sink from the bus-emit closure.
    pub fn new(emit: impl Fn(&Value) -> bool + Send + Sync + 'static) -> Self {
        Self {
            emit: std::sync::Arc::new(emit),
        }
    }

    /// Forward one decoded frame; `false` means "stop the stream".
    pub fn emit(&self, frame: &Value) -> bool {
        (self.emit)(frame)
    }
}

/// A live subscription: its id + the spawned forwarder task the caller registers
/// so `concerto_unsubscribe` can abort it. (`design/15 §3.2` returns just the
/// id; the desktop keeps the `JoinHandle` so the existing
/// [`crate::commands::SubscriptionRegistry`] can abort the task.)
pub struct StreamSubscription {
    /// The renderer-facing subscription id.
    pub id: SubscriptionId,
    /// The spawned forwarder task; aborted on unsubscribe.
    pub join: JoinHandle<()>,
}

/// The transport-agnostic gRPC client the renderer's command proxy wraps
/// (`design/15 §3.2`). **FROZEN method set** — `dispatch` + `start_stream` with
/// these exact signatures; every transport impl (UDS, Iroh) implements it and
/// `commands.rs` only ever talks to the trait.
#[async_trait::async_trait]
pub trait CoreClient: Send + Sync {
    /// Single unary dispatch entry. `method` is `"<Service>.<Rpc>"`; `payload`
    /// is the request body as JSON; returns the response body as JSON.
    async fn dispatch(&self, method: &str, payload: Value) -> Result<Value, CoreClientError>;

    /// Open a `Streams.Subscribe(subject)` server-stream and forward each frame
    /// to `sink`. Returns the [`StreamSubscription`] (id + forwarder handle).
    async fn start_stream(
        &self,
        subject: &str,
        filter: Value,
        sink: StreamSink,
    ) -> Result<StreamSubscription, CoreClientError>;
}

/// Co-located UDS impl (`design/15 §3.2`). Peer-UID auth; reuses the lazy
/// process-wide channel + reset-on-error reconnect from [`crate::core_client`].
pub struct UdsCoreClient {
    socket_path: PathBuf,
}

impl UdsCoreClient {
    /// Build a client for the UDS at `socket_path`.
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    /// Resolve the live UDS channel, retrying transient dial failures the way
    /// the old `concerto_rpc` did (the Core may be mid-restart with a new
    /// socket inode). Resets the cached channel on failure so the next call
    /// re-dials.
    async fn channel(&self) -> Result<Channel, CoreClientError> {
        match crate::core_client::get_or_connect(&self.socket_path).await {
            Ok(ch) => Ok(ch),
            Err(e) => {
                crate::core_client::reset_channel();
                Err(e)
            }
        }
    }
}

#[async_trait::async_trait]
impl CoreClient for UdsCoreClient {
    async fn dispatch(&self, method: &str, payload: Value) -> Result<Value, CoreClientError> {
        // Retry transport (dial) failures only — real RPC errors return
        // immediately. Mirrors the old `concerto_rpc` retry loop.
        let mut attempts = 0;
        loop {
            let channel = self.channel().await;
            let channel = match channel {
                Ok(ch) => ch,
                Err(e) => {
                    if attempts >= 3 {
                        return Err(e);
                    }
                    attempts += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                    continue;
                }
            };
            let result = rpc::dispatch_over_channel(channel, method, payload.clone()).await;
            match result {
                Err(CoreClientError::Transport(_)) if attempts < 3 => {
                    crate::core_client::reset_channel();
                    attempts += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                }
                other => return other,
            }
        }
    }

    async fn start_stream(
        &self,
        subject: &str,
        filter: Value,
        sink: StreamSink,
    ) -> Result<StreamSubscription, CoreClientError> {
        // Retry the dial / first subscribe the way the old `concerto_subscribe`
        // did so a Core restart is transparent.
        let mut attempts = 0;
        loop {
            let channel = match self.channel().await {
                Ok(ch) => ch,
                Err(e) => {
                    if attempts >= 3 {
                        return Err(e);
                    }
                    attempts += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                    continue;
                }
            };
            match rpc::subscribe_over_channel(channel, subject, filter.clone(), sink.clone()).await
            {
                Ok(sub) => return Ok(sub),
                Err(_e) if attempts < 3 => {
                    crate::core_client::reset_channel();
                    attempts += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}
