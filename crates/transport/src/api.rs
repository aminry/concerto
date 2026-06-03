//! The FROZEN public surface of `concerto-transport` (Task 212).
//!
//! Per the keychain/identity convention (`tasks/v1.0/205`,
//! `crates/identity/src/api.rs`), this file is what `scripts/regen-interfaces.sh`
//! reads to produce the `concerto-transport` section of
//! `docs/interfaces/rust-api.md`. The canonical **type declarations** (the
//! `pub struct` / `pub enum` / `pub trait` the generator indexes) are declared
//! **directly here**; their heavier internal logic lives in the topic modules
//! ([`crate::adapter`], [`crate::channels`], [`crate::endpoint`],
//! [`crate::state`]) as free functions the `impl` blocks below delegate to.
//!
//! # Why this surface is frozen
//!
//! Task 217's `TransportHandle` is a **thin façade** over [`IrohTransport`]; the
//! mDNS (213), relay (214), WSS (215), migration (216), and Desktop (218) tasks
//! build against the [`IrohDuplex`] / [`NoiseDuplex`] / [`IrohConnector`] adapter
//! contract, the [`ChannelTag`] framing + [`MAX_MESSAGE_SIZE`] ceiling, and the
//! [`TransportState`] / [`ActiveSession`] / [`ConnectionPath`] model. Freezing
//! names + signatures here lets all of them compile against this crate without a
//! re-lock.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use iroh::endpoint::Connection;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::{Result, TransportError};

// Re-export the free functions / consts / aliases that are part of the frozen
// surface but are not type declarations (the generator indexes the `pub
// struct/enum/trait` decls below; these flatten the rest for ergonomic import).
pub use crate::adapter::{
    handshake_initiator, handshake_responder, read_channel_tag, write_channel_tag,
};
pub use crate::channels::{MAX_MESSAGE_SIZE, NOISE_PLAINTEXT_CHUNK};
pub use crate::endpoint::{connect_channel, direct_endpoint_addr, ALPN};
pub use crate::error::Result as TransportResult;
pub use crate::state::classify_path;

// ===========================================================================
// channels.rs surface
// ===========================================================================

/// The single channel-tag byte at the head of every Iroh bidi stream
/// (`design/11 §3.3`). **FROZEN** — the demux contract every transport writes:
/// the acceptor reads this byte first, then routes the stream to API (gRPC) /
/// push-hint / pairing handling. The tag byte doubles as the acceptor-priming
/// write (spike gotcha #3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ChannelTag {
    /// The long-lived gRPC API stream pool. Wire byte `0x01`.
    Api = 0x01,
    /// The lightweight push-hint / wakeup-fetch channel. Wire byte `0x02`.
    PushHint = 0x02,
    /// The short-lived pairing channel (Noise XX over the token). Wire byte `0x03`.
    Pairing = 0x03,
}

impl ChannelTag {
    /// The on-wire byte for this tag.
    pub fn as_byte(self) -> u8 {
        self as u8
    }

    /// Decode a channel tag from its wire byte. Unknown bytes are a protocol
    /// error (the peer named a channel this Core does not multiplex).
    pub fn from_byte(b: u8) -> Result<Self> {
        crate::channels::tag_from_byte(b)
    }
}

// ===========================================================================
// state.rs surface — the in-memory model (`design/11 §4`)
// ===========================================================================

/// A device identifier — the string the transport keys live sessions on so the
/// Task-209 `SessionCloser` seam ([`IrohTransport::close_sessions_for_device`])
/// can sever a revoked device. A newtype (not a bare `String`) so the FROZEN
/// signature and the session map agree on the key type across 212/216/217.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(pub String);

impl DeviceId {
    /// Borrow the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for DeviceId {
    fn from(s: String) -> Self {
        DeviceId(s)
    }
}

/// How a session's QUIC traffic currently reaches the peer (`design/11 §4`).
/// **FROZEN** — the in-memory enum 216 aggregates into NAT stats and 217
/// surfaces per-session. Classified from Iroh's own per-path signal via
/// [`classify_path`] (`design/11 §3.6`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionPath {
    /// Hole-punched direct path over a public/routable IP (NAT traversal won).
    Direct,
    /// Relayed QUIC through the configured relay (hole-punch fell back).
    Relayed,
    /// LAN-direct over a loopback / private-range IP (mDNS-discovered or
    /// same-network), no relay involved.
    Lan,
}

impl ConnectionPath {
    /// Whether this path is local (LAN-direct) — the `disable_remote` accept
    /// gate (Task 211) admits only [`ConnectionPath::Lan`] when remote is off.
    pub fn is_lan(self) -> bool {
        matches!(self, ConnectionPath::Lan)
    }
}

/// Daily NAT-traversal counters (`design/11 §4`, §3.6). 212 increments these;
/// **Task 216 owns the aggregation** (`by_network_class`,
/// `transport.nat_success_changed`). FROZEN field layout so 216 reads it.
#[derive(Debug, Clone, Default)]
pub struct NatStats {
    /// Sessions that came up on a direct (hole-punched) path today.
    pub direct_today: u32,
    /// Sessions that came up relayed today.
    pub relayed_today: u32,
    /// Sessions that came up LAN-direct today.
    pub lan_today: u32,
}

impl NatStats {
    /// Increment the counter for a newly-established session's path.
    pub fn record(&mut self, path: ConnectionPath) {
        match path {
            ConnectionPath::Direct => self.direct_today = self.direct_today.saturating_add(1),
            ConnectionPath::Relayed => self.relayed_today = self.relayed_today.saturating_add(1),
            ConnectionPath::Lan => self.lan_today = self.lan_today.saturating_add(1),
        }
    }
}

/// One paired device's live connection (`design/11 §4`). Holds the Iroh
/// `Connection` so `close_sessions_for_device` can close it; `path` feeds 216's
/// NAT telemetry. The per-stream `NoiseSession` lives in the [`NoiseDuplex`] at
/// the adapter layer, not here (see [`crate::adapter`]).
pub struct ActiveSession {
    /// The device this session belongs to (keys the `sessions` map).
    pub device_id: DeviceId,
    /// The underlying Iroh QUIC connection (many API bidi streams ride it).
    pub iroh_connection: Connection,
    /// The classified connection path, refreshed from Iroh's signal.
    pub path: ConnectionPath,
    /// Last time a stream on this connection was seen (liveness / idle GC).
    pub last_seen: Instant,
}

impl ActiveSession {
    /// Build a session from a freshly-accepted/-opened Iroh connection,
    /// classifying its path now.
    pub fn new(device_id: DeviceId, iroh_connection: Connection) -> Self {
        let path = classify_path(&iroh_connection);
        Self {
            device_id,
            iroh_connection,
            path,
            last_seen: Instant::now(),
        }
    }

    /// Re-classify [`Self::path`] from the connection's current selected path
    /// (216 calls this as paths migrate). Returns the new path.
    pub fn refresh_path(&mut self) -> ConnectionPath {
        self.path = classify_path(&self.iroh_connection);
        self.last_seen = Instant::now();
        self.path
    }
}

/// The per-Core transport view (`design/11 §4`): the live sessions + the NAT
/// counters. The `iroh_endpoint` / `relay_url` / `pairing_listener` fields of the
/// design's struct live on [`IrohTransport`] (the owning handle); this is the
/// pure session-registry model 216/217 read.
#[derive(Default)]
pub struct TransportState {
    /// Live sessions keyed by device id.
    pub sessions: HashMap<DeviceId, ActiveSession>,
    /// Daily NAT-success counters (216 aggregates; 212 seeds).
    pub nat_stats: NatStats,
}

impl TransportState {
    /// A fresh, empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record (or replace) a session and bump the NAT counter for its path.
    pub fn insert_session(&mut self, session: ActiveSession) {
        self.nat_stats.record(session.path);
        self.sessions.insert(session.device_id.clone(), session);
    }

    /// Remove every session for `device_id`, closing each Iroh connection
    /// (`design/12 §7.3`). Idempotent.
    pub fn close_sessions_for_device(&mut self, device_id: &DeviceId) {
        if let Some(session) = self.sessions.remove(device_id) {
            session
                .iroh_connection
                .close(0u32.into(), b"device revoked");
        }
    }
}

// ===========================================================================
// adapter.rs surface — the four-gotcha adapter contract (`design/11 §3.1.1`)
// ===========================================================================

/// One accepted/opened Iroh bidi stream presented as a single
/// `AsyncRead + AsyncWrite + Connected` duplex for Tonic — the **raw**
/// (pre-Noise) byte duplex. One Iroh bidi stream ⇒ one [`IrohDuplex`] (spike
/// gotcha #2). The `Async{Read,Write}` impls use fully-qualified trait syntax to
/// dodge the inherent-vs-trait `poll_*` shadowing (gotcha #1); see
/// [`crate::adapter`].
pub struct IrohDuplex {
    pub(crate) send: iroh::endpoint::SendStream,
    pub(crate) recv: iroh::endpoint::RecvStream,
}

/// A byte duplex that transparently runs every Tonic frame through an
/// established Noise IK session — the second AEAD of `design/12 §3.4` (atop
/// Iroh's TLS). Wraps an [`IrohDuplex`]; itself `AsyncRead + AsyncWrite +
/// Connected`. Framing is length-prefixed Noise frames (≤
/// [`NOISE_PLAINTEXT_CHUNK`] plaintext per frame); a decrypt failure surfaces an
/// `io::Error` so Tonic tears the connection down (`design/12 §6.3`). Impl in
/// [`crate::adapter`].
pub struct NoiseDuplex {
    pub(crate) inner: IrohDuplex,
    pub(crate) session: concerto_identity::NoiseSession,
    pub(crate) read_plain: Vec<u8>,
    pub(crate) read_plain_pos: usize,
    pub(crate) read_state: crate::adapter::ReadState,
    pub(crate) write_buf: Vec<u8>,
    pub(crate) write_pos: usize,
}

/// Client-side connector: a tower `Service<Uri>` that, per gRPC channel connect,
/// opens a fresh bidi stream on the shared Iroh [`Connection`], writes the
/// channel-tag byte (the acceptor-priming write, gotcha #3), runs the Noise IK
/// **initiator** handshake, and hands Tonic the resulting [`NoiseDuplex`]. One
/// gRPC connection ⇒ one fresh primed bidi stream (gotcha #2). Impl in
/// [`crate::adapter`].
#[derive(Clone)]
pub struct IrohConnector {
    pub(crate) conn: Connection,
    pub(crate) local_static: Arc<concerto_identity::NoiseStatic>,
    pub(crate) remote_static_pub: [u8; 32],
}

impl IrohConnector {
    /// Build a connector over an established Iroh connection with the local
    /// (device) Noise static and the remote (Core) Noise static public key.
    pub fn new(
        conn: Connection,
        local_static: Arc<concerto_identity::NoiseStatic>,
        remote_static_pub: [u8; 32],
    ) -> Self {
        Self {
            conn,
            local_static,
            remote_static_pub,
        }
    }
}

// ===========================================================================
// endpoint.rs surface — the handle Task 217 wraps (`design/11 §5.1`)
// ===========================================================================

/// How a caller supplies the Core's gRPC service set to the serve loop without
/// the transport crate depending on `concerto-core`. The Core's `serve_iroh`
/// implements this over the **same** `Server::builder().add_service(..)` chain
/// `run_uds` uses, injecting `ConnTransport(TransportKind::Iroh)` (Task 201
/// seam). One call == one gRPC connection over one Noise-wrapped API stream
/// (gotcha #2). FROZEN shape so 217's façade + 213/214/215 reuse it.
pub trait ApiDispatcher: Send + Sync + 'static {
    /// Serve exactly one gRPC connection over the established Noise-wrapped
    /// duplex until the stream closes. Errors are logged by the serve loop.
    fn serve_connection(
        &self,
        io: NoiseDuplex,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<(), TransportError>> + Send>>;
}

/// Where the Core reaches its relay + an optional directly-supplied peer addr
/// (`design/11 §3.1`, §8; spike Note B). FROZEN config the Core actor builds.
#[derive(Debug, Clone, Default)]
pub struct TransportConfig {
    /// The relay URL to register with. `None` → Iroh's default relay map.
    /// Ignored entirely under `disable_remote`.
    pub relay_url: Option<String>,
    /// LAN-only mode (`managed.json.disable_remote`, Task 211 / `design/11
    /// §6.4`). `true` → no relay registration, LAN connections only. mDNS
    /// publication (Task 213) is unaffected.
    pub disable_remote: bool,
    /// A directly-supplied Core address (`host:port`) so a blocked-DNS client
    /// can connect without DNS/pkarr discovery (spike Note B). Ignored when
    /// empty; a malformed value is a [`TransportError::Endpoint`] at `start`.
    pub direct_addr: Option<String>,
}

/// The current relay association (`design/11 §5.1`, Task 217
/// `current_relay`/`switch_relay`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayInfo {
    /// The relay URL the endpoint uses, or `None` under `disable_remote` / when
    /// no relay is set.
    pub url: Option<String>,
    /// Whether remote (relay) access is disabled by managed policy.
    pub remote_disabled: bool,
}

/// A push-hint the transport queues toward a device (`design/11 §3.3`,
/// `design/14`). Task 217's `send_wakeup_hint` enqueues these; APNs/FCM delivery
/// is Task 14. ID-only payload — no PII on the wire (`design/14`).
#[derive(Debug, Clone)]
pub struct WakeupHint {
    /// The device to wake.
    pub device_id: DeviceId,
    /// Opaque, ID-only payload.
    pub payload: Vec<u8>,
}

/// A short-lived pairing listener (`design/11 §3.3` / §4 `pairing_listener`).
/// Opened by [`IrohTransport::listen_pairing`] for one device, gated by the
/// 32-byte token hash; the serve loop routes [`ChannelTag::Pairing`] raw
/// duplexes to [`Self::accept`]. Task 207 drives the Noise XX over the token
/// inside the delivered duplex; Task 217 calls `listen_pairing` / `close`.
pub struct PairingListener {
    pub(crate) token_hash: [u8; 32],
    pub(crate) rx: mpsc::Receiver<IrohDuplex>,
}

impl PairingListener {
    /// The token hash this listener is bound to (`design/12 §3.3`).
    pub fn token_hash(&self) -> [u8; 32] {
        self.token_hash
    }

    /// Await the next inbound pairing-channel duplex (Task 207 then runs the
    /// Noise XX token handshake over it). `None` when the listener is closed.
    pub async fn accept(&mut self) -> Option<IrohDuplex> {
        self.rx.recv().await
    }
}

/// The owning transport handle — the internals Task 217's `TransportHandle`
/// wraps (`design/11 §5.1`). Holds the one Iroh endpoint, the session registry
/// ([`TransportState`]), the current relay association, the pairing-listener
/// slot, and the push-hint channel. **FROZEN public surface**; method impls in
/// [`crate::endpoint`].
pub struct IrohTransport {
    pub(crate) endpoint: iroh::Endpoint,
    pub(crate) state: Arc<Mutex<TransportState>>,
    pub(crate) relay: Arc<Mutex<RelayInfo>>,
    pub(crate) config: TransportConfig,
    pub(crate) core_static: Arc<concerto_identity::NoiseStatic>,
    #[allow(clippy::type_complexity)]
    pub(crate) pairing_tx: Arc<Mutex<Option<([u8; 32], mpsc::Sender<IrohDuplex>)>>>,
    pub(crate) wakeup_tx: mpsc::UnboundedSender<WakeupHint>,
    pub(crate) wakeup_rx: Arc<Mutex<Option<mpsc::UnboundedReceiver<WakeupHint>>>>,
    pub(crate) shutdown: CancellationToken,
}
