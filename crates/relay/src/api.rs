//! The FROZEN public surface of `concerto-relay` (Task 214).
//!
//! Per the keychain/identity convention (`tasks/v1.0/205`,
//! `crates/identity/src/api.rs`, `crates/transport/src/api.rs`), this file is
//! what `scripts/regen-interfaces.sh` reads to produce the `concerto-relay`
//! section of `docs/interfaces/rust-api.md`. The canonical **type
//! declarations** (the `pub struct` / `pub enum` the generator indexes) are
//! declared **directly here**; their heavier method bodies live in the topic
//! modules ([`crate::config`], [`crate::state`], [`crate::metrics`],
//! [`crate::relay`]) as `impl` blocks / free functions.
//!
//! # Why this surface is frozen
//!
//! Task 215's WSS bridge wraps the **same binary**: it builds + runs the relay
//! from [`RelayConfig`] via [`Relay::start`], then adds the WSS↔Iroh bridge on
//! the reserved [`RelayConfig::wss_listen_addr`]. Freezing the lib entry point,
//! the env-var config surface, the Prometheus **metric names**, and the
//! [`RelayState`] / [`EndpointRoute`] field layout here lets 215 extend
//! [`RelayState::wss_bridges`] and the binary without a re-lock.
//!
//! ## FROZEN env-var config surface (`design/11 §6.3`, Twelve-Factor)
//!
//! - `RELAY_LISTEN_ADDR` — the Iroh-relay HTTP bind address.
//! - `WSS_LISTEN_ADDR` — **reserved for Task 215** (parsed + validated here; the
//!   bridge itself is 215). See [`RelayConfig::wss_listen_addr`].
//! - `MAX_ROUTES` — routing-table cap (`design/11 §6.3`: a node handles
//!   10k–50k routes).
//! - `BANDWIDTH_CAP_PER_ENDPOINT` — per-endpoint forwarded-byte cap.
//! - `PROMETHEUS_LISTEN_ADDR` — where the `/metrics` endpoint is served.
//!
//! ## FROZEN Prometheus metric names (`design/11 §6.3`)
//!
//! See [`crate::metrics`] for the exact constants. Additions are append-only;
//! dashboards/alerts key off these names:
//!
//! - `concerto_relay_routes` (gauge) — current routing-table size.
//! - `concerto_relay_bytes_forwarded_total` (counter) — total ciphertext bytes
//!   forwarded.
//! - `concerto_relay_holepunch_success_total{region=...}` (counter) — hole-punch
//!   successes, labelled by region.
//! - `concerto_relay_holepunch_attempt_total{region=...}` (counter) — hole-punch
//!   attempts, labelled by region (the denominator for the success *rate*).
//! - `concerto_relay_routes_rejected_total` (counter) — registrations refused at
//!   the `MAX_ROUTES` cap.
//! - `concerto_relay_bandwidth_capped_total` (counter) — forwards refused at the
//!   per-endpoint bandwidth cap.
//! - `concerto_relay_up` (gauge) — `1` while the relay is running.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// The TTL on a routing-table entry (`design/11 §3.2`, §4): an endpoint's route
/// expires 90 s after its last keep-alive. The endpoint refreshes ~every minute
/// (`design/11 §3.2`), so a live endpoint always stays inside this window.
/// **FROZEN** (the design's "90 s TTL").
pub const ROUTE_TTL: Duration = Duration::from_secs(90);

/// Default Iroh-relay HTTP bind address when `RELAY_LISTEN_ADDR` is unset
/// (`design/11 §6.3` notes "normally port 80"; we default to all-interfaces:80
/// for the Docker/Fly deploy, overridden by env in tests/CI to loopback:0).
pub const DEFAULT_RELAY_LISTEN_ADDR: &str = "0.0.0.0:80";

/// Default Prometheus bind address when `PROMETHEUS_LISTEN_ADDR` is unset.
pub const DEFAULT_PROMETHEUS_LISTEN_ADDR: &str = "0.0.0.0:9090";

/// Default routing-table cap when `MAX_ROUTES` is unset (`design/11 §6.3`: a
/// single node handles 10k–50k routes; the conservative default is 50k).
pub const DEFAULT_MAX_ROUTES: usize = 50_000;

// ===========================================================================
// config.rs surface — the FROZEN Twelve-Factor env-var config (`design/11 §6.3`)
// ===========================================================================

/// The relay's complete configuration, parsed **only** from environment
/// variables (Twelve-Factor — no config file, no flags beyond `--help` /
/// `--version`; `design/11 §6.3`). [`RelayConfig::from_env`] parses + validates
/// once at startup and fails fast with a precise message naming the bad var.
///
/// **FROZEN env-var surface** — operators script against these names; new vars
/// are additive. Task 215 reads [`Self::wss_listen_addr`] to add the WSS bridge.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// `RELAY_LISTEN_ADDR` — the Iroh-relay HTTP server bind address (the relay
    /// protocol endpoint, `design/11 §3.2`). Defaults to
    /// [`DEFAULT_RELAY_LISTEN_ADDR`].
    pub relay_listen_addr: SocketAddr,
    /// `WSS_LISTEN_ADDR` — **reserved for Task 215** (the WSS↔Iroh bridge,
    /// `design/11 §3.4`). Parsed + validated here so the env surface is frozen
    /// and a malformed value fails fast, but this task does **not** stand up a
    /// WSS listener. `None` when unset.
    pub wss_listen_addr: Option<SocketAddr>,
    /// `MAX_ROUTES` — the routing-table cap (`design/11 §6.3`). Registrations
    /// beyond it are rejected (and counted). Defaults to [`DEFAULT_MAX_ROUTES`].
    pub max_routes: usize,
    /// `BANDWIDTH_CAP_PER_ENDPOINT` — max forwarded bytes per endpoint
    /// (`design/11 §6.3`, §3.9). `None` (unset) → unlimited. `0` is rejected as
    /// malformed (a zero cap would forward nothing).
    pub bandwidth_cap_per_endpoint: Option<u64>,
    /// `PROMETHEUS_LISTEN_ADDR` — where the `/metrics` endpoint is served
    /// (`design/11 §6.3`). Defaults to [`DEFAULT_PROMETHEUS_LISTEN_ADDR`].
    pub prometheus_listen_addr: SocketAddr,
}

// ===========================================================================
// state.rs surface — the in-memory routing model (`design/11 §4`)
// ===========================================================================

/// One endpoint's routing entry (`design/11 §4`). **FROZEN field layout.**
/// Registered when the endpoint announces itself, `expires_at` refreshed on each
/// keep-alive (90 s TTL — [`ROUTE_TTL`]), evicted on expiry.
#[derive(Debug, Clone)]
pub struct EndpointRoute {
    /// The endpoint's current public IP + port (`design/11 §4`: `endpoint_id →
    /// current public IP+port`).
    pub public_addr: SocketAddr,
    /// When this route was last refreshed by a keep-alive.
    pub last_seen: Instant,
    /// When this route expires absent a refresh (`last_seen + `[`ROUTE_TTL`]).
    pub expires_at: Instant,
}

/// Per-endpoint forwarded-byte accounting (`design/11 §4` `bandwidth_counters`,
/// §3.9 ciphertext-only). Counts **only** byte totals — never payload — so the
/// relay's observable surface stays metadata-only (`design/11 §3.9`).
#[derive(Debug, Clone, Default)]
pub struct BandwidthCounter {
    /// Total ciphertext bytes forwarded for this endpoint since registration.
    pub bytes_forwarded: u64,
}

/// The relay's entire in-memory state (`design/11 §4`). **FROZEN field layout.**
/// Stateless except these per-endpoint entries (`design/11 §3.2`); no SQLite, no
/// Redis (Redis is V2.0 per `design/11 §3.2`). Task 215 fills
/// [`Self::wss_bridges`].
#[derive(Debug, Default)]
pub struct RelayState {
    /// The routing table: `iroh_endpoint_id → current route` (`design/11 §4`).
    /// Keyed by the endpoint id string (the relay never parses the Iroh key
    /// material — it only routes by id, `design/11 §3.9`).
    pub routes: HashMap<String, EndpointRoute>,
    /// Per-endpoint forwarded-byte counters (`design/11 §4`).
    pub bandwidth_counters: HashMap<String, BandwidthCounter>,
    /// **Reserved for Task 215** — the active WSS↔Iroh bridges
    /// (`design/11 §3.4`, §4 `wss_bridges`). Keyed by bridge id. Empty in V1.0
    /// Task 214; 215 populates it.
    pub wss_bridges: HashMap<String, WssBridge>,
}

/// A live WSS↔Iroh bridge entry (`design/11 §3.4`, §4 `wss_bridges`). One per
/// open browser WSS connection (Task 215). Bridges are **ephemeral** — created on
/// a successful `/wss/<endpoint_id>` upgrade, torn down on disconnect — so this
/// entry carries only the §3.9-permitted metadata: the bridge id, the addressed
/// endpoint id, and the source IP. **No payload, no inner-frame structure, no
/// device credential** ever reaches this struct (the relay sees ciphertext only,
/// `design/11 §3.9`); the byte *counts* live on [`BandwidthCounter`], keyed by
/// `endpoint_id`. Field additions are append-only.
#[derive(Debug, Clone)]
pub struct WssBridge {
    /// The bridge's identifier (the relay-side handle the bridge is keyed on in
    /// [`RelayState::wss_bridges`]). A fresh opaque id per browser connection.
    pub bridge_id: String,
    /// The addressed Iroh endpoint id parsed from the `/wss/<endpoint_id>` path
    /// (`design/11 §3.4`) — the Core this bridge forwards to. Routing metadata
    /// (`design/11 §3.9`: "Iroh endpoint ID being addressed" is observable).
    pub endpoint_id: String,
    /// The source IP+port of the browser's WSS connection (`design/11 §3.9`:
    /// "source IP of a connecting client" is observable — QUIC/TCP headers a
    /// network observer sees anyway). Metadata only.
    pub peer_addr: SocketAddr,
}

// ===========================================================================
// wss.rs surface — the WSS↔Iroh bridge (`design/11 §3.4` Path B, Task 215)
// ===========================================================================

/// The maximum length (bytes) of the `<endpoint_id>` segment accepted on the
/// FROZEN `/wss/<endpoint_id>` path before the upgrade is refused with HTTP 4xx
/// (`design/11 §3.4`). An Iroh endpoint id is a 32-byte Ed25519 key rendered as
/// 64 lowercase hex chars; the ceiling is generous (a few multiples) so the parse
/// — never the length — is the real validator, while still rejecting an
/// oversized path before any work. **FROZEN** (the Phase-5 web client builds
/// `wss://<relay-host>/wss/<endpoint_id>` with the hex id).
pub const MAX_ENDPOINT_ID_LEN: usize = 128;

/// The FROZEN URL path prefix the WSS bridge upgrades on: `/wss/<endpoint_id>`
/// (`design/11 §3.4`). The Phase-5 web client (Task 521) constructs exactly
/// `wss://<relay-host>/wss/<endpoint_id>`; changing this prefix is a wire break
/// across the relay↔web-client boundary. **FROZEN.**
pub const WSS_PATH_PREFIX: &str = "/wss/";

/// The TLS material the WSS bridge terminates the browser's `wss://` connection
/// with (`design/11 §3.4`: "TLS terminated at the relay"). The inner Noise IK
/// (`design/12 §3.4`), established by the browser *inside* the WSS stream, is what
/// protects payload confidentiality end-to-end; this is only the **outer**
/// transport hop the browser's `wss://` scheme requires, so the relay can read
/// **ciphertext only** (`design/11 §3.9`).
///
/// PEM-encoded so an operator supplies their own cert/key (Fly.io / their own
/// CA). [`WssTlsConfig::self_signed`] generates an ephemeral self-signed pair for
/// the Tier-2 loopback double (and as a last-resort dev fallback) — production
/// deploys supply a real cert. **FROZEN shape** (fields append-only).
#[derive(Clone)]
pub struct WssTlsConfig {
    /// The PEM-encoded certificate chain (leaf first).
    pub cert_pem: Vec<u8>,
    /// The PEM-encoded PKCS#8 private key.
    pub key_pem: Vec<u8>,
}

/// A running WSS↔Iroh bridge listener (`design/11 §3.4` Path B, §6.2 the `Wss`
/// node) — the only non-Iroh path the relay runs. Browsers cannot speak Iroh
/// natively in V1.0, so per browser WSS connection on `/wss/<endpoint_id>` this
/// bridge opens **one** Iroh bidi stream to the addressed Core and pumps opaque
/// bytes both ways (WSS binary-frame payload → Iroh `SendStream`; Iroh
/// `RecvStream` bytes → WSS binary frame). The pump is **transparent**: it copies
/// `&[u8]` and never parses, decodes, decrypts, or logs frame contents — the
/// browser does Noise IK inside the stream with its device cert and the relay
/// sees **ciphertext only** (`design/11 §3.9`).
///
/// Started by [`crate::wss::WssBridgeServer::start`] when
/// [`RelayConfig::wss_listen_addr`] (the reserved `WSS_LISTEN_ADDR`) is set; the
/// `concerto-relay` binary spawns it alongside [`Relay`]. Method impls live in
/// [`crate::wss`].
pub struct WssBridgeServer {
    pub(crate) inner: crate::wss::WssBridgeInner,
}

/// The WSS-bridge Prometheus metrics handle (`design/11 §3.9` metadata-only — the
/// FROZEN `concerto_relay_*` names live in [`crate::metrics`]). Tracks the live
/// bridge gauge and the per-direction forwarded-byte counters (**byte counts
/// only**, never content). Cloneable into each per-connection pump. Method impls
/// live in [`crate::metrics`].
#[derive(Clone)]
pub struct WssBridgeMetrics {
    pub(crate) inner: crate::metrics::WssMetricsInner,
}

// ===========================================================================
// relay.rs surface — the FROZEN lib entry point (Task 215 wraps this)
// ===========================================================================

/// A running self-hosted relay (`design/11 §3.2`, §6.2) — **the FROZEN library
/// entry point Task 215 wraps**. Embeds Iroh's `iroh-relay` server (no new
/// protocol — `design/11 §12 R-7`) for the actual hole-punch assist + relayed
/// QUIC forwarding, and owns the [`RelayState`] routing-table observability, the
/// per-endpoint bandwidth caps, and the Prometheus endpoint
/// ([`crate::metrics`]).
///
/// Build + run it from a [`RelayConfig`] with [`Relay::start`]; drive a clean
/// shutdown with [`Relay::shutdown`]. Task 215's WSS bridge calls the same
/// [`Relay::start`] and adds a listener on [`RelayConfig::wss_listen_addr`].
///
/// Method impls live in [`crate::relay`].
pub struct Relay {
    pub(crate) inner: crate::relay::RelayInner,
}
