//! Tier-2 relay-route double for the self-hosted relay (Task 214).
//!
//! Stands up the `concerto-relay` library **in-process** (the embedded
//! `iroh-relay` dev server on an OS-assigned loopback port — the spike §6
//! construction), has two Iroh endpoints register with it, and routes a relayed
//! gRPC stream through it: **IP transports are cleared on both endpoints, so the
//! ONLY viable QUIC path is the relay**. It exercises, end to end with NO
//! network beyond loopback:
//!
//!   - the embedded relay library (`Relay::start`) on a hermetic loopback port,
//!   - a real relayed gRPC round-trip + server-streaming firehose over the
//!     Task-212 adapter + Noise IK (the production codegen, real framing),
//!   - the routing-table lifecycle: register → the route appears in `RelayState`
//!     with a refreshing 90 s TTL → TTL eviction,
//!   - the `MAX_ROUTES` cap (a new endpoint past the cap is rejected),
//!   - the `BANDWIDTH_CAP_PER_ENDPOINT` cap (a forward past the cap is refused),
//!   - the Prometheus endpoint: `concerto_relay_routes` >= 1 and
//!     `concerto_relay_bytes_forwarded_total` > 0 after the transfer, scraped
//!     over real HTTP,
//!   - the ciphertext-only posture: the relay's observable surface (metrics text)
//!     carries only metadata — never the plaintext payload.
//!
//! Every wait is timeout-bounded so a headless CI runner can never hang.
//!
//! What this does NOT cover (→ Phase-2 Tier-3 manual checklist / the spike's
//! PENDING real-WAN-relayed row): a relay on REAL infrastructure routing a REAL
//! remote client over a real WAN — real relay-server distance, real RTT, real
//! bandwidth limits, anycast routing. That is "deploy the relay on real infra and
//! route a remote client through it" on the Phase-2 manual checklist.

use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use concerto_relay::config::ENV_MAX_ROUTES;
use concerto_relay::state::{ForwardOutcome, RegisterOutcome};
use concerto_relay::{Relay, RelayConfig, RelayState, ROUTE_TTL};
use concerto_transport::adapter::{handshake_responder, read_channel_tag};
use concerto_transport::{
    connect_channel, ChannelTag, IrohDuplex, NoiseDuplex, ALPN, MAX_MESSAGE_SIZE,
};
use iroh::endpoint::{presets, Connection, RelayMode};
use iroh::{Endpoint, EndpointAddr, RelayMap, RelayUrl};
use tonic::transport::Server;
use tonic::{Request, Response, Status};

pub mod pb {
    tonic::include_proto!("concerto.relay.route.v1");
}

use pb::relay_route_client::RelayRouteClient;
use pb::relay_route_server::{RelayRoute as RelayRouteSvcTrait, RelayRouteServer};
use pb::{EchoReply, EchoRequest, FirehoseChunk, FirehoseRequest};

const PLAINTEXT_MARKER: &[u8] = b"SUPER-SECRET-PLAINTEXT-PAYLOAD-MARKER";

// ---------------------------------------------------------------------------
// A fixed loopback test config (relay + prometheus both on 127.0.0.1:0).
// ---------------------------------------------------------------------------

fn loopback_config() -> RelayConfig {
    RelayConfig::from_lookup(|key| match key {
        "RELAY_LISTEN_ADDR" => Some("127.0.0.1:0".into()),
        "PROMETHEUS_LISTEN_ADDR" => Some("127.0.0.1:0".into()),
        _ => None,
    })
    .expect("loopback relay config")
}

// ---------------------------------------------------------------------------
// The trivial RelayRoute service (mirrors the transport loopback double).
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct RelayRouteSvc;

type FirehoseStream = Pin<Box<dyn futures::Stream<Item = Result<FirehoseChunk, Status>> + Send>>;

#[tonic::async_trait]
impl RelayRouteSvcTrait for RelayRouteSvc {
    async fn echo(&self, req: Request<EchoRequest>) -> Result<Response<EchoReply>, Status> {
        Ok(Response::new(EchoReply {
            payload: req.into_inner().payload,
        }))
    }

    type FirehoseStream = FirehoseStream;

    async fn firehose(
        &self,
        req: Request<FirehoseRequest>,
    ) -> Result<Response<Self::FirehoseStream>, Status> {
        let FirehoseRequest {
            total_bytes,
            chunk_bytes,
        } = req.into_inner();
        let chunk_bytes = (chunk_bytes.max(1)) as usize;
        let chunk = vec![0x5Au8; chunk_bytes];
        let mut remaining = total_bytes;
        let stream = futures::stream::poll_fn(move |_cx| {
            if remaining == 0 {
                return std::task::Poll::Ready(None);
            }
            let take = remaining.min(chunk_bytes as u64) as usize;
            remaining -= take as u64;
            std::task::Poll::Ready(Some(Ok(FirehoseChunk {
                data: chunk[..take].to_vec(),
            })))
        });
        Ok(Response::new(Box::pin(stream) as Self::FirehoseStream))
    }
}

// ---------------------------------------------------------------------------
// Relay-forced Iroh endpoint pair (IP transports cleared → only the relay path).
// ---------------------------------------------------------------------------

/// Build a `(server, client)` Iroh endpoint pair whose ONLY viable QUIC path is
/// the relay at `relay_url` (IP transports cleared on both — the spike's
/// relay-forced model).
async fn build_relay_pair(relay_url: &RelayUrl) -> (Endpoint, Endpoint) {
    let map = RelayMap::from_iter([relay_url.clone()]);
    let server = Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .clear_ip_transports()
        .relay_mode(RelayMode::Custom(map.clone()))
        .bind()
        .await
        .expect("bind relay server endpoint");
    let client = Endpoint::builder(presets::N0)
        .clear_ip_transports()
        .relay_mode(RelayMode::Custom(map))
        .bind()
        .await
        .expect("bind relay client endpoint");
    (server, client)
}

/// A minimal server serve loop reusing the Task-212 adapter: accept the relayed
/// connection, read the channel tag, run the Noise IK responder, serve one tonic
/// `RelayRoute` connection per bidi stream. (The production `IrohTransport::serve`
/// builds its endpoint internally and can't be pointed at a relay-forced one, so
/// the double drives the same adapter pieces directly — the relay is what's under
/// test here, not the Core's serve wiring, which the transport's own loopback
/// double already covers.)
fn spawn_relay_server(
    server_ep: Endpoint,
    core_static: Arc<concerto_identity::NoiseStatic>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let incoming = tokio::select! {
                _ = shutdown.cancelled() => break,
                inc = server_ep.accept() => match inc {
                    Some(inc) => inc,
                    None => break,
                },
            };
            let core_static = core_static.clone();
            let sd = shutdown.clone();
            tokio::spawn(async move {
                if let Ok(conn) = incoming.await {
                    serve_conn(conn, core_static, sd).await;
                }
            });
        }
        server_ep.close().await;
    });
}

async fn serve_conn(
    conn: Connection,
    core_static: Arc<concerto_identity::NoiseStatic>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    loop {
        let (send, recv) = tokio::select! {
            _ = shutdown.cancelled() => break,
            res = conn.accept_bi() => match res {
                Ok(pair) => pair,
                Err(_) => break,
            },
        };
        let duplex = IrohDuplex::new(send, recv);
        let core_static = core_static.clone();
        tokio::spawn(async move {
            let (tag, duplex) = match read_channel_tag(duplex).await {
                Ok(t) => t,
                Err(_) => return,
            };
            if tag != ChannelTag::Api {
                return;
            }
            let noise = match handshake_responder(duplex, &core_static).await {
                Ok(n) => n,
                Err(_) => return,
            };
            serve_one(noise).await;
        });
    }
}

async fn serve_one(io: NoiseDuplex) {
    let svc = RelayRouteServer::new(RelayRouteSvc)
        .max_decoding_message_size(MAX_MESSAGE_SIZE)
        .max_encoding_message_size(MAX_MESSAGE_SIZE);
    let incoming = futures::stream::once(async move { Ok::<_, std::io::Error>(io) });
    let _ = Server::builder()
        .add_service(svc)
        .serve_with_incoming(incoming)
        .await;
}

/// Full harness: start the in-process relay, build a relay-forced endpoint pair,
/// register the server's route with the relay, and return a connected tonic
/// client routed through the relay + the live [`Relay`] handle + a shutdown
/// token + the leaked client endpoint (kept alive for the channel's lifetime).
struct Harness {
    relay: Relay,
    client: RelayRouteClient<tonic::transport::Channel>,
    server_endpoint_id: String,
    shutdown: tokio_util::sync::CancellationToken,
}

async fn harness_with_config(config: RelayConfig) -> Harness {
    let relay = Relay::start(config).await.expect("start in-process relay");
    let relay_url: RelayUrl = relay
        .relay_url()
        .expect("relay url")
        .parse()
        .expect("parse relay url");

    let (server_ep, client_ep) = build_relay_pair(&relay_url).await;
    let server_id = server_ep.id().to_string();

    // Register the server's route with the relay (the relay-protocol registration
    // the relay observes; design/11 §3.2). The public addr is the relay-reachable
    // identity — we use the relay's own bound addr as the route's public addr
    // (loopback stand-in for the endpoint's public IP+port).
    relay
        .register_route(&server_id, relay.relay_listen_addr().expect("relay addr"))
        .expect("register server route");

    let core_seed = [7u8; 32];
    let core_static = Arc::new(concerto_identity::NoiseStatic::from_private(core_seed).unwrap());
    let core_noise_pub = core_static.public();

    let shutdown = tokio_util::sync::CancellationToken::new();
    spawn_relay_server(server_ep, core_static, shutdown.clone());

    let server_addr = relay_server_addr_from_id(&relay_url, &server_id);

    // Leak the client endpoint so it outlives the channel (the channel holds the
    // Iroh connection, which needs the endpoint alive).
    let client_ep: &'static Endpoint = Box::leak(Box::new(client_ep));
    let device_static = Arc::new(concerto_identity::NoiseStatic::generate().unwrap());

    let channel = tokio::time::timeout(
        Duration::from_secs(30),
        connect_channel(client_ep, server_addr, device_static, core_noise_pub),
    )
    .await
    .expect("relayed connect did not hang")
    .expect("connect channel over relay");

    let client = RelayRouteClient::new(channel)
        .max_decoding_message_size(MAX_MESSAGE_SIZE)
        .max_encoding_message_size(MAX_MESSAGE_SIZE);

    Harness {
        relay,
        client,
        server_endpoint_id: server_id,
        shutdown,
    }
}

async fn harness() -> Harness {
    harness_with_config(loopback_config()).await
}

/// Build the relayed server [`EndpointAddr`] from a known endpoint-id string —
/// its id + the relay URL only (no direct IP), so the client must dial via the
/// relay.
fn relay_server_addr_from_id(relay_url: &RelayUrl, id: &str) -> EndpointAddr {
    let endpoint_id: iroh::EndpointId = id.parse().expect("parse endpoint id");
    EndpointAddr::new(endpoint_id).with_relay_url(relay_url.clone())
}

// ===========================================================================
// Tests — the relay routes a real session.
// ===========================================================================

/// A full unary gRPC round-trip ROUTED THROUGH the relay (IP transports cleared)
/// — proves the embedded relay forwards a real session, and that after the
/// transfer the Prometheus endpoint reports a live route + bytes forwarded.
#[tokio::test]
async fn relayed_unary_round_trip_and_metrics() {
    let Harness {
        relay,
        mut client,
        server_endpoint_id,
        shutdown,
    } = harness().await;

    let reply = tokio::time::timeout(
        Duration::from_secs(20),
        client.echo(Request::new(EchoRequest {
            payload: PLAINTEXT_MARKER.to_vec(),
        })),
    )
    .await
    .expect("relayed echo did not hang")
    .expect("echo over relay")
    .into_inner();
    assert_eq!(reply.payload, PLAINTEXT_MARKER);

    // The route appears in RelayState with a refreshing TTL.
    assert!(relay.route_count() >= 1, "at least one route registered");

    // Scrape the Prometheus endpoint over real HTTP.
    let text = scrape(&relay).await;
    assert_metric_at_least(&text, "concerto_relay_routes", 1.0);
    assert_metric_greater_than(&text, "concerto_relay_bytes_forwarded_total", 0.0);
    assert!(text.contains("concerto_relay_up 1"), "up gauge is 1");

    // Ciphertext-only: the relay's observable surface never carries plaintext.
    assert!(
        !text.contains(std::str::from_utf8(PLAINTEXT_MARKER).unwrap()),
        "relay metrics must not leak plaintext payload"
    );
    let _ = server_endpoint_id;
    shutdown.cancel();
    relay.shutdown().await.expect("relay shutdown");
}

/// A server-streaming firehose through the relay — drives real bytes so
/// `bytes_forwarded` climbs, and proves the streaming subject survives the relay.
#[tokio::test]
async fn relayed_streaming_drives_bytes_forwarded() {
    let Harness {
        relay,
        mut client,
        shutdown,
        ..
    } = harness().await;

    let total: u64 = 2 * 1024 * 1024; // 2 MiB across 64 KiB chunks
    let mut stream = tokio::time::timeout(
        Duration::from_secs(20),
        client.firehose(Request::new(FirehoseRequest {
            total_bytes: total,
            chunk_bytes: 64 * 1024,
        })),
    )
    .await
    .expect("relayed firehose open did not hang")
    .expect("open firehose over relay")
    .into_inner();

    let mut received: u64 = 0;
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(20), stream.message())
        .await
        .expect("firehose recv did not hang")
        .expect("firehose recv")
    {
        received += chunk.data.len() as u64;
    }
    assert_eq!(received, total, "streamed byte count over relay");

    let text = scrape(&relay).await;
    assert_metric_greater_than(&text, "concerto_relay_bytes_forwarded_total", 0.0);

    shutdown.cancel();
    relay.shutdown().await.expect("relay shutdown");
}

/// `MAX_ROUTES=1`: a second, distinct endpoint registration is REJECTED and the
/// reject counter is bumped — the cap is enforced (design/11 §6.3).
#[tokio::test]
async fn max_routes_cap_rejects_new_endpoint() {
    let config = RelayConfig::from_lookup(|key| match key {
        "RELAY_LISTEN_ADDR" => Some("127.0.0.1:0".into()),
        "PROMETHEUS_LISTEN_ADDR" => Some("127.0.0.1:0".into()),
        ENV_MAX_ROUTES => Some("1".into()),
        _ => None,
    })
    .expect("config");
    let relay = Relay::start(config).await.expect("relay start");

    let addr = relay.relay_listen_addr().unwrap();
    relay
        .register_route("endpoint-A", addr)
        .expect("first route fits the cap");
    let err = relay.register_route("endpoint-B", addr);
    assert!(
        err.is_err(),
        "second distinct endpoint exceeds MAX_ROUTES=1"
    );

    // An existing endpoint's keep-alive still succeeds (it doesn't grow the table).
    relay
        .keepalive("endpoint-A", addr)
        .expect("keep-alive for an existing endpoint always succeeds");

    let text = scrape(&relay).await;
    assert_metric_at_least(&text, "concerto_relay_routes_rejected_total", 1.0);
    assert_metric_value(&text, "concerto_relay_routes", 1.0);

    relay.shutdown().await.expect("shutdown");
}

/// `BANDWIDTH_CAP_PER_ENDPOINT`: forwards up to the cap succeed; the one that
/// would exceed it is refused and the cap counter is bumped (design/11 §6.3, §3.9).
#[tokio::test]
async fn bandwidth_cap_per_endpoint_enforced() {
    let config = RelayConfig::from_lookup(|key| match key {
        "RELAY_LISTEN_ADDR" => Some("127.0.0.1:0".into()),
        "PROMETHEUS_LISTEN_ADDR" => Some("127.0.0.1:0".into()),
        "BANDWIDTH_CAP_PER_ENDPOINT" => Some("1000".into()),
        _ => None,
    })
    .expect("config");
    let relay = Relay::start(config).await.expect("relay start");
    let addr = relay.relay_listen_addr().unwrap();
    relay.register_route("ep", addr).expect("register");

    relay.account_forward("ep", 600).expect("under cap");
    relay.account_forward("ep", 400).expect("exactly at cap");
    let err = relay.account_forward("ep", 1);
    assert!(err.is_err(), "1 byte over the 1000-byte cap is refused");

    let text = scrape(&relay).await;
    assert_metric_at_least(&text, "concerto_relay_bandwidth_capped_total", 1.0);

    relay.shutdown().await.expect("shutdown");
}

/// Hole-punch success/attempt metrics are labelled by region (design/11 §6.3).
#[tokio::test]
async fn holepunch_metrics_are_labelled_by_region() {
    let relay = Relay::start(loopback_config()).await.expect("relay start");
    relay.record_holepunch_attempt("us-east");
    relay.record_holepunch_attempt("us-east");
    relay.record_holepunch_success("us-east");
    relay.record_holepunch_attempt("eu-fra");

    let text = scrape(&relay).await;
    assert!(
        text.contains("concerto_relay_holepunch_attempt_total{region=\"us-east\"} 2"),
        "us-east attempts labelled; got:\n{text}"
    );
    assert!(
        text.contains("concerto_relay_holepunch_success_total{region=\"us-east\"} 1"),
        "us-east successes labelled; got:\n{text}"
    );
    assert!(
        text.contains("concerto_relay_holepunch_attempt_total{region=\"eu-fra\"} 1"),
        "eu-fra attempts labelled; got:\n{text}"
    );

    relay.shutdown().await.expect("shutdown");
}

// ===========================================================================
// Unit-level RelayState lifecycle (TTL register / refresh / evict) — pure, fast.
// ===========================================================================

/// The route registers, refreshes its 90 s TTL on keep-alive, and is evicted
/// once expired (design/11 §3.2, §4).
#[test]
fn route_ttl_register_refresh_evict() {
    let mut state = RelayState::new();
    let addr = "127.0.0.1:7000".parse().unwrap();
    let t0 = Instant::now();

    assert_eq!(
        state.register("ep", addr, 100, t0),
        RegisterOutcome::Inserted
    );
    let route = state.route("ep").expect("route present");
    assert_eq!(route.expires_at, t0 + ROUTE_TTL);

    // Keep-alive 60 s later refreshes the TTL (the per-minute refresh).
    let t1 = t0 + Duration::from_secs(60);
    assert_eq!(
        state.register("ep", addr, 100, t1),
        RegisterOutcome::Refreshed
    );
    assert_eq!(state.route("ep").unwrap().expires_at, t1 + ROUTE_TTL);

    // Not yet expired just before the refreshed deadline.
    let before = t1 + ROUTE_TTL - Duration::from_secs(1);
    assert_eq!(state.evict_expired(before), 0, "still live");
    assert!(state.route("ep").is_some());

    // Past the refreshed deadline → evicted.
    let after = t1 + ROUTE_TTL + Duration::from_secs(1);
    assert_eq!(state.evict_expired(after), 1, "evicted on TTL expiry");
    assert!(state.route("ep").is_none());
}

/// The bandwidth cap accounts forwards and refuses the over-cap forward without
/// counting it (design/11 §3.9 ciphertext-only — byte totals only).
#[test]
fn bandwidth_counter_caps_without_counting_the_rejected_forward() {
    let mut state = RelayState::new();
    assert_eq!(
        state.account_forward("ep", 600, Some(1000)),
        ForwardOutcome::Forwarded
    );
    assert_eq!(
        state.account_forward("ep", 400, Some(1000)),
        ForwardOutcome::Forwarded
    );
    assert_eq!(state.bandwidth("ep").unwrap().bytes_forwarded, 1000);
    assert_eq!(
        state.account_forward("ep", 1, Some(1000)),
        ForwardOutcome::Capped
    );
    // The rejected forward did NOT add its bytes.
    assert_eq!(state.bandwidth("ep").unwrap().bytes_forwarded, 1000);
}

// ===========================================================================
// Scrape helpers
// ===========================================================================

/// Scrape the relay's real Prometheus `/metrics` endpoint over HTTP, bounded by a
/// timeout so a CI runner can't hang.
async fn scrape(relay: &Relay) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let addr = relay.prometheus_listen_addr();
    let body = tokio::time::timeout(Duration::from_secs(10), async move {
        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("connect metrics");
        stream
            .write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await
            .expect("write request");
        stream.flush().await.expect("flush");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("read response");
        String::from_utf8_lossy(&buf).into_owned()
    })
    .await
    .expect("metrics scrape did not hang");
    // Strip the HTTP head; return the exposition body.
    body.split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or(body)
}

/// Parse the value of a simple (unlabelled) metric line `name <value>`.
fn metric_value(text: &str, name: &str) -> Option<f64> {
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix(name) {
            let rest = rest.trim_start();
            // Skip labelled variants (`name{...} v`); we want the bare series.
            if rest.starts_with('{') {
                continue;
            }
            if let Some(v) = rest.split_whitespace().next() {
                if let Ok(parsed) = v.parse::<f64>() {
                    return Some(parsed);
                }
            }
        }
    }
    None
}

fn assert_metric_value(text: &str, name: &str, expected: f64) {
    let got =
        metric_value(text, name).unwrap_or_else(|| panic!("metric {name} not found in:\n{text}"));
    assert!(
        (got - expected).abs() < f64::EPSILON,
        "metric {name} = {got}, expected {expected}\n{text}"
    );
}

fn assert_metric_at_least(text: &str, name: &str, min: f64) {
    let got =
        metric_value(text, name).unwrap_or_else(|| panic!("metric {name} not found in:\n{text}"));
    assert!(
        got >= min,
        "metric {name} = {got}, expected >= {min}\n{text}"
    );
}

fn assert_metric_greater_than(text: &str, name: &str, floor: f64) {
    let got =
        metric_value(text, name).unwrap_or_else(|| panic!("metric {name} not found in:\n{text}"));
    assert!(
        got > floor,
        "metric {name} = {got}, expected > {floor}\n{text}"
    );
}
