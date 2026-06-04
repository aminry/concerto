//! Prometheus metrics (`design/11 §6.3`, Task 214) — the FROZEN metric names.
//!
//! The relay exposes a `/metrics` endpoint at `PROMETHEUS_LISTEN_ADDR` carrying
//! **only metadata** (`design/11 §3.9` ciphertext-only): route counts, forwarded
//! byte totals, hole-punch success/attempt counts by region, cap-rejection
//! counters, and an up gauge. No payload, no endpoint key material, no device
//! credentials ever reach a metric or a log.
//!
//! The metric **names** below are FROZEN (`design/11 §6.3`) — dashboards and
//! alerts key off them; additions are append-only. Task 215 may add WSS-bridge
//! metrics under the same `concerto_relay_*` prefix.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use prometheus::{Encoder, IntCounter, IntCounterVec, IntGauge, Opts, Registry, TextEncoder};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::api::WssBridgeMetrics;
use crate::error::{RelayError, Result};

// ---------------------------------------------------------------------------
// FROZEN metric names (`design/11 §6.3`). Operators' dashboards depend on these.
// ---------------------------------------------------------------------------

/// Current routing-table size (`design/11 §6.3` "routes count"). Gauge.
pub const METRIC_ROUTES: &str = "concerto_relay_routes";
/// Total ciphertext bytes forwarded (`design/11 §6.3` "bytes forwarded"). Counter.
pub const METRIC_BYTES_FORWARDED_TOTAL: &str = "concerto_relay_bytes_forwarded_total";
/// Hole-punch successes, labelled by region (`design/11 §6.3` "hole-punch
/// success rate per region" — the numerator). Counter vec.
pub const METRIC_HOLEPUNCH_SUCCESS_TOTAL: &str = "concerto_relay_holepunch_success_total";
/// Hole-punch attempts, labelled by region (the denominator for the *rate*).
/// Counter vec.
pub const METRIC_HOLEPUNCH_ATTEMPT_TOTAL: &str = "concerto_relay_holepunch_attempt_total";
/// Registrations refused at the `MAX_ROUTES` cap (`design/11 §6.3`). Counter.
pub const METRIC_ROUTES_REJECTED_TOTAL: &str = "concerto_relay_routes_rejected_total";
/// Forwards refused at the per-endpoint bandwidth cap (`design/11 §6.3`, §3.9).
/// Counter.
pub const METRIC_BANDWIDTH_CAPPED_TOTAL: &str = "concerto_relay_bandwidth_capped_total";
/// `1` while the relay is running, `0` after shutdown (basic up/health signal).
/// Gauge.
pub const METRIC_UP: &str = "concerto_relay_up";

/// Current number of live WSS↔Iroh bridges — one per open browser connection
/// (`design/11 §3.4`, Task 215). Gauge. **FROZEN** (append-only addition under
/// the `concerto_relay_*` prefix). Bridges are ephemeral; this rises on upgrade,
/// falls on teardown.
pub const METRIC_WSS_BRIDGES: &str = "concerto_relay_wss_bridges";
/// Total ciphertext bytes pumped across all WSS↔Iroh bridges, labelled by
/// direction (`design/11 §3.4`, §3.9 — byte *counts* only, never content).
/// Counter vec. **FROZEN** (append-only).
pub const METRIC_WSS_BYTES_FORWARDED_TOTAL: &str = "concerto_relay_wss_bytes_forwarded_total";

/// The label key for the region dimension on the hole-punch metrics
/// (`design/11 §6.3` "per region"). FROZEN.
pub const LABEL_REGION: &str = "region";

/// The label key for the direction dimension on the WSS bridge byte counter
/// (`design/11 §3.4`). FROZEN.
pub const LABEL_DIRECTION: &str = "direction";
/// Direction label value: WSS binary frame → Iroh stream (browser → Core).
pub const DIRECTION_TO_CORE: &str = "to_core";
/// Direction label value: Iroh stream → WSS binary frame (Core → browser).
pub const DIRECTION_TO_BROWSER: &str = "to_browser";

/// The relay's Prometheus metrics handles, cloneable into the routing-table and
/// forwarding paths. Backed by one [`Registry`] the `/metrics` endpoint encodes.
#[derive(Clone)]
pub struct RelayMetrics {
    registry: Registry,
    routes: IntGauge,
    bytes_forwarded_total: IntCounter,
    holepunch_success_total: IntCounterVec,
    holepunch_attempt_total: IntCounterVec,
    routes_rejected_total: IntCounter,
    bandwidth_capped_total: IntCounter,
    up: IntGauge,
    /// The last absolute bytes-forwarded total we synced from the embedded
    /// iroh-relay's counters; [`Self::set_bytes_forwarded`] increments the
    /// monotonic Prometheus counter by the delta so it stays a proper counter
    /// while being driven by an absolute source.
    bytes_forwarded_synced: Arc<AtomicU64>,
}

impl RelayMetrics {
    /// Register every FROZEN metric in a fresh registry.
    pub fn new() -> Result<Self> {
        let registry = Registry::new();

        let routes = IntGauge::with_opts(Opts::new(
            METRIC_ROUTES,
            "Current number of endpoint routes in the in-memory routing table.",
        ))
        .map_err(metrics_err)?;
        let bytes_forwarded_total = IntCounter::with_opts(Opts::new(
            METRIC_BYTES_FORWARDED_TOTAL,
            "Total ciphertext bytes forwarded by the relay (metadata only).",
        ))
        .map_err(metrics_err)?;
        let holepunch_success_total = IntCounterVec::new(
            Opts::new(
                METRIC_HOLEPUNCH_SUCCESS_TOTAL,
                "Hole-punch successes, labelled by region.",
            ),
            &[LABEL_REGION],
        )
        .map_err(metrics_err)?;
        let holepunch_attempt_total = IntCounterVec::new(
            Opts::new(
                METRIC_HOLEPUNCH_ATTEMPT_TOTAL,
                "Hole-punch attempts, labelled by region (rate denominator).",
            ),
            &[LABEL_REGION],
        )
        .map_err(metrics_err)?;
        let routes_rejected_total = IntCounter::with_opts(Opts::new(
            METRIC_ROUTES_REJECTED_TOTAL,
            "Registrations refused because the routing table is at MAX_ROUTES.",
        ))
        .map_err(metrics_err)?;
        let bandwidth_capped_total = IntCounter::with_opts(Opts::new(
            METRIC_BANDWIDTH_CAPPED_TOTAL,
            "Forwards refused because the endpoint hit BANDWIDTH_CAP_PER_ENDPOINT.",
        ))
        .map_err(metrics_err)?;
        let up = IntGauge::with_opts(Opts::new(
            METRIC_UP,
            "1 while the relay is running, 0 after shutdown.",
        ))
        .map_err(metrics_err)?;

        registry
            .register(Box::new(routes.clone()))
            .map_err(metrics_err)?;
        registry
            .register(Box::new(bytes_forwarded_total.clone()))
            .map_err(metrics_err)?;
        registry
            .register(Box::new(holepunch_success_total.clone()))
            .map_err(metrics_err)?;
        registry
            .register(Box::new(holepunch_attempt_total.clone()))
            .map_err(metrics_err)?;
        registry
            .register(Box::new(routes_rejected_total.clone()))
            .map_err(metrics_err)?;
        registry
            .register(Box::new(bandwidth_capped_total.clone()))
            .map_err(metrics_err)?;
        registry
            .register(Box::new(up.clone()))
            .map_err(metrics_err)?;

        Ok(Self {
            registry,
            routes,
            bytes_forwarded_total,
            holepunch_success_total,
            holepunch_attempt_total,
            routes_rejected_total,
            bandwidth_capped_total,
            up,
            bytes_forwarded_synced: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Set the current routing-table size gauge.
    pub fn set_routes(&self, n: usize) {
        self.routes.set(n as i64);
    }

    /// Set the total forwarded-bytes counter from an **absolute** source (the
    /// embedded iroh-relay's own monotonic byte counter). Internally increments
    /// the Prometheus counter by the delta since the last sync so it stays a
    /// well-formed monotonic counter. A non-increasing `absolute` (e.g. a relay
    /// restart resetting iroh's counter) is ignored (no decrement).
    pub fn set_bytes_forwarded(&self, absolute: u64) {
        let prev = self
            .bytes_forwarded_synced
            .swap(absolute, Ordering::Relaxed);
        if absolute > prev {
            self.bytes_forwarded_total.inc_by(absolute - prev);
        }
    }

    /// Record a hole-punch attempt for a region.
    pub fn inc_holepunch_attempt(&self, region: &str) {
        self.holepunch_attempt_total
            .with_label_values(&[region])
            .inc();
    }

    /// Record a hole-punch success for a region.
    pub fn inc_holepunch_success(&self, region: &str) {
        self.holepunch_success_total
            .with_label_values(&[region])
            .inc();
    }

    /// Record a registration rejected at the `MAX_ROUTES` cap.
    pub fn inc_routes_rejected(&self) {
        self.routes_rejected_total.inc();
    }

    /// Record a forward refused at the per-endpoint bandwidth cap.
    pub fn inc_bandwidth_capped(&self) {
        self.bandwidth_capped_total.inc();
    }

    /// Flip the up gauge.
    pub fn set_up(&self, up: bool) {
        self.up.set(if up { 1 } else { 0 });
    }

    /// Encode the current metrics in Prometheus text-exposition format.
    pub fn encode(&self) -> Result<String> {
        let mut buf = Vec::new();
        let encoder = TextEncoder::new();
        let families = self.registry.gather();
        encoder
            .encode(&families, &mut buf)
            .map_err(|e| RelayError::Metrics(format!("encoding metrics: {e}")))?;
        String::from_utf8(buf)
            .map_err(|e| RelayError::Metrics(format!("metrics output not utf-8: {e}")))
    }
}

impl RelayMetrics {
    /// Register the WSS-bridge metrics (`design/11 §3.4`, Task 215) into **this**
    /// relay registry and return the cloneable [`WssBridgeMetrics`] handle the
    /// bridge drives, so the WSS series appear on the same `/metrics` endpoint as
    /// the relay's core metrics. Idempotent registration is **not** assumed —
    /// call once per [`RelayMetrics`].
    pub fn wss_metrics(&self) -> Result<WssBridgeMetrics> {
        let bridges = IntGauge::with_opts(Opts::new(
            METRIC_WSS_BRIDGES,
            "Current number of live WSS<->Iroh bridges (one per open browser connection).",
        ))
        .map_err(metrics_err)?;
        let bytes_forwarded = IntCounterVec::new(
            Opts::new(
                METRIC_WSS_BYTES_FORWARDED_TOTAL,
                "Total ciphertext bytes pumped across WSS bridges, by direction (metadata only).",
            ),
            &[LABEL_DIRECTION],
        )
        .map_err(metrics_err)?;

        self.registry
            .register(Box::new(bridges.clone()))
            .map_err(metrics_err)?;
        self.registry
            .register(Box::new(bytes_forwarded.clone()))
            .map_err(metrics_err)?;

        // Touch both direction series so they appear at 0 before any traffic
        // (dashboards prefer a present-at-zero counter to a missing one).
        bytes_forwarded.with_label_values(&[DIRECTION_TO_CORE]);
        bytes_forwarded.with_label_values(&[DIRECTION_TO_BROWSER]);

        Ok(WssBridgeMetrics {
            inner: WssMetricsInner {
                bridges,
                bytes_forwarded,
            },
        })
    }
}

/// The private internals behind [`WssBridgeMetrics`](crate::api::WssBridgeMetrics)
/// — the live-bridge gauge and the per-direction byte counter, sharing the relay
/// [`Registry`]. **Byte counts only** (`design/11 §3.9`) — no payload reaches a
/// metric.
#[derive(Clone)]
pub struct WssMetricsInner {
    bridges: IntGauge,
    bytes_forwarded: IntCounterVec,
}

impl WssBridgeMetrics {
    /// A bridge opened — bump the live-bridge gauge.
    pub fn bridge_opened(&self) {
        self.inner.bridges.inc();
    }

    /// A bridge closed — drop the live-bridge gauge (never below 0).
    pub fn bridge_closed(&self) {
        let g = &self.inner.bridges;
        if g.get() > 0 {
            g.dec();
        }
    }

    /// Account `n` bytes pumped browser → Core (WSS frame → Iroh stream). Only
    /// the **count** is observed (`design/11 §3.9`).
    pub fn add_bytes_to_core(&self, n: u64) {
        self.inner
            .bytes_forwarded
            .with_label_values(&[DIRECTION_TO_CORE])
            .inc_by(n);
    }

    /// Account `n` bytes pumped Core → browser (Iroh stream → WSS frame). Only
    /// the **count** is observed (`design/11 §3.9`).
    pub fn add_bytes_to_browser(&self, n: u64) {
        self.inner
            .bytes_forwarded
            .with_label_values(&[DIRECTION_TO_BROWSER])
            .inc_by(n);
    }

    /// The current live-bridge count (the `concerto_relay_wss_bridges` value).
    pub fn live_bridges(&self) -> i64 {
        self.inner.bridges.get()
    }
}

fn metrics_err(e: prometheus::Error) -> RelayError {
    RelayError::Metrics(e.to_string())
}

/// A refresh hook invoked immediately before each scrape encodes, so the
/// exposition reflects the live relay (the routes gauge from the in-memory table
/// plus the bytes-forwarded counter from the embedded iroh-relay's own
/// counters). `None` for tests that drive the registry directly.
pub type RefreshFn = Arc<dyn Fn() + Send + Sync>;

/// Serve the Prometheus `/metrics` endpoint on `addr` until `shutdown` fires.
///
/// A minimal HTTP/1.1 server (no router framework — one path, `GET /metrics`).
/// Binds to `addr`; in CI/tests `PROMETHEUS_LISTEN_ADDR` is set to `127.0.0.1:0`
/// so the OS assigns a loopback port. Returns the bound [`SocketAddr`] so the
/// caller (and tests) can scrape it. `refresh` runs before each encode so a live
/// scrape is consistent with the relay.
pub async fn serve_metrics(
    addr: SocketAddr,
    metrics: RelayMetrics,
    refresh: Option<RefreshFn>,
    shutdown: CancellationToken,
) -> Result<SocketAddr> {
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| RelayError::Metrics(format!("binding {addr}: {e}")))?;
    let local = listener
        .local_addr()
        .map_err(|e| RelayError::Metrics(format!("reading local addr: {e}")))?;

    let metrics = Arc::new(metrics);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, _peer)) => {
                            let metrics = metrics.clone();
                            let refresh = refresh.clone();
                            tokio::spawn(async move {
                                if let Err(err) = serve_one(stream, &metrics, refresh.as_ref()).await {
                                    tracing::debug!(%err, "metrics connection ended");
                                }
                            });
                        }
                        Err(err) => {
                            tracing::warn!(%err, "metrics accept failed");
                            break;
                        }
                    }
                }
            }
        }
    });

    Ok(local)
}

/// Handle one HTTP connection: read the request line, answer `GET /metrics` with
/// the encoded exposition, everything else with `404`. Deliberately minimal — a
/// scrape endpoint, not a web server.
async fn serve_one(
    mut stream: tokio::net::TcpStream,
    metrics: &RelayMetrics,
    refresh: Option<&RefreshFn>,
) -> std::io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Read up to the end of the request head (bounded — never read the whole
    // body; a scrape has none). 8 KiB is ample for a request line + headers.
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf).await?;
    let head = String::from_utf8_lossy(&buf[..n]);
    let path = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("");

    let response = if path == "/metrics" {
        if let Some(refresh) = refresh {
            refresh();
        }
        match metrics.encode() {
            Ok(body) => http_response("200 OK", "text/plain; version=0.0.4; charset=utf-8", &body),
            Err(e) => http_response("500 Internal Server Error", "text/plain", &e.to_string()),
        }
    } else if path == "/healthz" || path == "/health" {
        // A trivial liveness probe (`design/11 §6.2` health endpoint).
        http_response("200 OK", "text/plain", "ok\n")
    } else {
        http_response("404 Not Found", "text/plain", "not found\n")
    };

    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

fn http_response(status: &str, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
