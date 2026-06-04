//! The relay core: embed Iroh's `iroh-relay` server (`design/11 §3.2`, R-7),
//! own the routing-table observability + caps, run the Prometheus endpoint, and
//! handle clean shutdown (Task 214).
//!
//! The FROZEN entry point — [`Relay`](crate::api::Relay) — is declared in
//! [`crate::api`]; this module holds its `impl` and the private `RelayInner`.
//!
//! # Embed, don't fork (`design/11 §12 R-7`)
//!
//! `iroh-relay`'s `server` feature exposes the relay server; the spike
//! (`design/spikes/tonic-iroh-findings.md` §6) stood one up in-process on an
//! OS-assigned loopback port with plain HTTP. We lift that construction and
//! drive its bind addr from `RELAY_LISTEN_ADDR`. The hole-punch assist and QUIC
//! forwarding are iroh-relay's — our code is config + routing-table
//! observability + bandwidth caps + Prometheus metrics. We do **not** define a
//! new wire protocol.
//!
//! # Routing-table observability
//!
//! iroh-relay owns the actual `endpoint_id → addr` routing internally. This crate
//! keeps a **parallel** [`RelayState`](crate::api::RelayState) it drives through
//! [`Relay::register_route`] / [`Relay::keepalive`] / [`Relay::account_forward`]
//! so the Prometheus `routes` gauge, the `bytes_forwarded_total`, and the cap
//! enforcement (`MAX_ROUTES`, `BANDWIDTH_CAP_PER_ENDPOINT`) are exact and
//! testable hermetically. The relay reports **only metadata** (counts, byte
//! totals, source addr, endpoint id) — never payload (`design/11 §3.9`).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iroh_relay::server::{
    AccessConfig, RelayConfig as IrohRelayConfig, RelayMetrics as IrohRelayMetrics,
    Server as IrohRelayServer, ServerConfig as IrohServerConfig,
};
use tokio_util::sync::CancellationToken;

use crate::api::{Relay, RelayConfig, RelayState};
use crate::error::{RelayError, Result};
use crate::metrics::{self, RelayMetrics};
use crate::state::{ForwardOutcome, RegisterOutcome};

/// How often the background sweep evicts expired routes (`design/11 §3.2`: "one
/// sweep timer", no per-route task). Half the TTL keeps the gauge fresh without
/// busy-looping.
const SWEEP_INTERVAL: Duration = Duration::from_secs(45);

/// The private internals behind [`Relay`]. Holds the embedded iroh-relay server
/// (kept alive — dropping it stops the relay), the shared routing state, the
/// metrics, the config, and the shutdown token.
pub struct RelayInner {
    server: IrohRelayServer,
    state: Arc<Mutex<RelayState>>,
    metrics: RelayMetrics,
    config: RelayConfig,
    prometheus_addr: SocketAddr,
    shutdown: CancellationToken,
}

impl Relay {
    /// Build + run the relay from `config` (`design/11 §3.2`, §6.3) — **the
    /// FROZEN entry point Task 215 wraps**. Spawns the embedded `iroh-relay`
    /// server (R-7), the Prometheus `/metrics` endpoint, and the background
    /// routing-table sweep. Returns once everything is bound; the relay runs in
    /// the background until [`Relay::shutdown`] (or drop).
    ///
    /// Binds the iroh-relay HTTP server to `config.relay_listen_addr` and the
    /// metrics server to `config.prometheus_listen_addr`. In CI/tests both are
    /// set to loopback `:0` so the OS assigns ports — read them back with
    /// [`Relay::relay_listen_addr`] / [`Relay::prometheus_listen_addr`].
    pub async fn start(config: RelayConfig) -> Result<Self> {
        // iroh-relay initializes a rustls crypto provider on some code paths even
        // without TLS; install one process-wide so nothing panics (idempotent).
        let _ = rustls::crypto::ring::default_provider().install_default();

        let shutdown = CancellationToken::new();
        let state = Arc::new(Mutex::new(RelayState::new()));
        let relay_metrics = RelayMetrics::new()?;
        relay_metrics.set_up(true);

        // Embed iroh-relay (R-7): plain-HTTP relay on the configured bind addr.
        // The spike's hermetic construction (§6), parameterized by env config.
        let iroh_config = IrohServerConfig::<(), ()> {
            relay: Some(IrohRelayConfig::<(), ()> {
                http_bind_addr: config.relay_listen_addr,
                tls: None,
                limits: Default::default(),
                key_cache_capacity: Some(1024),
                access: AccessConfig::Everyone,
            }),
            quic: None,
            // We expose our OWN Prometheus endpoint (the FROZEN
            // `concerto_relay_*` names, `design/11 §6.3`), not iroh-relay's
            // internal `relayserver_*` metrics — so leave iroh's metrics addr off.
            metrics_addr: None,
        };
        let server = IrohRelayServer::spawn(iroh_config)
            .await
            .map_err(|e| RelayError::Server(format!("spawning iroh-relay server: {e}")))?;

        // iroh-relay's own (cloneable) metrics — the source of truth for bytes
        // forwarded (it does the actual forwarding, R-7). Share into the scrape
        // refresh + the sweep so the FROZEN `concerto_relay_bytes_forwarded_total`
        // tracks real relayed traffic.
        let iroh_metrics = server.metrics().clone();

        // The scrape refresh: routes gauge from the live table + bytes forwarded
        // from iroh-relay's ingress counter, applied before each encode.
        let refresh: metrics::RefreshFn = {
            let state = state.clone();
            let our_metrics = relay_metrics.clone();
            let iroh_metrics = iroh_metrics.clone();
            Arc::new(move || {
                let count = state.lock().expect("relay state lock").route_count();
                our_metrics.set_routes(count);
                our_metrics.set_bytes_forwarded(iroh_metrics.server.bytes_recv.get());
            })
        };

        // The metrics endpoint.
        let prometheus_addr = metrics::serve_metrics(
            config.prometheus_listen_addr,
            relay_metrics.clone(),
            Some(refresh),
            shutdown.clone(),
        )
        .await?;

        // The background TTL sweep (`design/11 §3.2`: one sweep timer) — also
        // keeps the routes gauge + bytes-forwarded counter fresh between scrapes.
        spawn_sweep(
            state.clone(),
            relay_metrics.clone(),
            iroh_metrics,
            shutdown.clone(),
        );

        tracing::info!(
            relay_http = %server
                .http_addr()
                .map(|a| a.to_string())
                .unwrap_or_else(|| "<none>".into()),
            prometheus = %prometheus_addr,
            max_routes = config.max_routes,
            bandwidth_cap = ?config.bandwidth_cap_per_endpoint,
            wss_reserved = ?config.wss_listen_addr,
            "concerto-relay started (iroh-relay embedded; ciphertext-only)"
        );

        Ok(Self {
            inner: RelayInner {
                server,
                state,
                metrics: relay_metrics,
                config,
                prometheus_addr,
                shutdown,
            },
        })
    }

    /// The address the embedded iroh-relay HTTP server bound to (the relay
    /// protocol endpoint clients dial; `design/11 §3.2`). `None` if the relay
    /// service is not configured (never, in this build).
    pub fn relay_listen_addr(&self) -> Option<SocketAddr> {
        self.inner.server.http_addr()
    }

    /// The address the Prometheus `/metrics` endpoint bound to (`design/11
    /// §6.3`). With `PROMETHEUS_LISTEN_ADDR=127.0.0.1:0` this is the OS-assigned
    /// loopback port tests scrape.
    pub fn prometheus_listen_addr(&self) -> SocketAddr {
        self.inner.prometheus_addr
    }

    /// The relay's URL clients register with / dial (the embedded iroh-relay's
    /// HTTP URL, built from its bound HTTP addr). `None` until the server has
    /// bound. (iroh-relay's own `http_url()` is gated behind its `test-utils`
    /// feature, so we format the addr the same way the spike did.)
    pub fn relay_url(&self) -> Option<String> {
        self.inner
            .server
            .http_addr()
            .map(|addr| format!("http://{addr}"))
    }

    /// The effective config (post-env-parse).
    pub fn config(&self) -> &RelayConfig {
        &self.inner.config
    }

    /// Register an endpoint's route, or refresh its TTL on keep-alive
    /// (`design/11 §3.2`, §4). Enforces the `MAX_ROUTES` cap: a *new* endpoint
    /// is refused (and the `concerto_relay_routes_rejected_total` metric bumped)
    /// when the table is full. Updates the `concerto_relay_routes` gauge.
    /// Returns [`RelayError::RoutesFull`] on rejection so the caller can refuse
    /// the registration.
    ///
    /// iroh-relay performs the actual routing; this is the observability + cap
    /// layer the relay's operators see (`design/11 §6.3`).
    pub fn register_route(&self, endpoint_id: &str, public_addr: SocketAddr) -> Result<()> {
        let now = Instant::now();
        let mut state = self.inner.state.lock().expect("relay state lock");
        let outcome = state.register(endpoint_id, public_addr, self.inner.config.max_routes, now);
        let count = state.route_count();
        drop(state);
        self.inner.metrics.set_routes(count);
        match outcome {
            RegisterOutcome::Inserted | RegisterOutcome::Refreshed => Ok(()),
            RegisterOutcome::Rejected => {
                self.inner.metrics.inc_routes_rejected();
                Err(RelayError::RoutesFull(format!(
                    "routing table at MAX_ROUTES={}; refusing new endpoint {endpoint_id}",
                    self.inner.config.max_routes
                )))
            }
        }
    }

    /// Refresh an endpoint's route TTL (the per-minute keep-alive, `design/11
    /// §3.2`). Equivalent to [`Self::register_route`] with the same address; a
    /// keep-alive for an unknown endpoint registers it (subject to the cap).
    pub fn keepalive(&self, endpoint_id: &str, public_addr: SocketAddr) -> Result<()> {
        self.register_route(endpoint_id, public_addr)
    }

    /// Account `bytes` of forwarded ciphertext for an endpoint against its
    /// per-endpoint cap (`design/11 §6.3`, §3.9) and the global
    /// `concerto_relay_bytes_forwarded_total` counter. Returns
    /// [`RelayError::BandwidthCapped`] (and accounts nothing) once the endpoint
    /// would exceed `BANDWIDTH_CAP_PER_ENDPOINT`. **Only the byte count** is
    /// observed — never the payload (`design/11 §3.9`).
    pub fn account_forward(&self, endpoint_id: &str, bytes: u64) -> Result<()> {
        let cap = self.inner.config.bandwidth_cap_per_endpoint;
        let mut state = self.inner.state.lock().expect("relay state lock");
        match state.account_forward(endpoint_id, bytes, cap) {
            ForwardOutcome::Forwarded => {
                // The `concerto_relay_bytes_forwarded_total` metric is driven from
                // the embedded iroh-relay's own byte counters (the real
                // forwarder, R-7) via `sync_forwarded_bytes_from_relay`, NOT from
                // here — this per-endpoint tally is the bandwidth-CAP policy layer
                // only, so we don't double-count.
                Ok(())
            }
            ForwardOutcome::Capped => {
                drop(state);
                self.inner.metrics.inc_bandwidth_capped();
                Err(RelayError::BandwidthCapped(format!(
                    "endpoint {endpoint_id} hit BANDWIDTH_CAP_PER_ENDPOINT={}",
                    cap.map(|c| c.to_string())
                        .unwrap_or_else(|| "<unset>".into())
                )))
            }
        }
    }

    /// Sync the `concerto_relay_bytes_forwarded_total` counter from the embedded
    /// iroh-relay server's **own** byte counters (`bytes_recv + bytes_sent`,
    /// `design/11 §3.9` ciphertext-only — byte totals, never content). iroh-relay
    /// owns the actual forwarding (R-7), so its counters are the source of truth
    /// for bytes-forwarded; this folds them into the FROZEN metric name operators
    /// scrape. Idempotent (sets the counter to the relay's current total). Called
    /// before a scrape ([`Self::metrics_text`]) and by the background sweep.
    fn sync_forwarded_bytes_from_relay(&self) {
        // iroh-relay's `RelayMetrics` exposes per-direction byte counters. The
        // relay forwards each byte once in and once out, so recv is the bytes
        // forwarded into the relay; we report recv (the ingress total) to avoid
        // double counting the same payload.
        let m = self.inner.server.metrics();
        let bytes = m.server.bytes_recv.get();
        self.inner.metrics.set_bytes_forwarded(bytes);
    }

    /// Record a hole-punch *attempt* for a region (the rate denominator,
    /// `design/11 §6.3`).
    pub fn record_holepunch_attempt(&self, region: &str) {
        self.inner.metrics.inc_holepunch_attempt(region);
    }

    /// Record a hole-punch *success* for a region (`design/11 §6.3`). The
    /// success *rate* is `success / attempt` per region in the dashboard.
    pub fn record_holepunch_success(&self, region: &str) {
        self.inner.metrics.inc_holepunch_success(region);
    }

    /// The current number of live routes (the `concerto_relay_routes` value).
    pub fn route_count(&self) -> usize {
        self.inner
            .state
            .lock()
            .expect("relay state lock")
            .route_count()
    }

    /// Total ciphertext bytes forwarded across all endpoints (the
    /// `concerto_relay_bytes_forwarded_total` value).
    pub fn total_bytes_forwarded(&self) -> u64 {
        self.inner
            .state
            .lock()
            .expect("relay state lock")
            .total_bytes_forwarded()
    }

    /// Encode the current Prometheus metrics in text-exposition format (the same
    /// bytes the `/metrics` endpoint serves). Convenience for tests + the
    /// in-process smoke double.
    pub fn metrics_text(&self) -> Result<String> {
        // Refresh the routes gauge from live state and the bytes-forwarded
        // counter from iroh-relay's own counters before encoding, so a scrape is
        // always consistent with the live relay (in case a sweep raced).
        let count = self.route_count();
        self.inner.metrics.set_routes(count);
        self.sync_forwarded_bytes_from_relay();
        self.inner.metrics.encode()
    }

    /// Run the relay until a Ctrl-C / SIGTERM signal arrives, then shut down
    /// cleanly. The binary wrapper (`main.rs`) calls this; Task 215 may replace
    /// it with its own loop that also drives the WSS bridge.
    pub async fn run_until_signal(self) -> Result<()> {
        wait_for_signal().await;
        tracing::info!("concerto-relay: shutdown signal received");
        self.shutdown().await
    }

    /// Shut the relay down cleanly (`design/11 §6.2` health/lifecycle): flip the
    /// up gauge, cancel the metrics + sweep tasks, and stop the embedded
    /// iroh-relay server.
    pub async fn shutdown(self) -> Result<()> {
        self.inner.metrics.set_up(false);
        self.inner.shutdown.cancel();
        self.inner
            .server
            .shutdown()
            .await
            .map_err(|e| RelayError::Server(format!("shutting down iroh-relay server: {e}")))?;
        Ok(())
    }
}

/// Spawn the single background sweep that evicts expired routes (`design/11
/// §3.2`: one sweep timer, not a per-route task) and keeps the routes gauge in
/// sync.
fn spawn_sweep(
    state: Arc<Mutex<RelayState>>,
    metrics: RelayMetrics,
    iroh_metrics: IrohRelayMetrics,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SWEEP_INTERVAL);
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = ticker.tick() => {
                    let now = Instant::now();
                    let (evicted, count) = {
                        let mut st = state.lock().expect("relay state lock");
                        let evicted = st.evict_expired(now);
                        (evicted, st.route_count())
                    };
                    if evicted > 0 {
                        tracing::debug!(evicted, live = count, "relay: swept expired routes");
                    }
                    metrics.set_routes(count);
                    metrics.set_bytes_forwarded(iroh_metrics.server.bytes_recv.get());
                }
            }
        }
    });
}

/// Await Ctrl-C, plus SIGTERM on Unix (the Docker / Fly stop signal). Resolves
/// on the first signal. Cross-platform: Windows only gets Ctrl-C
/// (`design/11 §6.3` — the Docker image targets Linux, but the binary must
/// compile on the Windows CI lane, Task 113).
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(%e, "could not install SIGTERM handler; Ctrl-C only");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
