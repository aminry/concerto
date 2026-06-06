//! The WSS↔Iroh bridge (`design/11 §3.4` Path B, §6.2 the `Wss` node, Task 215).
//!
//! The **only** non-Iroh path the relay runs. Browsers cannot speak Iroh
//! natively in V1.0 (`design/11 §3.4`), so a browser reaches a Core by pointing
//! its `wss://<relay-host>/wss/<endpoint_id>` connection at this bridge. Per
//! browser WSS connection the bridge:
//!
//!   1. terminates the outer TLS the `wss://` scheme requires (operator cert, or
//!      an ephemeral self-signed pair for the loopback double),
//!   2. parses the FROZEN `/wss/<endpoint_id>` path into an Iroh endpoint id
//!      (rejecting malformed/oversized ids with HTTP 4xx **before** the upgrade),
//!   3. completes the WebSocket upgrade (`tokio_tungstenite::accept_hdr_async`),
//!   4. opens **one** Iroh bidi stream to the addressed Core
//!      (`design/11 §3.1.1` gotcha 2: one WSS connection == one Iroh bidi
//!      stream), registers a [`WssBridge`](crate::api::WssBridge) in
//!      [`RelayState::wss_bridges`](crate::api::RelayState::wss_bridges), and
//!   5. runs a **bidirectional opaque byte pump**: WSS binary-frame payload →
//!      Iroh `SendStream`, Iroh `RecvStream` bytes → WSS binary frame.
//!
//! # Ciphertext-only (`design/11 §3.9`) — the load-bearing invariant
//!
//! The browser establishes Noise IK **inside** the WSS stream using its device
//! cert (`design/12 §3.4`); the relay sees **ciphertext only**. The pump operates
//! exclusively on `&[u8]` — it never parses, decodes, decrypts, length-prefixes,
//! re-frames, or logs frame contents. The only relay-observable derivations are
//! the §3.9-permitted metadata: the addressed endpoint id, the source IP, byte
//! *counts*, and timestamps. A WSS **binary** message payload maps **1:1** onto
//! the bytes written to the Iroh stream and vice versa — no relay-imposed
//! envelope. This is enforced by a property test
//! (`crates/relay/tests/wss_bridge.rs`).
//!
//! # One Iroh endpoint, relay-forced
//!
//! `iroh-relay`'s `Server` (the embedded relay, Task 214) is a relay-**protocol**
//! server — it has no client `Endpoint` to dial peers. So the bridge holds its
//! own Iroh client [`Endpoint`] configured to route through the relay's own URL
//! (`RelayMode::Custom`): the bridge dials the addressed Core endpoint id and the
//! traffic rides the same relay the Core registered with. (Drift from the task's
//! "the relay's existing Iroh endpoint" — there is no such dialable endpoint on
//! `iroh-relay::Server`; flagged in Handoff.) The dial uses the **raw** bidi
//! stream — NO channel-tag byte, NO Noise — because the browser's encrypted gRPC
//! payload already carries the transport framing the Core expects; the bridge is
//! a transparent pipe.
//!
//! # Cross-platform
//!
//! TLS is rustls (`design/11 §6.3` / the `reqwest`-rustls posture, Task 112);
//! nothing here is `#[cfg(unix)]`. The Windows CI lane (Task 113) builds it as-is.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use iroh::endpoint::{presets, Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::api::{
    RelayState, WssBridge, WssBridgeMetrics, WssBridgeServer, WssTlsConfig, MAX_ENDPOINT_ID_LEN,
    WSS_PATH_PREFIX,
};
use crate::error::{RelayError, Result};

/// The Iroh ALPN the bridge dials Cores with. **Must match**
/// `concerto_transport::ALPN` (`b"concerto/transport/1"`, Task 212) — the bridge
/// opens a transport-protocol bidi stream to the Core, so the ALPN is the same
/// one the Core's endpoint accepts. Declared locally (not a `concerto-transport`
/// dep) so the relay binary stays dependency-light; the value is FROZEN by the
/// transport crate.
const TRANSPORT_ALPN: &[u8] = b"concerto/transport/1";

/// How long a single idle bridge waits with no traffic in either direction
/// before it is torn down (`design/11 §3.4` lifecycle: idle timeout closes the
/// bridge). Bounds every pump wait so a stalled peer can't pin a bridge forever.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Bounded copy buffer for the Iroh→WSS direction. WSS framing already bounds the
/// other direction (one message at a time); this caps a single read so a fast
/// Core can't make us buffer unboundedly (`design/11 §3.4` backpressure).
const IROH_READ_CHUNK: usize = 64 * 1024;

/// How long the per-connection dial to the addressed Core may take before the
/// bridge gives up and closes the WSS cleanly (`design/11 §3.4`: an unreachable /
/// refusing Core yields a clean close, not a hang). Generous enough for a real
/// relayed hole-punch, bounded so a phantom endpoint id does not pin the task.
const DIAL_TIMEOUT: Duration = Duration::from_secs(20);

/// The private internals behind [`WssBridgeServer`].
pub struct WssBridgeInner {
    /// The bound WSS TLS listener address (read back for tests / logging).
    local_addr: SocketAddr,
    /// The Iroh client endpoint the bridge dials Cores with (relay-forced via the
    /// relay's own URL). Held so it outlives every in-flight bridge.
    endpoint: Endpoint,
    /// The relay URL Cores register with — every dialed [`EndpointAddr`] carries
    /// it so the dial routes through the relay.
    relay_url: RelayUrl,
    /// Shared relay state — bridges register/deregister in `wss_bridges`.
    state: Arc<Mutex<RelayState>>,
    /// The WSS-bridge metrics (live-bridge gauge + per-direction byte counters).
    metrics: WssBridgeMetrics,
    /// Cancels the accept loop + signals every bridge to tear down on shutdown.
    shutdown: CancellationToken,
}

impl WssTlsConfig {
    /// Generate an ephemeral self-signed cert/key pair (rcgen) for `san` — used
    /// by the Tier-2 loopback double, and as a last-resort dev fallback when no
    /// operator cert is supplied. **Production deploys supply a real cert** via
    /// [`WssTlsConfig`]'s PEM fields; a self-signed cert is not browser-trusted.
    pub fn self_signed(san: &str) -> Result<Self> {
        let cert = rcgen::generate_simple_self_signed(vec![san.to_string()])
            .map_err(|e| RelayError::Server(format!("generating self-signed WSS cert: {e}")))?;
        Ok(Self {
            cert_pem: cert.cert.pem().into_bytes(),
            key_pem: cert.signing_key.serialize_pem().into_bytes(),
        })
    }

    /// Build a rustls [`ServerConfig`](tokio_rustls::rustls::ServerConfig) from
    /// the PEM cert chain + key. Pure TLS-server setup (no client auth — the
    /// browser authenticates to the *Core* via the inner Noise IK, not to the
    /// relay, `design/11 §3.9`).
    pub(crate) fn rustls_server_config(&self) -> Result<tokio_rustls::rustls::ServerConfig> {
        // PEM parsing via rustls-pki-types' `PemObject` (the maintained path —
        // `rustls-pemfile` is RUSTSEC-2025-0134 unmaintained and is just a thin
        // wrapper over this same code).
        use tokio_rustls::rustls::pki_types::pem::PemObject;
        use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

        let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&self.cert_pem)
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| RelayError::Server(format!("parsing WSS cert PEM: {e}")))?;
        if certs.is_empty() {
            return Err(RelayError::Server(
                "WSS cert PEM contained no certificates".into(),
            ));
        }
        let key = PrivateKeyDer::from_pem_slice(&self.key_pem)
            .map_err(|e| RelayError::Server(format!("parsing WSS key PEM: {e}")))?;

        tokio_rustls::rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| RelayError::Server(format!("building WSS rustls config: {e}")))
    }
}

impl WssBridgeServer {
    /// Start the WSS bridge: bind the TLS listener on `listen_addr`, build the
    /// relay-forced Iroh client endpoint (dialing through `relay_url`), and spawn
    /// the accept loop. Returns once bound; the loop runs in the background until
    /// `shutdown` fires (or [`Self::shutdown`]).
    ///
    /// Called by the `concerto-relay` binary when the reserved `WSS_LISTEN_ADDR`
    /// is set ([`RelayConfig::wss_listen_addr`](crate::api::RelayConfig)). In
    /// CI/tests `listen_addr` is `127.0.0.1:0` so the OS assigns a port — read it
    /// back with [`Self::local_addr`].
    pub async fn start(
        listen_addr: SocketAddr,
        relay_url: RelayUrl,
        tls: WssTlsConfig,
        state: Arc<Mutex<RelayState>>,
        metrics: WssBridgeMetrics,
        shutdown: CancellationToken,
    ) -> Result<Self> {
        // Same ring provider the relay installs (idempotent) so rustls has a
        // process-wide default crypto provider on every code path.
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();

        let server_config = tls.rustls_server_config()?;
        let acceptor = TlsAcceptor::from(Arc::new(server_config));

        let listener = TcpListener::bind(listen_addr)
            .await
            .map_err(|e| RelayError::Server(format!("binding WSS listener {listen_addr}: {e}")))?;
        let local_addr = listener
            .local_addr()
            .map_err(|e| RelayError::Server(format!("reading WSS listener addr: {e}")))?;

        // The bridge's own Iroh client endpoint, routed through the relay
        // (`RelayMode::Custom`). It dials Cores by endpoint id; the ALPN matches
        // the Core's transport ALPN (Task 212).
        let map = RelayMap::from_iter([relay_url.clone()]);
        let endpoint = Endpoint::builder(presets::N0)
            .alpns(vec![TRANSPORT_ALPN.to_vec()])
            .relay_mode(RelayMode::Custom(map))
            .bind()
            .await
            .map_err(|e| {
                RelayError::Server(format!("binding WSS bridge Iroh client endpoint: {e}"))
            })?;

        let inner = WssBridgeInner {
            local_addr,
            endpoint,
            relay_url,
            state,
            metrics,
            shutdown,
        };

        spawn_accept_loop(listener, acceptor, &inner);

        tracing::info!(
            wss_listen = %local_addr,
            relay = %inner.relay_url,
            "concerto-relay WSS bridge started (WSS<->Iroh, ciphertext-only)"
        );

        Ok(Self { inner })
    }

    /// The bound WSS listener address (`127.0.0.1:0` resolves to the OS-assigned
    /// port in tests).
    pub fn local_addr(&self) -> SocketAddr {
        self.inner.local_addr
    }

    /// Tear the bridge down: cancel the accept loop + every in-flight pump, then
    /// close the Iroh client endpoint.
    pub async fn shutdown(self) -> Result<()> {
        self.inner.shutdown.cancel();
        self.inner.endpoint.close().await;
        Ok(())
    }
}

/// A clone of the per-connection context the accept loop hands each bridge task —
/// everything a bridge needs without holding the whole [`WssBridgeServer`].
#[derive(Clone)]
struct BridgeCtx {
    endpoint: Endpoint,
    relay_url: RelayUrl,
    state: Arc<Mutex<RelayState>>,
    metrics: WssBridgeMetrics,
    shutdown: CancellationToken,
}

fn spawn_accept_loop(listener: TcpListener, acceptor: TlsAcceptor, inner: &WssBridgeInner) {
    let ctx = BridgeCtx {
        endpoint: inner.endpoint.clone(),
        relay_url: inner.relay_url.clone(),
        state: inner.state.clone(),
        metrics: inner.metrics.clone(),
        shutdown: inner.shutdown.clone(),
    };
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = ctx.shutdown.cancelled() => break,
                accepted = listener.accept() => {
                    match accepted {
                        Ok((tcp, peer)) => {
                            let acceptor = acceptor.clone();
                            let ctx = ctx.clone();
                            tokio::spawn(async move {
                                if let Err(err) = serve_connection(tcp, peer, acceptor, ctx).await {
                                    // Metadata-only: never the payload (§3.9).
                                    tracing::debug!(%peer, %err, "WSS bridge connection ended");
                                }
                            });
                        }
                        Err(err) => {
                            tracing::warn!(%err, "WSS bridge accept failed");
                            break;
                        }
                    }
                }
            }
        }
    });
}

/// Terminate TLS, complete the `/wss/<endpoint_id>` upgrade (rejecting a bad path
/// pre-upgrade), dial the Core, register the bridge, and run the pump.
// The handshake callback's `Result<Response, ErrorResponse>` return type is
// dictated by `tokio_tungstenite::accept_hdr_async` — the large `Err` variant is
// the library's `http::Response`, not ours to box.
#[allow(clippy::result_large_err)]
async fn serve_connection(
    tcp: TcpStream,
    peer: SocketAddr,
    acceptor: TlsAcceptor,
    ctx: BridgeCtx,
) -> Result<()> {
    // Outer TLS (the `wss://` scheme's transport hop). Bounded so a stalled
    // handshake can't pin the task.
    let tls = tokio::time::timeout(Duration::from_secs(30), acceptor.accept(tcp))
        .await
        .map_err(|_| RelayError::Server("WSS TLS handshake timed out".into()))?
        .map_err(|e| RelayError::Server(format!("WSS TLS handshake: {e}")))?;

    // The path is parsed during the WebSocket handshake callback so a malformed
    // `<endpoint_id>` is refused with an HTTP 4xx BEFORE the upgrade completes
    // (`design/11 §3.4`). The callback stashes the parsed id.
    let parsed: Arc<Mutex<Option<EndpointId>>> = Arc::new(Mutex::new(None));
    let parsed_cb = parsed.clone();
    let callback =
        move |req: &Request, resp: Response| -> std::result::Result<Response, ErrorResponse> {
            match parse_endpoint_id(req.uri().path()) {
                Ok(id) => {
                    *parsed_cb.lock().expect("wss parse lock") = Some(id);
                    Ok(resp)
                }
                Err(reason) => {
                    let body = Some(format!("invalid WSS path: {reason}\n"));
                    let err = ErrorResponse::new(body);
                    let (mut parts, body) = err.into_parts();
                    parts.status = StatusCode::BAD_REQUEST;
                    Err(ErrorResponse::from_parts(parts, body))
                }
            }
        };

    let ws = tokio::time::timeout(
        Duration::from_secs(30),
        tokio_tungstenite::accept_hdr_async(tls, callback),
    )
    .await
    .map_err(|_| RelayError::Server("WSS upgrade timed out".into()))?
    .map_err(|e| RelayError::Server(format!("WSS upgrade: {e}")))?;

    let endpoint_id = parsed
        .lock()
        .expect("wss parse lock")
        .take()
        .ok_or_else(|| RelayError::Server("WSS upgrade completed without a parsed id".into()))?;
    let endpoint_id_str = endpoint_id.to_string();

    // Dial the addressed Core through the relay and open ONE bidi stream
    // (`design/11 §3.1.1` gotcha 2). Bounded so a missing/refusing Core yields a
    // clean close, not a hang.
    let dial = async {
        let addr = EndpointAddr::new(endpoint_id).with_relay_url(ctx.relay_url.clone());
        let conn: Connection = ctx
            .endpoint
            .connect(addr, TRANSPORT_ALPN)
            .await
            .map_err(|e| RelayError::Server(format!("dialing Core {endpoint_id_str}: {e}")))?;
        let (send, recv) = conn
            .open_bi()
            .await
            .map_err(|e| RelayError::Server(format!("opening Iroh bidi to Core: {e}")))?;
        Ok::<_, RelayError>((conn, send, recv))
    };
    let (conn, send, recv) = match tokio::time::timeout(DIAL_TIMEOUT, dial).await {
        Ok(Ok(parts)) => parts,
        Ok(Err(e)) => {
            // Core refused / unknown endpoint: clean WSS close (`design/11 §3.4`).
            tracing::debug!(endpoint_id = %endpoint_id_str, %peer, "WSS bridge: Core dial failed; closing WSS");
            close_ws_clean(ws).await;
            return Err(e);
        }
        Err(_) => {
            tracing::debug!(endpoint_id = %endpoint_id_str, %peer, "WSS bridge: Core dial timed out; closing WSS");
            close_ws_clean(ws).await;
            return Err(RelayError::Server("dialing Core timed out".into()));
        }
    };

    // Register the live bridge (metadata only — §3.9).
    let bridge_id = new_bridge_id(peer);
    {
        let mut state = ctx.state.lock().expect("relay state lock");
        state.wss_bridges.insert(
            bridge_id.clone(),
            WssBridge {
                bridge_id: bridge_id.clone(),
                endpoint_id: endpoint_id_str.clone(),
                peer_addr: peer,
            },
        );
    }
    ctx.metrics.bridge_opened();
    tracing::info!(
        bridge_id = %bridge_id,
        endpoint_id = %endpoint_id_str,
        %peer,
        "WSS bridge opened"
    );

    // The opaque byte pump. On any exit (close/error/idle/shutdown) we tear down.
    let pump_result = pump(ws, send, recv, &ctx.metrics, &ctx.shutdown).await;

    // Teardown: drop the bridge entry, close the Iroh connection, drop the gauge.
    {
        let mut state = ctx.state.lock().expect("relay state lock");
        state.wss_bridges.remove(&bridge_id);
    }
    ctx.metrics.bridge_closed();
    conn.close(0u32.into(), b"wss bridge closed");
    tracing::info!(bridge_id = %bridge_id, endpoint_id = %endpoint_id_str, %peer, "WSS bridge closed");

    pump_result
}

/// The bidirectional **opaque** byte pump (`design/11 §3.4`, §3.9). Two
/// directions over one `tokio::select!`:
///   - WSS **binary** frame payload → Iroh `SendStream` (browser → Core),
///   - Iroh `RecvStream` bytes → WSS **binary** frame (Core → browser).
///
/// It copies `&[u8]` verbatim — it NEVER parses, decodes, decrypts, or
/// length-prefixes the contents. A WSS binary message maps 1:1 onto the Iroh
/// stream bytes and vice versa. Text/ping/pong control frames are handled per the
/// WebSocket protocol but carry no bridged payload. Every wait is idle-timeout
/// bounded.
async fn pump(
    ws: tokio_tungstenite::WebSocketStream<tokio_rustls::server::TlsStream<TcpStream>>,
    mut send: SendStream,
    mut recv: RecvStream,
    metrics: &WssBridgeMetrics,
    shutdown: &CancellationToken,
) -> Result<()> {
    let (mut ws_tx, mut ws_rx) = ws.split();
    let mut iroh_buf = vec![0u8; IROH_READ_CHUNK];

    loop {
        tokio::select! {
            biased;

            _ = shutdown.cancelled() => {
                let _ = send.finish();
                let _ = ws_tx.close().await;
                return Ok(());
            }

            // browser → Core
            ws_msg = tokio::time::timeout(IDLE_TIMEOUT, ws_rx.next()) => {
                let ws_msg = ws_msg.map_err(|_| RelayError::Server("WSS bridge idle timeout".into()))?;
                match ws_msg {
                    Some(Ok(Message::Binary(data))) => {
                        if !data.is_empty() {
                            metrics.add_bytes_to_core(data.len() as u64);
                            send.write_all(&data)
                                .await
                                .map_err(|e| RelayError::Server(format!("write to Iroh stream: {e}")))?;
                        }
                    }
                    // A WSS binary message is the bridged payload; text is not
                    // part of the framing contract (binary frames only,
                    // `design/11 §3.4`) — ignore without inspecting it.
                    Some(Ok(Message::Text(_))) => {}
                    // Control frames: tungstenite auto-replies to ping; we let
                    // close fall through to teardown. None of these carry bridged
                    // bytes.
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Frame(_))) => {}
                    Some(Ok(Message::Close(_))) | None => {
                        let _ = send.finish();
                        return Ok(());
                    }
                    Some(Err(e)) => {
                        let _ = send.finish();
                        return Err(RelayError::Server(format!("WSS recv: {e}")));
                    }
                }
            }

            // Core → browser
            n = tokio::time::timeout(IDLE_TIMEOUT, recv.read(&mut iroh_buf)) => {
                let n = n.map_err(|_| RelayError::Server("WSS bridge idle timeout".into()))?;
                match n {
                    Ok(Some(0)) => {} // spurious; keep going
                    Ok(Some(n)) => {
                        metrics.add_bytes_to_browser(n as u64);
                        // 1:1: the read bytes become exactly one binary WSS frame.
                        ws_tx
                            .send(Message::binary(iroh_buf[..n].to_vec()))
                            .await
                            .map_err(|e| RelayError::Server(format!("WSS send: {e}")))?;
                    }
                    Ok(None) => {
                        // Core closed its stream → close the WSS cleanly.
                        let _ = ws_tx.close().await;
                        return Ok(());
                    }
                    Err(e) => {
                        let _ = ws_tx.close().await;
                        return Err(RelayError::Server(format!("read from Iroh stream: {e}")));
                    }
                }
            }
        }
    }
}

/// Send a clean WebSocket close (best-effort) — used when the Core dial fails so
/// the browser gets a graceful close, not a dropped TCP (`design/11 §3.4`).
async fn close_ws_clean(
    mut ws: tokio_tungstenite::WebSocketStream<tokio_rustls::server::TlsStream<TcpStream>>,
) {
    let _ = ws.close(None).await;
}

/// Parse + validate the `<endpoint_id>` from a `/wss/<endpoint_id>` request path
/// (`design/11 §3.4`). Rejects (with a reason string for the 4xx body) anything
/// that is not the frozen prefix, is oversized, or does not parse as an Iroh
/// endpoint id — **before** any upgrade or dial.
fn parse_endpoint_id(path: &str) -> std::result::Result<EndpointId, String> {
    let rest = path
        .strip_prefix(WSS_PATH_PREFIX)
        .ok_or_else(|| format!("path must start with {WSS_PATH_PREFIX}"))?;
    // No further path segments / query handled here — the id is the whole tail.
    let id_seg = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if id_seg.is_empty() {
        return Err("empty endpoint id".into());
    }
    if id_seg.len() > MAX_ENDPOINT_ID_LEN {
        return Err(format!(
            "endpoint id too long ({} > {MAX_ENDPOINT_ID_LEN})",
            id_seg.len()
        ));
    }
    id_seg
        .parse::<EndpointId>()
        .map_err(|e| format!("not a valid Iroh endpoint id: {e}"))
}

/// A fresh opaque bridge id (the relay-side handle). Combines the peer addr with
/// a process-monotonic counter so it is unique per connection without exposing
/// anything beyond §3.9 metadata.
fn new_bridge_id(peer: SocketAddr) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("wss-{peer}-{n}")
}
