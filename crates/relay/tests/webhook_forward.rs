//! Tier-2 loopback double for Task 315: the inbound-webhook route
//! `POST /webhook/github/<endpoint_id>` (`design/11 §3.4.1`) end-to-end across:
//!   - the `concerto-relay` library (`Relay::start`) on a hermetic loopback port,
//!   - the **webhook route** (`Relay::start_webhook_route`) on a loopback TLS
//!     port, relay-forced dial through the embedded relay,
//!   - a loopback "Core" — a relay-forced Iroh endpoint that demuxes the `0x04`
//!     tag, reads the FROZEN `WebhookEnvelope` off the RAW duplex (via the
//!     production `concerto_transport::read_envelope`), and writes a single ack
//!     byte (via `write_ack`) — and
//!   - a synthetic HTTPS POST client (raw HTTP/1.1 over rustls) that plays GitHub.
//!
//! It proves: the route parses `<endpoint_id>` before work, opens **one** `0x04`
//! bidi, writes the envelope with the five FROZEN length-prefixed fields, reads
//! the Core's ack, and chains it to the HTTP status (`0x00`→200, `0x01`→400);
//! a malformed path is `400`'d before any dial; an offline Core (no route /
//! unbound id) is `503`'d (drop + log, no buffering).
//!
//! What the double does NOT cover (→ the Phase-3 Tier-3 checklist line): real
//! GitHub computing a real `X-Hub-Signature-256`, real webhook delivery over a
//! real relay on real infra, and GitHub's real redelivery policy. The HMAC
//! verify + delivery-id idempotency + parse + targeted-invalidate (the Core-side
//! pipeline) are covered by `crates/core/tests/webhook_ingest.rs`; here the Core
//! is a thin envelope-reader that proves the wire framing + the ack-chain.

use std::sync::Arc;
use std::time::Duration;

use concerto_relay::{Relay, RelayConfig, WssTlsConfig};
use concerto_transport::{read_envelope, write_ack, WebhookAck, WebhookEnvelope};
use iroh::endpoint::{presets, Connection, RelayMode};
use iroh::{Endpoint, RelayMap, RelayUrl};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName};
use tokio_rustls::TlsConnector;

const TRANSPORT_ALPN: &[u8] = b"concerto/transport/1";
const SAN: &str = "localhost";

fn loopback_config() -> RelayConfig {
    RelayConfig::from_lookup(|key| match key {
        "RELAY_LISTEN_ADDR" => Some("127.0.0.1:0".into()),
        "PROMETHEUS_LISTEN_ADDR" => Some("127.0.0.1:0".into()),
        "WEBHOOK_LISTEN_ADDR" => Some("127.0.0.1:0".into()),
        _ => None,
    })
    .expect("loopback relay+webhook config")
}

/// Spawn a loopback "Core": a relay-forced Iroh endpoint that accepts the `0x04`
/// bidi, reads the tag byte + the FROZEN envelope (production framing), reports
/// the received envelope on `seen_tx`, and acks `ack`.
async fn spawn_core(
    relay_url: &RelayUrl,
    ack: WebhookAck,
    seen_tx: mpsc::UnboundedSender<WebhookEnvelope>,
    shutdown: tokio_util::sync::CancellationToken,
) -> (String, Endpoint) {
    let map = RelayMap::from_iter([relay_url.clone()]);
    let core = Endpoint::builder(presets::N0)
        .alpns(vec![TRANSPORT_ALPN.to_vec()])
        .clear_ip_transports()
        .relay_mode(RelayMode::Custom(map))
        .bind()
        .await
        .expect("bind core endpoint");
    // Wait until the Core is connected to its home relay so the route's
    // relay-forced dial can reach it (otherwise the first POST races the Core's
    // relay registration and the dial 503s).
    tokio::time::timeout(Duration::from_secs(30), core.online())
        .await
        .expect("core online within 30s");
    let id = core.id().to_string();

    let accept_ep = core.clone();
    tokio::spawn(async move {
        loop {
            let incoming = tokio::select! {
                _ = shutdown.cancelled() => break,
                inc = accept_ep.accept() => match inc {
                    Some(inc) => inc,
                    None => break,
                },
            };
            let sd = shutdown.clone();
            let seen_tx = seen_tx.clone();
            tokio::spawn(async move {
                if let Ok(conn) = incoming.await {
                    handle_conn(conn, ack, seen_tx, sd).await;
                }
            });
        }
        accept_ep.close().await;
    });

    (id, core)
}

/// Read the `0x04` tag + envelope off one accepted bidi and ack.
async fn handle_conn(
    conn: Connection,
    ack: WebhookAck,
    seen_tx: mpsc::UnboundedSender<WebhookEnvelope>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let (send, recv) = match tokio::select! {
        _ = shutdown.cancelled() => return,
        res = conn.accept_bi() => res,
    } {
        Ok(pair) => pair,
        Err(_) => return,
    };
    // Bridge the Iroh (Send|Recv)Stream onto the transport's `IrohDuplex` framing
    // helpers by reading the raw bytes ourselves: first the 1-byte channel tag,
    // then the envelope via the production reader over a combined duplex.
    let mut duplex = concerto_transport::IrohDuplex::new(send, recv);
    let mut tag = [0u8; 1];
    if duplex.read_exact(&mut tag).await.is_err() || tag[0] != 0x04 {
        return;
    }
    let env = match read_envelope(&mut duplex).await {
        Ok(env) => env,
        Err(_) => {
            let _ = write_ack(&mut duplex, WebhookAck::Error).await;
            finish_and_drain(duplex, &conn).await;
            return;
        }
    };
    let _ = seen_tx.send(env);
    let _ = write_ack(&mut duplex, ack).await;
    // Finish the send stream and hold the connection open until the relay has
    // read the ack and closed, so the single ack byte is not lost to a premature
    // connection drop (the production serve loop holds the long-lived
    // per-connection task; here we mirror that by draining before returning).
    finish_and_drain(duplex, &conn).await;
}

/// Finish the ack send stream and wait until the peer closes the connection (or
/// a short grace elapses), so the ack byte is flushed to the wire before the
/// `Connection` is dropped.
async fn finish_and_drain(duplex: concerto_transport::IrohDuplex, conn: &Connection) {
    let (mut send, _recv) = duplex.into_halves();
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
}

struct Harness {
    relay: Relay,
    _route: concerto_relay::WebhookRouteServer,
    webhook_addr: std::net::SocketAddr,
    core_id: String,
    tls: WssTlsConfig,
    seen_rx: mpsc::UnboundedReceiver<WebhookEnvelope>,
    shutdown: tokio_util::sync::CancellationToken,
    _core_ep: Endpoint,
}

async fn harness(ack: WebhookAck) -> Harness {
    let relay = Relay::start(loopback_config())
        .await
        .expect("start in-process relay");
    let relay_url: RelayUrl = relay
        .relay_url()
        .expect("relay url")
        .parse()
        .expect("parse relay url");

    let tls = WssTlsConfig::self_signed(SAN).expect("self-signed cert");
    let route = relay
        .start_webhook_route(tls.clone())
        .await
        .expect("start webhook route")
        .expect("WEBHOOK_LISTEN_ADDR set → route present");
    let webhook_addr = route.local_addr();

    let shutdown = tokio_util::sync::CancellationToken::new();
    let (seen_tx, seen_rx) = mpsc::unbounded_channel();
    let (core_id, core_ep) = spawn_core(&relay_url, ack, seen_tx, shutdown.clone()).await;
    relay
        .register_route(&core_id, relay.relay_listen_addr().expect("relay addr"))
        .expect("register core route");

    Harness {
        relay,
        _route: route,
        webhook_addr,
        core_id,
        tls,
        seen_rx,
        shutdown,
        _core_ep: core_ep,
    }
}

/// POST a webhook to `https://127.0.0.1:<port>/webhook/github/<path_id>` over a
/// rustls TLS connection trusting the route's self-signed cert, and return the
/// HTTP status code from the response status line.
async fn post_webhook(
    h: &Harness,
    path_id: &str,
    event: &str,
    delivery: &str,
    signature: &str,
    body: &[u8],
) -> u16 {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    use tokio_rustls::rustls::pki_types::pem::PemObject;
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&h.tls.cert_pem)
        .collect::<Result<_, _>>()
        .expect("parse cert");
    for c in certs {
        roots.add(c).expect("add cert");
    }
    let client_config = tokio_rustls::rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(client_config));

    let tcp = tokio::net::TcpStream::connect(h.webhook_addr)
        .await
        .expect("tcp connect");
    let domain = ServerName::try_from(SAN.to_string()).expect("server name");
    let mut tls = connector.connect(domain, tcp).await.expect("tls handshake");

    let req = format!(
        "POST /webhook/github/{path_id} HTTP/1.1\r\nHost: {SAN}\r\n\
         X-GitHub-Delivery: {delivery}\r\nX-Hub-Signature-256: {signature}\r\n\
         X-GitHub-Event: {event}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    tls.write_all(req.as_bytes()).await.expect("write head");
    tls.write_all(body).await.expect("write body");
    tls.flush().await.expect("flush");

    // The route writes `Connection: close` and closes without a TLS
    // `close_notify`; rustls surfaces that as `UnexpectedEof`. The response was
    // fully received before the close, so treat that EOF as end-of-stream.
    let mut resp = Vec::new();
    if let Err(e) = tls.read_to_end(&mut resp).await {
        if e.kind() != std::io::ErrorKind::UnexpectedEof {
            panic!("read response: {e}");
        }
    }
    let text = String::from_utf8_lossy(&resp);
    let status_line = text.lines().next().unwrap_or("");
    // "HTTP/1.1 200 OK" → 200
    status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

// ===========================================================================
// Tests
// ===========================================================================

/// The route parses the path, opens a `0x04` bidi, writes the FROZEN envelope to
/// the Core (which receives all five fields byte-for-byte), reads the `0x00` ack,
/// and chains it to HTTP `200`.
#[tokio::test]
async fn forwards_envelope_and_chains_ok_ack() {
    let mut h = harness(WebhookAck::Accepted).await;
    let body = br#"{"action":"completed","check_run":{"head_sha":"abc"}}"#;
    let status = tokio::time::timeout(
        Duration::from_secs(30),
        post_webhook(
            &h,
            &h.core_id,
            "check_run",
            "delivery-xyz",
            "sha256=deadbeef",
            body,
        ),
    )
    .await
    .expect("post did not hang");
    assert_eq!(status, 200, "0x00 ack → HTTP 200");

    let env = tokio::time::timeout(Duration::from_secs(5), h.seen_rx.recv())
        .await
        .expect("core received an envelope")
        .expect("envelope present");
    assert_eq!(env.delivery_id, "delivery-xyz");
    assert_eq!(env.signature_256, "sha256=deadbeef");
    assert_eq!(env.event_type, "check_run");
    assert_eq!(env.endpoint_id, h.core_id, "endpoint id carried verbatim");
    assert_eq!(
        env.body,
        body.to_vec(),
        "body forwarded opaquely byte-identical"
    );

    h.shutdown.cancel();
    let _ = h.relay;
}

/// A `0x01` ack from the Core chains to HTTP `400` (HMAC mismatch / reject — the
/// relay reveals no reason).
#[tokio::test]
async fn reject_ack_chains_400() {
    let h = harness(WebhookAck::Reject).await;
    let status = tokio::time::timeout(
        Duration::from_secs(30),
        post_webhook(&h, &h.core_id, "check_run", "d-1", "sha256=00", b"{}"),
    )
    .await
    .expect("post did not hang");
    assert_eq!(status, 400, "0x01 ack → HTTP 400");
    h.shutdown.cancel();
}

/// A malformed `<endpoint_id>` is rejected with HTTP `400` BEFORE any dial.
#[tokio::test]
async fn malformed_endpoint_id_rejected_400() {
    let h = harness(WebhookAck::Accepted).await;
    let status = tokio::time::timeout(
        Duration::from_secs(30),
        post_webhook(&h, "not-a-valid-endpoint-id", "ping", "d-2", "", b"{}"),
    )
    .await
    .expect("post did not hang");
    assert_eq!(status, 400, "bad endpoint id → 400, no dial");
    h.shutdown.cancel();
}

/// An offline Core (a syntactically valid but unrouted endpoint id) → HTTP `503`
/// (drop + log, no buffering).
#[tokio::test]
async fn offline_core_chains_503() {
    let h = harness(WebhookAck::Accepted).await;
    // Mint a real endpoint id that is never bound / routed.
    let phantom = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .expect("bind phantom");
    let phantom_id = phantom.id().to_string();
    phantom.close().await;

    let status = tokio::time::timeout(
        Duration::from_secs(30),
        post_webhook(&h, &phantom_id, "check_run", "d-3", "sha256=00", b"{}"),
    )
    .await
    .expect("post did not hang");
    assert_eq!(status, 503, "offline core → 503 drop, no buffering");
    h.shutdown.cancel();
}
