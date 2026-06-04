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
use std::net::IpAddr;
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
// mdns.rs surface — LAN discovery (`design/11 §3.5`, Task 213)
// ===========================================================================

/// The mDNS service type the Core advertises and clients browse for
/// (`design/11 §3.5`). **FROZEN** — mobile (Task 511) and web (Task 521) browse
/// for this exact string. The trailing `.local.` is the mDNS convention
/// (`mdns-sd` requires the fully-qualified form).
pub const SERVICE_TYPE: &str = "_concerto._tcp.local.";

/// TXT key for the Iroh endpoint id the client dials (`design/11 §3.5`).
/// **FROZEN.**
pub const TXT_ENDPOINT_ID: &str = "endpoint_id";
/// TXT key for the base64 Ed25519 Core public key — a fingerprint hint, NOT an
/// auth credential (`design/11 §3.5`, `design/12 §3.6`). **FROZEN.**
pub const TXT_CORE_PUBKEY: &str = "core_pubkey";
/// TXT key for the Core semver (`design/11 §3.5`). **FROZEN.**
pub const TXT_VERSION: &str = "version";
/// TXT key for the comma-separated coarse feature list (`design/11 §3.5`).
/// **FROZEN.** New keys are append-only; values within `caps` grow freely.
pub const TXT_CAPS: &str = "caps";

/// A Core discovered on the LAN via mDNS — the parsed `_concerto._tcp.local` TXT
/// record plus the resolved addresses (`design/11 §3.5`). The descriptor
/// Task 218/219 (Desktop Connect-to-Core picker) and Task 511 (mobile pairing)
/// consume: the client hands [`Self::endpoint_id`] to the Task-212 connect path
/// ([`connect_channel`]) to open Iroh **directly** on the LAN
/// ([`ConnectionPath::Lan`], no relay). **FROZEN shape** (fields are
/// append-only).
///
/// The `core_pubkey_b64` is a fingerprint *hint* only — discovery is
/// unauthenticated advertisement; trust is still established by the QR/cert
/// pairing flow (Task 207).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredCore {
    /// The advertised instance label (the Core's display name).
    pub instance_name: String,
    /// The Iroh endpoint id to dial (TXT `endpoint_id`).
    pub endpoint_id: String,
    /// The base64 Ed25519 Core public key (TXT `core_pubkey`) — fingerprint hint.
    pub core_pubkey_b64: String,
    /// The Core semver (TXT `version`).
    pub version: String,
    /// The raw comma-separated coarse feature list (TXT `caps`); see
    /// [`Self::caps_list`].
    pub caps: String,
    /// The resolved IP addresses (IPv4 + IPv6, R-3) — informational; the dial
    /// uses `endpoint_id` via Iroh, not these directly.
    pub addresses: Vec<IpAddr>,
}

/// The values the responder stamps into the `_concerto._tcp.local` advertisement
/// (`design/11 §3.5`). The Core actor (Task 217) builds this from the live
/// transport: `endpoint_id` from [`IrohTransport::endpoint_id`], `core_pubkey`
/// from the Core Ed25519 identity (Task 206), `version` from the Core version,
/// `caps` mirroring the coarse `ServerCapabilities` feature surface (Task 201).
///
/// `opt_out` is the dedicated managed / per-network mDNS suppression
/// (`design/11 §3.5`) — **orthogonal to `disable_remote`** (which leaves mDNS
/// publishing, `design/11 §6.4`).
#[derive(Debug, Clone)]
pub struct MdnsConfig {
    /// The advertised instance label (the Core's display name).
    pub instance_name: String,
    /// The Iroh endpoint id (TXT `endpoint_id`).
    pub endpoint_id: String,
    /// The base64 Ed25519 Core public key (TXT `core_pubkey`).
    pub core_pubkey_b64: String,
    /// The Core semver (TXT `version`).
    pub version: String,
    /// The comma-separated coarse feature list (TXT `caps`).
    pub caps: String,
    /// The advertised port (informational; the Iroh endpoint id is the real
    /// dial target). Carried for SRV-record completeness.
    pub port: u16,
    /// Host IPs to advertise (both v4 and v6 where available, R-3). Empty → the
    /// responder auto-detects and keeps the host's addresses updated.
    pub addrs: Vec<IpAddr>,
    /// Suppress publication entirely (the dedicated mDNS opt-out). When `true`,
    /// [`MdnsResponder::publish`] returns a no-op handle that advertises
    /// nothing. **NOT** driven by `disable_remote`.
    pub opt_out: bool,
}

/// The Core's mDNS responder — owns the `mdns-sd` daemon and the registered
/// service instance (`design/11 §3.5`, §4 `mdns_responder`). Held alongside the
/// Iroh endpoint in [`crate::api::TransportState`]'s owning transport; published
/// after the endpoint is up (it needs the `endpoint_id`) and deregistered (mDNS
/// goodbye) on shutdown/drop. When the opt-out is set the handle holds no daemon
/// and advertises nothing. Method impls in [`crate::mdns`].
pub struct MdnsResponder {
    pub(crate) daemon: Option<mdns_sd::ServiceDaemon>,
    pub(crate) fullname: Option<String>,
    pub(crate) config: MdnsConfig,
}

/// The client-side mDNS browser — owns the `mdns-sd` browse daemon and yields
/// [`DiscoveredCore`] descriptors as `_concerto._tcp.local` services resolve
/// (`design/11 §3.5`). The discovery helper Task 218/219/511 drive; they feed
/// each discovered `endpoint_id` to the Task-212 LAN connect path. Method impls
/// in [`crate::mdns`].
pub struct MdnsBrowser {
    pub(crate) daemon: Option<mdns_sd::ServiceDaemon>,
    pub(crate) rx: mpsc::UnboundedReceiver<DiscoveredCore>,
}

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

/// The kind of client a session belongs to (`design/11 §2`). NAT-success is
/// **broken out by this** so we can see whether split-host Desktops (mostly
/// residential↔residential or residential↔cloud-VM) get worse direct rates than
/// mobile (`design/11 §2` V1.0 note). **FROZEN value set** —
/// { desktop-split-host, mobile, web } — and the closed enum the by-kind
/// [`NatStats`] map keys on. The connecting client's kind is known at session
/// establishment (the device/connect metadata, `design/11 §3.1`); web reaches
/// the Core via the WSS bridge (Task 215) and counts as [`ClientKind::Web`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientKind {
    /// Split-host Desktop (Tauri shell on a different machine), over Iroh.
    DesktopSplitHost,
    /// Mobile (iOS / Android via the RN Iroh native module), over Iroh.
    Mobile,
    /// Web (browser), reaching the Core via the WSS bridge (Task 215).
    Web,
}

impl ClientKind {
    /// The canonical name string used as the by-client-kind map key on the wire
    /// (`NatStats.by_client_kind`) — the `ClientKind` proto enum value name. A
    /// proto map key cannot be an enum, so the Core keys the proto map on this.
    /// **FROZEN strings.**
    pub fn as_key(self) -> &'static str {
        match self {
            ClientKind::DesktopSplitHost => "CLIENT_KIND_DESKTOP_SPLIT_HOST",
            ClientKind::Mobile => "CLIENT_KIND_MOBILE",
            ClientKind::Web => "CLIENT_KIND_WEB",
        }
    }
}

/// Per-bucket NAT-traversal counters (`design/11 §4`): how many sessions in this
/// bucket came up direct / relayed / lan. The aggregate [`NatStats`] holds one
/// implicitly; the `by_network_class` + `by_client_kind` maps hold one per key.
/// **FROZEN field layout** — the Core maps this 1:1 to the `NetworkStats` proto.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetworkStats {
    /// Sessions that came up on a direct (hole-punched) path.
    pub direct: u32,
    /// Sessions that came up relayed.
    pub relayed: u32,
    /// Sessions that came up LAN-direct.
    pub lan: u32,
}

impl NetworkStats {
    /// Increment the counter for one session's path.
    pub fn record(&mut self, path: ConnectionPath) {
        match path {
            ConnectionPath::Direct => self.direct = self.direct.saturating_add(1),
            ConnectionPath::Relayed => self.relayed = self.relayed.saturating_add(1),
            ConnectionPath::Lan => self.lan = self.lan.saturating_add(1),
        }
    }

    /// Direct + LAN sessions (the "did not need a relay" numerator for the
    /// direct-% used by `nat_success_changed`).
    pub fn direct_or_lan(&self) -> u32 {
        self.direct.saturating_add(self.lan)
    }

    /// Total sessions counted in this bucket.
    pub fn total(&self) -> u32 {
        self.direct
            .saturating_add(self.relayed)
            .saturating_add(self.lan)
    }
}

/// Daily NAT-traversal counters (`design/11 §4`, §3.6), broken out **by network
/// class and by client kind** (Task 216). Keeps the `design/11 §4` canonical
/// field names (`direct_today` / `relayed_today` / `by_network_class`) and adds
/// the V1.0 by-client-kind split (`design/11 §2`). **FROZEN shape** — the Core's
/// `GetNatStats` Runtime RPC (D1) maps this 1:1 to the `NatStats` proto the
/// Desktop badge + Diagnostics percentage read.
#[derive(Debug, Clone, Default)]
pub struct NatStats {
    /// Sessions that came up on a direct (hole-punched) path today.
    pub direct_today: u32,
    /// Sessions that came up relayed today.
    pub relayed_today: u32,
    /// Sessions that came up LAN-direct today.
    pub lan_today: u32,
    /// Per-network-class counters (`design/11 §4` `by_network_class`). Key is a
    /// coarse network label (`"wifi"` / `"cellular"` / `"ethernet"` / `"other"`).
    pub by_network_class: HashMap<String, NetworkStats>,
    /// Per-client-kind counters (`design/11 §2`). Keyed by [`ClientKind`] so the
    /// split-host-desktop-vs-mobile direct-rate gap is visible.
    pub by_client_kind: HashMap<ClientKind, NetworkStats>,
}

impl NatStats {
    /// Increment the counters for a newly-established session's path, attributing
    /// it to its `network_class` and `client_kind` buckets (`design/11 §3.6,
    /// §2`). The aggregate `*_today` counters and both maps advance together.
    pub fn record(&mut self, path: ConnectionPath, network_class: &str, client_kind: ClientKind) {
        match path {
            ConnectionPath::Direct => self.direct_today = self.direct_today.saturating_add(1),
            ConnectionPath::Relayed => self.relayed_today = self.relayed_today.saturating_add(1),
            ConnectionPath::Lan => self.lan_today = self.lan_today.saturating_add(1),
        }
        self.by_network_class
            .entry(network_class.to_string())
            .or_default()
            .record(path);
        self.by_client_kind
            .entry(client_kind)
            .or_default()
            .record(path);
    }

    /// The rolling direct-connection percentage (0..=100) across all sessions
    /// today: `(direct + lan) / total`. The numerator counts both hole-punched
    /// and LAN-direct as "did not need a relay" (`design/11 §3.6` — the badge is
    /// direct-vs-via-relay). Zero sessions → 0%.
    pub fn direct_percent(&self) -> u32 {
        let total = self
            .direct_today
            .saturating_add(self.relayed_today)
            .saturating_add(self.lan_today);
        if total == 0 {
            return 0;
        }
        let direct = self.direct_today.saturating_add(self.lan_today);
        ((u64::from(direct) * 100) / u64::from(total)) as u32
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
    /// The kind of client this session belongs to (`design/11 §2`) — drives the
    /// by-client-kind NAT breakdown. Known at session establishment.
    pub client_kind: ClientKind,
    /// Last time a stream on this connection was seen (liveness / idle GC).
    pub last_seen: Instant,
}

impl ActiveSession {
    /// Build a session from a freshly-accepted/-opened Iroh connection,
    /// classifying its path now and attributing it to `client_kind`.
    pub fn new(device_id: DeviceId, iroh_connection: Connection, client_kind: ClientKind) -> Self {
        let path = classify_path(&iroh_connection);
        Self {
            device_id,
            iroh_connection,
            path,
            client_kind,
            last_seen: Instant::now(),
        }
    }

    /// Re-classify [`Self::path`] from the connection's current selected path
    /// (216 calls this as paths migrate — the migration contract, `design/11
    /// §3.7`). Returns the new path. A path change here updates the session in
    /// place; it does **NOT** close it (only a true connection drop does).
    pub fn refresh_path(&mut self) -> ConnectionPath {
        self.path = classify_path(&self.iroh_connection);
        self.last_seen = Instant::now();
        self.path
    }

    /// The coarse network class of this session's current path (`design/11
    /// §3.6`), used as the `by_network_class` key. The transport cannot read the
    /// client's NIC, so it classifies by the path it observes: a relayed path is
    /// `"cellular"`-ish from the Core's view only as `"relayed"`; the honest
    /// label is the path class. V1.0 maps Direct→`"direct"`, Relayed→`"relayed"`,
    /// Lan→`"lan"` — the network-class dimension the Core can actually attest.
    /// (A richer client-reported class is a later task; noted in Handoff.)
    pub fn network_class(&self) -> &'static str {
        network_class_for(self.path)
    }
}

/// The coarse `by_network_class` key for a path (`design/11 §3.6, §4`). The Core
/// labels by the path it can attest (it cannot see the client's NIC), so the
/// class mirrors the path: `direct` / `relayed` / `lan`. FROZEN key strings.
pub fn network_class_for(path: ConnectionPath) -> &'static str {
    match path {
        ConnectionPath::Direct => "direct",
        ConnectionPath::Relayed => "relayed",
        ConnectionPath::Lan => "lan",
    }
}

/// A proto-free transport-lifecycle telemetry event (`design/11 §5.3`). The
/// transport broadcasts these as it observes sessions open/close, the relay
/// switch, and the rolling NAT-success rate move; the Core ([`crate::api`]'s
/// consumer in `concerto-core`) maps each into the `streams.proto`
/// `Event { TransportEvent }` arm (D1: no `transport.proto`) and fans it out on
/// the `transport.events` subject. Kept proto-free so the transport stays a thin
/// leaf (no `concerto-proto` dep). **FROZEN** — the Core's mapper matches on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportTelemetry {
    /// A device established a session (`transport.session_opened`).
    SessionOpened {
        /// The device id (the session key).
        device_id: DeviceId,
        /// The path the session came up on.
        path: ConnectionPath,
        /// The kind of client.
        client_kind: ClientKind,
    },
    /// A device disconnected — a TRUE connection drop, NOT a path migration
    /// (`transport.session_closed`). Migration updates `path` in place and does
    /// NOT emit this (`design/11 §3.7`, §7.4 — the FROZEN migration contract).
    SessionClosed {
        /// The device id whose session dropped.
        device_id: DeviceId,
    },
    /// The relay URL the Core registers with changed (`transport.relay_switched`).
    RelaySwitched {
        /// The new relay URL; empty under `disable_remote` / no relay.
        relay_url: String,
    },
    /// The rolling 1-hour direct-% materially changed (`transport.nat_success_changed`).
    /// Debounced (see [`NAT_SUCCESS_DELTA_PCT`] / the 70% PRD line).
    NatSuccessChanged {
        /// The new rolling direct-% (0..=100).
        direct_percent: u32,
    },
}

/// The debounce threshold for [`TransportTelemetry::NatSuccessChanged`]
/// (`design/11 §5.3`, §3.6): emit only when the rolling direct-% moves by **≥
/// this many percentage points** since the last emission, OR when it crosses the
/// PRD §22.3 70% direct line in either direction. A named constant per the task's
/// "don't gold-plate a statistics engine" note — a simple hysteresis, not a stats
/// engine. **FROZEN.**
pub const NAT_SUCCESS_DELTA_PCT: u32 = 5;

/// The PRD §22.3 target the [`TransportTelemetry::NatSuccessChanged`] debounce
/// also fires on crossing (`design/11 §3.6`): > 70% direct. **FROZEN.**
pub const NAT_SUCCESS_PRD_LINE_PCT: u32 = 70;

/// Whether a direct-% change from `prev` to `now` is "material" enough to emit a
/// [`TransportTelemetry::NatSuccessChanged`] (`design/11 §5.3` debounce): a move
/// of ≥ [`NAT_SUCCESS_DELTA_PCT`] points, or a crossing of the
/// [`NAT_SUCCESS_PRD_LINE_PCT`] line.
pub fn nat_success_is_material(prev: u32, now: u32) -> bool {
    let delta = prev.abs_diff(now);
    let crossed_line = (prev <= NAT_SUCCESS_PRD_LINE_PCT) != (now <= NAT_SUCCESS_PRD_LINE_PCT);
    delta >= NAT_SUCCESS_DELTA_PCT || crossed_line
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
    /// The last rolling direct-% the transport emitted a `NatSuccessChanged`
    /// for, so the debounce ([`nat_success_is_material`]) is exact. `None` until
    /// the first session (so the first session emits the initial rate).
    pub last_nat_percent: Option<u32>,
}

impl TransportState {
    /// A fresh, empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record (or replace) a session and bump the NAT counter for its path,
    /// network class, and client kind.
    pub fn insert_session(&mut self, session: ActiveSession) {
        self.nat_stats
            .record(session.path, session.network_class(), session.client_kind);
        self.sessions.insert(session.device_id.clone(), session);
    }

    /// Apply a client **path change** (the migration contract, `design/11 §3.7`,
    /// §7.4): re-classify the live session's path in place. This does **NOT**
    /// remove the session or count a new NAT outcome — a migration is one
    /// session, already counted at open; only its `path` (and `last_seen`)
    /// update. Returns the new path, or `None` if no such session is live.
    ///
    /// The QUIC connection-id is preserved by Iroh/QUIC natively across the path
    /// change; the Core-side work is exactly to NOT treat this as a disconnect.
    pub fn note_path_change(&mut self, device_id: &DeviceId) -> Option<ConnectionPath> {
        self.sessions.get_mut(device_id).map(|s| s.refresh_path())
    }

    /// Whether a session for `device_id` is currently live (a migration leaves it
    /// live; a true drop removes it). The seam the §5.3 events promise.
    pub fn has_session(&self, device_id: &DeviceId) -> bool {
        self.sessions.contains_key(device_id)
    }

    /// Remove every session for `device_id`, closing each Iroh connection
    /// (`design/12 §7.3`). Idempotent. This is a TRUE drop (revocation /
    /// disconnect), the only path that ends a session — NOT a migration.
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
/// slot, the push-hint channel, and the mDNS responder ([`MdnsResponder`],
/// `design/11 §4` `mdns_responder` — owned alongside the endpoint so it starts/
/// stops with the transport). **FROZEN public surface**; method impls in
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
    /// The live mDNS responder (`design/11 §4` `mdns_responder`). `None` until
    /// [`IrohTransport::publish_mdns`] is called (the Core publishes after the
    /// endpoint is up, since the TXT needs the `endpoint_id`). Replaced on
    /// re-announce; deregistered (mDNS goodbye) on [`IrohTransport::stop_mdns`]
    /// / drop.
    pub(crate) mdns: Arc<Mutex<Option<MdnsResponder>>>,
    /// The transport-lifecycle telemetry broadcast (`design/11 §5.3`, Task 216).
    /// The serve loop / relay switch publish [`TransportTelemetry`] here; the
    /// Core subscribes (via [`IrohTransport::subscribe_telemetry`]) and maps each
    /// into the `streams.proto` `Event { TransportEvent }` arm on the
    /// `transport.events` subject. Held as a `broadcast::Sender` so it can be
    /// published from spawned per-connection tasks with no receiver attached.
    pub(crate) telemetry_tx: tokio::sync::broadcast::Sender<TransportTelemetry>,
    pub(crate) shutdown: CancellationToken,
}
