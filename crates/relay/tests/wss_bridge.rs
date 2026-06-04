//! Tier-2 loopback double for the WSS↔Iroh bridge (`design/11 §3.4` Path B,
//! Task 215).
//!
//! Stands up, entirely in-process with **no network beyond loopback**:
//!
//!   - the `concerto-relay` library (`Relay::start`) on a hermetic loopback port
//!     (the embedded `iroh-relay` dev server, Task 214),
//!   - the **WSS bridge** (`Relay::start_wss_bridge`) on a loopback TLS port,
//!     terminating an ephemeral self-signed cert,
//!   - a **loopback "Core" Iroh endpoint** — a second Iroh endpoint, IP
//!     transports CLEARED so the only viable path is the relay (the §5.1
//!     loopback-Iroh double) — that accepts the bridge's bidi stream and echoes
//!     **opaque bytes**,
//!   - a `tokio-tungstenite` **WSS client** that connects to
//!     `wss://127.0.0.1:<port>/wss/<core_endpoint_id>` trusting the bridge's cert.
//!
//! It proves: the FROZEN `/wss/<endpoint_id>` route, the one-WSS-to-one-Iroh-bidi
//! mapping, **byte-identical bidirectional forwarding** (browser→relay→Core and
//! back), the **ciphertext-only invariant** (the pump derives no plaintext — a
//! random opaque blob round-trips and never appears in the relay's observable
//! metrics surface; the pump's only input type is `&[u8]`), path-routing
//! rejection (malformed id → pre-upgrade HTTP 4xx; unknown endpoint → clean close,
//! no hang), and clean teardown of `wss_bridges`.
//!
//! Every wait is timeout-bounded so a headless CI runner can never hang.
//!
//! What this does NOT cover (→ Phase-5 web client / the Phase-2/Phase-5 Tier-3
//! checklist): a **real browser** establishing a real Noise IK over a **real WSS
//! connection across a real network** to a Core behind a real NAT, served by a
//! relay on real infrastructure — "open the web client on a borrowed laptop …
//! LAN-direct + relayed" (Tasks 519–522), gated on the still-PENDING
//! real-WAN-relayed datapoint (`design/spikes/tonic-iroh-findings.md §5`).

use std::sync::Arc;
use std::time::Duration;

use concerto_relay::{Relay, RelayConfig, WssTlsConfig};
use futures::{SinkExt, StreamExt};
use iroh::endpoint::{presets, Connection, RelayMode};
use iroh::{Endpoint, RelayMap, RelayUrl};
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName};
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

const TRANSPORT_ALPN: &[u8] = b"concerto/transport/1";
const SAN: &str = "localhost";

// ---------------------------------------------------------------------------
// Loopback config (relay + prometheus + WSS all on 127.0.0.1:0).
// ---------------------------------------------------------------------------

fn loopback_config() -> RelayConfig {
    RelayConfig::from_lookup(|key| match key {
        "RELAY_LISTEN_ADDR" => Some("127.0.0.1:0".into()),
        "PROMETHEUS_LISTEN_ADDR" => Some("127.0.0.1:0".into()),
        "WSS_LISTEN_ADDR" => Some("127.0.0.1:0".into()),
        _ => None,
    })
    .expect("loopback relay+wss config")
}

// ---------------------------------------------------------------------------
// A loopback "Core": a relay-forced Iroh endpoint that accepts the bridge's bidi
// stream and ECHOES OPAQUE BYTES (it never interprets them — the bridge is what's
// under test, not a real Core). The "browser" drives the framing end-to-end.
// ---------------------------------------------------------------------------

/// Spawn the echo "Core" on a relay-forced endpoint. Returns its endpoint id.
async fn spawn_echo_core(
    relay_url: &RelayUrl,
    shutdown: tokio_util::sync::CancellationToken,
) -> (String, Endpoint) {
    let map = RelayMap::from_iter([relay_url.clone()]);
    let core = Endpoint::builder(presets::N0)
        .alpns(vec![TRANSPORT_ALPN.to_vec()])
        .clear_ip_transports()
        .relay_mode(RelayMode::Custom(map))
        .bind()
        .await
        .expect("bind echo-core endpoint");
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
            tokio::spawn(async move {
                if let Ok(conn) = incoming.await {
                    echo_conn(conn, sd).await;
                }
            });
        }
        accept_ep.close().await;
    });

    (id, core)
}

/// Echo every byte received on the accepted bidi stream straight back — purely
/// opaque (the bytes are the browser's ciphertext as far as everything between
/// the WSS client and here is concerned).
async fn echo_conn(conn: Connection, shutdown: tokio_util::sync::CancellationToken) {
    let (mut send, mut recv) = match tokio::select! {
        _ = shutdown.cancelled() => return,
        res = conn.accept_bi() => res,
    } {
        Ok(pair) => pair,
        Err(_) => return,
    };
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = tokio::select! {
            _ = shutdown.cancelled() => return,
            r = recv.read(&mut buf) => r,
        };
        match n {
            Ok(Some(0)) => continue,
            Ok(Some(n)) => {
                if send.write_all(&buf[..n]).await.is_err() {
                    return;
                }
            }
            Ok(None) | Err(_) => return,
        }
    }
}

// ---------------------------------------------------------------------------
// The full harness: relay + WSS bridge + echo Core, all loopback.
// ---------------------------------------------------------------------------

struct Harness {
    relay: Relay,
    _bridge: concerto_relay::WssBridgeServer,
    wss_addr: std::net::SocketAddr,
    core_id: String,
    tls: WssTlsConfig,
    shutdown: tokio_util::sync::CancellationToken,
    _core_ep: Endpoint,
}

async fn harness() -> Harness {
    let relay = Relay::start(loopback_config())
        .await
        .expect("start in-process relay");
    let relay_url: RelayUrl = relay
        .relay_url()
        .expect("relay url")
        .parse()
        .expect("parse relay url");

    let tls = WssTlsConfig::self_signed(SAN).expect("self-signed wss cert");
    let bridge = relay
        .start_wss_bridge(tls.clone())
        .await
        .expect("start wss bridge")
        .expect("WSS_LISTEN_ADDR set → bridge present");
    let wss_addr = bridge.local_addr();

    let shutdown = tokio_util::sync::CancellationToken::new();
    let (core_id, core_ep) = spawn_echo_core(&relay_url, shutdown.clone()).await;

    // Register the Core's route with the relay (the relay-protocol registration
    // it observes; design/11 §3.2). The public addr is a loopback stand-in.
    relay
        .register_route(&core_id, relay.relay_listen_addr().expect("relay addr"))
        .expect("register core route");

    Harness {
        relay,
        _bridge: bridge,
        wss_addr,
        core_id,
        tls,
        shutdown,
        _core_ep: core_ep,
    }
}

// ---------------------------------------------------------------------------
// WSS client trusting the bridge's self-signed cert.
// ---------------------------------------------------------------------------

type WssClient =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Connect a WSS client to `wss://127.0.0.1:<port>/wss/<path_id>`, trusting the
/// bridge's self-signed cert and using SNI `localhost` (the cert's SAN). Returns
/// the upgraded stream, or the tungstenite error (e.g. an HTTP 4xx on a bad path).
async fn connect_wss(
    h: &Harness,
    path_id: &str,
) -> Result<WssClient, tokio_tungstenite::tungstenite::Error> {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();

    // Trust exactly the bridge's cert (via rustls-pki-types' PemObject — the
    // maintained PEM path).
    use tokio_rustls::rustls::pki_types::pem::PemObject;
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&h.tls.cert_pem)
        .collect::<Result<_, _>>()
        .expect("parse bridge cert");
    for c in certs {
        roots.add(c).expect("add bridge cert to roots");
    }
    let client_config = tokio_rustls::rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(client_config));

    let tcp = tokio::net::TcpStream::connect(h.wss_addr)
        .await
        .expect("tcp connect to wss bridge");
    let domain = ServerName::try_from(SAN.to_string()).expect("server name");
    let tls_stream = connector
        .connect(domain, tcp)
        .await
        .expect("client tls handshake");

    // Build the upgrade request with the right Host + path. We dial 127.0.0.1 but
    // the URL host is `localhost` (the SNI/SAN); the bridge ignores Host for
    // routing — it routes off the path only.
    let url = format!("wss://{SAN}/wss/{path_id}");
    let request = url.into_client_request().expect("client request");

    let (ws, _resp) = tokio_tungstenite::client_async(
        request,
        tokio_tungstenite::MaybeTlsStream::Rustls(tls_stream),
    )
    .await?;
    Ok(ws)
}

// ===========================================================================
// Tests
// ===========================================================================

/// A ciphertext blob round-trips browser→relay→Core and Core→relay→browser
/// **byte-identical** — proving the pump forwards faithfully, 1:1 on binary
/// frames, AND that the relay's observable metrics surface never carries the
/// blob (the ciphertext-only invariant).
#[tokio::test]
async fn round_trip_byte_identical_and_ciphertext_only() {
    let h = harness().await;

    let mut ws = tokio::time::timeout(Duration::from_secs(30), connect_wss(&h, &h.core_id))
        .await
        .expect("wss connect did not hang")
        .expect("wss upgrade succeeds for a valid endpoint id");

    // A random opaque blob — stands in for the browser's Noise/gRPC ciphertext.
    // The bridge must never decode it; the echo Core returns it verbatim.
    let blob: Vec<u8> = (0..4096u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 16) as u8)
        .collect();

    tokio::time::timeout(
        Duration::from_secs(10),
        ws.send(Message::binary(blob.clone())),
    )
    .await
    .expect("send did not hang")
    .expect("send blob");

    // Reassemble the echoed bytes until we have the whole blob back.
    let mut got: Vec<u8> = Vec::new();
    while got.len() < blob.len() {
        let msg = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("recv did not hang")
            .expect("stream not ended")
            .expect("ws message ok");
        match msg {
            Message::Binary(b) => got.extend_from_slice(&b),
            Message::Close(_) => break,
            _ => {}
        }
    }
    assert_eq!(
        got, blob,
        "blob round-trips byte-identical through the bridge"
    );

    // The bridge registered exactly one live bridge while open.
    {
        let state = h.relay.state();
        let guard = state.lock().unwrap();
        assert_eq!(guard.wss_bridges.len(), 1, "one live bridge while open");
        let b = guard.wss_bridges.values().next().unwrap();
        assert_eq!(
            b.endpoint_id, h.core_id,
            "bridge keyed to the addressed core"
        );
    }

    // Ciphertext-only: the relay's observable surface (metrics text) carries the
    // §3.9-permitted metadata (a live WSS bridge gauge + byte counts) but NEVER
    // the opaque payload bytes.
    let metrics = h.relay.metrics_text().expect("metrics text");
    assert!(
        metrics.contains("concerto_relay_wss_bridges"),
        "wss bridge gauge present:\n{metrics}"
    );
    assert!(
        metrics.contains("concerto_relay_wss_bytes_forwarded_total"),
        "wss byte counter present:\n{metrics}"
    );
    // The blob, rendered as a lossy string, must not appear anywhere in the
    // observable surface (a byte-count metric never embeds payload).
    let blob_lossy = String::from_utf8_lossy(&blob);
    assert!(
        !metrics.contains(blob_lossy.as_ref()),
        "relay metrics must not leak the bridged payload"
    );

    // Teardown: closing the WSS removes the bridge entry.
    ws.close(None).await.ok();
    drop(ws);
    wait_for_no_bridges(&h.relay).await;

    h.shutdown.cancel();
    h.relay.shutdown().await.expect("relay shutdown");
}

/// A malformed `<endpoint_id>` is rejected **pre-upgrade** with an HTTP 4xx — the
/// WSS handshake fails, no bridge is created, nothing hangs.
#[tokio::test]
async fn malformed_endpoint_id_rejected_pre_upgrade() {
    let h = harness().await;

    let err = tokio::time::timeout(
        Duration::from_secs(20),
        connect_wss(&h, "not-a-valid-endpoint-id"),
    )
    .await
    .expect("connect did not hang")
    .expect_err("malformed id must fail the upgrade");

    // tungstenite surfaces the non-101 status as an Http error.
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => {
            assert_eq!(
                resp.status(),
                tokio_tungstenite::tungstenite::http::StatusCode::BAD_REQUEST,
                "malformed id → 400 before upgrade"
            );
        }
        other => panic!("expected an HTTP 400, got: {other:?}"),
    }

    // No bridge was ever created.
    {
        let state = h.relay.state();
        assert_eq!(state.lock().unwrap().wss_bridges.len(), 0);
    }

    h.shutdown.cancel();
    h.relay.shutdown().await.expect("relay shutdown");
}

/// A well-formed but UNKNOWN endpoint id yields a clean close (the Core dial
/// fails / times out), not a hang — and leaves no surviving bridge state.
#[tokio::test]
async fn unknown_endpoint_closes_cleanly() {
    let h = harness().await;

    // A syntactically valid endpoint id that no Core is serving (a fresh random
    // Iroh keypair's id).
    let phantom = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .expect("bind phantom endpoint");
    let phantom_id = phantom.id().to_string();
    phantom.close().await;

    // The upgrade itself succeeds (the path parses); the dial then fails and the
    // bridge closes the WSS cleanly. Either way the client's next read ends
    // (clean close or connection-reset), bounded by the timeout.
    let connected = tokio::time::timeout(Duration::from_secs(30), connect_wss(&h, &phantom_id))
        .await
        .expect("connect did not hang");

    if let Ok(mut ws) = connected {
        // Drain until the stream ends — must terminate (clean close), not hang.
        let ended = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                match ws.next().await {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        })
        .await;
        assert!(ended.is_ok(), "unknown endpoint must end the WSS, not hang");
    }

    // No surviving bridge state for the failed dial.
    wait_for_no_bridges(&h.relay).await;

    h.shutdown.cancel();
    h.relay.shutdown().await.expect("relay shutdown");
}

/// Closing the WSS client removes the `wss_bridges` entry (teardown) — asserted
/// independently of the round-trip test so a teardown regression is isolated.
#[tokio::test]
async fn teardown_removes_bridge_entry() {
    let h = harness().await;

    let mut ws = tokio::time::timeout(Duration::from_secs(30), connect_wss(&h, &h.core_id))
        .await
        .expect("connect did not hang")
        .expect("upgrade ok");

    // Drive one frame so the bridge is fully live (dialed + pumping).
    ws.send(Message::binary(vec![1u8, 2, 3, 4]))
        .await
        .expect("send");
    let _ = tokio::time::timeout(Duration::from_secs(10), ws.next()).await;

    {
        let state = h.relay.state();
        assert_eq!(
            state.lock().unwrap().wss_bridges.len(),
            1,
            "live while open"
        );
    }

    ws.close(None).await.ok();
    drop(ws);
    wait_for_no_bridges(&h.relay).await;

    h.shutdown.cancel();
    h.relay.shutdown().await.expect("relay shutdown");
}

/// Poll (bounded) until the relay reports zero live bridges.
async fn wait_for_no_bridges(relay: &Relay) {
    let ok = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            {
                let state = relay.state();
                let n = state.lock().unwrap().wss_bridges.len();
                if n == 0 {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        ok.is_ok(),
        "wss_bridges must drain to empty after disconnect"
    );
}
