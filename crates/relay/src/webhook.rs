//! The inbound-webhook route `POST /webhook/github/<endpoint_id>` (`design/11
//! §3.4.1`, `design/13 §3.2`, Task 315) — a sibling of the WSS bridge (`§3.4`).
//!
//! GitHub POSTs a webhook to the relay; the relay opens **one** ephemeral `0x04`
//! Webhook Iroh bidi to the addressed Core, writes the FROZEN `WebhookEnvelope`,
//! reads the Core's one-byte ack, maps it to an HTTP status, and closes the
//! stream. Per request the route:
//!
//!   1. terminates the outer TLS the `https://` scheme requires (operator cert,
//!      or an ephemeral self-signed pair for the loopback double),
//!   2. parses `POST /webhook/github/<endpoint_id>` into an Iroh endpoint id
//!      (rejecting a malformed/oversized id with HTTP `400` **before** dialing),
//!   3. reads the GitHub headers (`X-GitHub-Delivery`, `X-Hub-Signature-256`,
//!      `X-GitHub-Event`) + the raw body (bounded to the 25 MiB ceiling — an
//!      oversized POST is `413`'d before any dial),
//!   4. opens the `0x04` bidi (relay-forced dial, `RelayMode::Custom`), writes
//!      the envelope, reads the ack, returns `200`/`4xx`/`5xx`.
//!
//! # Transparent forwarder (`design/11 §3.9`)
//!
//! The relay does **no** HMAC verify, **no** parse, **no** persistence, and never
//! sees the per-repo HMAC secret. It forwards GitHub's already-signed body
//! opaquely; the authenticity floor is the Core-verified HMAC, not relay trust.
//! The `0x04` channel runs **no** Noise (the peer is GitHub-via-relay, not a
//! paired device); the relay→Core hop is encrypted by the Iroh QUIC layer.
//!
//! # Offline Core (`design/11 §3.4.1`, `design/13 §8`)
//!
//! If the dial to `<endpoint_id>` fails (no route / Core down), the relay
//! **drops + logs** (endpoint id + delivery id + timestamp; never the body) and
//! returns `503` to GitHub. It does **not** buffer — it stays near-stateless
//! (`§3.2`). GitHub redelivers per its own policy.
//!
//! # One Iroh endpoint, relay-forced
//!
//! Like the WSS bridge, the route holds its own Iroh client [`Endpoint`]
//! configured to route through the relay's URL (`RelayMode::Custom`) — there is
//! no dialable endpoint on `iroh-relay::Server` (the 215 Handoff drift). The
//! `0x04` framing is written inline over the raw bidi, mirroring the FROZEN
//! `concerto_transport::webhook` contract (the same local-declaration discipline
//! `wss.rs` uses for the transport ALPN — the relay stays dependency-light).
//!
//! # Cross-platform
//!
//! TLS is rustls/ring; nothing here is `#[cfg(unix)]`. The Windows CI lane (Task
//! 113) builds it as-is, like the WSS bridge.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::{presets, Connection, RecvStream, SendStream};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;

use crate::api::{
    WebhookRouteMetrics, WebhookRouteServer, WssTlsConfig, MAX_ENDPOINT_ID_LEN,
    MAX_WEBHOOK_BODY_SIZE, WEBHOOK_PATH_PREFIX,
};
use crate::error::{RelayError, Result};

/// The Iroh ALPN the route dials Cores with. **Must match**
/// `concerto_transport::ALPN` (`b"concerto/transport/1"`, Task 212), like the WSS
/// bridge. Declared locally (no `concerto-transport` dep) so the relay binary
/// stays dependency-light; the value is FROZEN by the transport crate.
const TRANSPORT_ALPN: &[u8] = b"concerto/transport/1";

/// The `0x04` Webhook channel-tag byte (`concerto_transport::ChannelTag::Webhook`,
/// `design/11 §3.3`/§3.4.1). Written as the acceptor-priming first byte on the
/// bidi. FROZEN by the transport crate; declared locally per the dependency-light
/// discipline above.
const WEBHOOK_CHANNEL_TAG: u8 = 0x04;

/// How long the per-request dial to the addressed Core may take before the route
/// gives up and returns `503` (offline-Core, `design/11 §3.4.1`). Mirrors the WSS
/// bridge's `DIAL_TIMEOUT`.
const DIAL_TIMEOUT: Duration = Duration::from_secs(20);

/// Bound on the whole envelope-write → ack-read exchange once the bidi is open,
/// so a stalled Core can't pin the request task.
const ACK_TIMEOUT: Duration = Duration::from_secs(60);

/// Bound on the outer TLS handshake + HTTP request read, so a slow/abusive client
/// can't pin a task.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Bound on waiting for the route's client endpoint to register with its home
/// relay at `start` (so the first relay-forced dial can succeed). On a slow/no
/// relay we serve anyway after this and let individual dials 503.
const ENDPOINT_ONLINE_TIMEOUT: Duration = Duration::from_secs(20);

/// Private internals behind [`WebhookRouteServer`].
pub struct WebhookRouteInner {
    local_addr: SocketAddr,
    endpoint: Endpoint,
    relay_url: RelayUrl,
    metrics: WebhookRouteMetrics,
    shutdown: CancellationToken,
}

impl WebhookRouteServer {
    /// Start the webhook route: bind the TLS listener on `listen_addr`, build the
    /// relay-forced Iroh client endpoint (dialing through `relay_url`), and spawn
    /// the accept loop. Returns once bound; the loop runs in the background until
    /// `shutdown` fires.
    pub async fn start(
        listen_addr: SocketAddr,
        relay_url: RelayUrl,
        tls: WssTlsConfig,
        metrics: WebhookRouteMetrics,
        shutdown: CancellationToken,
    ) -> Result<Self> {
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();

        let server_config = tls.rustls_server_config()?;
        let acceptor = TlsAcceptor::from(Arc::new(server_config));

        let listener = TcpListener::bind(listen_addr).await.map_err(|e| {
            RelayError::Server(format!("binding webhook listener {listen_addr}: {e}"))
        })?;
        let local_addr = listener
            .local_addr()
            .map_err(|e| RelayError::Server(format!("reading webhook listener addr: {e}")))?;

        let map = RelayMap::from_iter([relay_url.clone()]);
        let endpoint = Endpoint::builder(presets::N0)
            .alpns(vec![TRANSPORT_ALPN.to_vec()])
            .relay_mode(RelayMode::Custom(map))
            .bind()
            .await
            .map_err(|e| {
                RelayError::Server(format!("binding webhook route Iroh client endpoint: {e}"))
            })?;

        // Wait (bounded) until the route's client endpoint has registered with
        // its home relay before accepting POSTs: a relay-forced dial to a
        // relay-only Core cannot succeed until this endpoint itself has a relay
        // path, so accepting a webhook before then would 503 a deliverable
        // payload (the WSS bridge tolerates this only because its bidi is
        // long-lived; the webhook route's dial is one-shot per request).
        tokio::select! {
            _ = endpoint.online() => {}
            _ = tokio::time::sleep(ENDPOINT_ONLINE_TIMEOUT) => {
                tracing::warn!(
                    "webhook route endpoint not relay-online within {ENDPOINT_ONLINE_TIMEOUT:?}; \
                     serving anyway (dials may 503 until the relay path establishes)"
                );
            }
        }

        let inner = WebhookRouteInner {
            local_addr,
            endpoint,
            relay_url,
            metrics,
            shutdown,
        };

        spawn_accept_loop(listener, acceptor, &inner);

        tracing::info!(
            webhook_listen = %local_addr,
            relay = %inner.relay_url,
            "concerto-relay webhook route started (POST /webhook/github/<endpoint_id>)"
        );

        Ok(Self { inner })
    }

    /// The bound webhook listener address (`127.0.0.1:0` resolves to the
    /// OS-assigned port in tests).
    pub fn local_addr(&self) -> SocketAddr {
        self.inner.local_addr
    }

    /// Tear the route down: cancel the accept loop + close the Iroh client
    /// endpoint.
    pub async fn shutdown(self) -> Result<()> {
        self.inner.shutdown.cancel();
        self.inner.endpoint.close().await;
        Ok(())
    }
}

/// The per-request context cloned into each handler.
#[derive(Clone)]
struct RouteCtx {
    endpoint: Endpoint,
    relay_url: RelayUrl,
    metrics: WebhookRouteMetrics,
    shutdown: CancellationToken,
}

fn spawn_accept_loop(listener: TcpListener, acceptor: TlsAcceptor, inner: &WebhookRouteInner) {
    let ctx = RouteCtx {
        endpoint: inner.endpoint.clone(),
        relay_url: inner.relay_url.clone(),
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
                                    // Metadata-only: never the body (§3.9).
                                    tracing::debug!(%peer, %err, "webhook route connection ended");
                                }
                            });
                        }
                        Err(err) => {
                            tracing::warn!(%err, "webhook route accept failed");
                            break;
                        }
                    }
                }
            }
        }
    });
}

/// Terminate TLS, parse the request, forward over `0x04`, and write the HTTP
/// response.
async fn serve_connection(
    tcp: TcpStream,
    peer: SocketAddr,
    acceptor: TlsAcceptor,
    ctx: RouteCtx,
) -> Result<()> {
    let mut tls = tokio::time::timeout(READ_TIMEOUT, acceptor.accept(tcp))
        .await
        .map_err(|_| RelayError::Server("webhook TLS handshake timed out".into()))?
        .map_err(|e| RelayError::Server(format!("webhook TLS handshake: {e}")))?;

    // Parse the HTTP/1.1 request (head + bounded body). A bad request / bad path
    // / oversized body is answered with the matching 4xx BEFORE any dial.
    let request = match tokio::time::timeout(READ_TIMEOUT, read_http_request(&mut tls)).await {
        Ok(Ok(req)) => req,
        Ok(Err(resp)) => {
            // A parse-level rejection (bad method/path/oversize) carries its own
            // HTTP status; write it and return.
            write_response(&mut tls, resp).await?;
            return Ok(());
        }
        Err(_) => {
            write_response(&mut tls, HttpStatus::BadRequest).await?;
            return Ok(());
        }
    };

    let status = forward_to_core(&ctx, &request, peer).await;
    write_response(&mut tls, status).await?;
    Ok(())
}

/// The parsed inbound webhook request (the fields the envelope needs).
struct WebhookRequest {
    endpoint_id: EndpointId,
    endpoint_id_str: String,
    delivery_id: String,
    signature_256: String,
    event_type: String,
    body: Vec<u8>,
}

/// The HTTP statuses the route returns to GitHub (`design/11 §3.4.1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HttpStatus {
    /// `200` — Core accepted (ack `0x00`).
    Ok,
    /// `400` — malformed request / bad path / Core 4xx reject (ack `0x01`).
    BadRequest,
    /// `413` — body exceeded the 25 MiB ceiling (rejected before dialing).
    PayloadTooLarge,
    /// `500` — Core-internal error after a valid frame (ack `0x02`).
    InternalError,
    /// `503` — the addressed Core is offline (dial failed) — drop + log, no
    /// buffering.
    ServiceUnavailable,
}

impl HttpStatus {
    fn line(self) -> &'static str {
        match self {
            HttpStatus::Ok => "200 OK",
            HttpStatus::BadRequest => "400 Bad Request",
            HttpStatus::PayloadTooLarge => "413 Payload Too Large",
            HttpStatus::InternalError => "500 Internal Server Error",
            HttpStatus::ServiceUnavailable => "503 Service Unavailable",
        }
    }
}

/// Read + parse one HTTP/1.1 request: the request line (`POST
/// /webhook/github/<id>`), the headers (the three GitHub headers plus
/// Content-Length and Host), and the body (bounded to the 25 MiB ceiling).
/// Returns `Err(status)` for a parse-level rejection the caller answers verbatim.
async fn read_http_request(
    tls: &mut TlsStream<TcpStream>,
) -> std::result::Result<WebhookRequest, HttpStatus> {
    // Read until the end of the header block (`\r\n\r\n`), keeping any body bytes
    // that arrived in the same read. Bounded header size.
    const MAX_HEADER: usize = 64 * 1024;
    let mut buf = Vec::with_capacity(8192);
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > MAX_HEADER {
            return Err(HttpStatus::BadRequest);
        }
        let n = tls
            .read(&mut chunk)
            .await
            .map_err(|_| HttpStatus::BadRequest)?;
        if n == 0 {
            return Err(HttpStatus::BadRequest); // EOF before a full header block.
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let head = std::str::from_utf8(&buf[..header_end]).map_err(|_| HttpStatus::BadRequest)?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or(HttpStatus::BadRequest)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or(HttpStatus::BadRequest)?;
    let path = parts.next().ok_or(HttpStatus::BadRequest)?;
    if method != "POST" {
        return Err(HttpStatus::BadRequest);
    }
    let (endpoint_id, endpoint_id_str) = parse_endpoint_id(path)?;

    // Headers (case-insensitive names).
    let mut delivery_id = String::new();
    let mut signature_256 = String::new();
    let mut event_type = String::new();
    let mut content_length: usize = 0;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "x-github-delivery" => delivery_id = value.to_string(),
            "x-hub-signature-256" => signature_256 = value.to_string(),
            "x-github-event" => event_type = value.to_string(),
            "content-length" => {
                content_length = value.parse().map_err(|_| HttpStatus::BadRequest)?;
            }
            _ => {}
        }
    }

    if content_length > MAX_WEBHOOK_BODY_SIZE {
        return Err(HttpStatus::PayloadTooLarge);
    }

    // Body: the bytes already read past the header block, plus the rest up to
    // Content-Length.
    let mut body = buf[header_end + 4..].to_vec();
    if body.len() > MAX_WEBHOOK_BODY_SIZE {
        return Err(HttpStatus::PayloadTooLarge);
    }
    while body.len() < content_length {
        let n = tls
            .read(&mut chunk)
            .await
            .map_err(|_| HttpStatus::BadRequest)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
        if body.len() > MAX_WEBHOOK_BODY_SIZE {
            return Err(HttpStatus::PayloadTooLarge);
        }
    }
    body.truncate(content_length);

    Ok(WebhookRequest {
        endpoint_id,
        endpoint_id_str,
        delivery_id,
        signature_256,
        event_type,
        body,
    })
}

/// Open the `0x04` bidi, write the envelope, read the ack, and map it to an HTTP
/// status (`design/11 §3.4.1`). An offline Core (dial fail) → `503` + drop + log
/// + metric; the relay never buffers.
async fn forward_to_core(ctx: &RouteCtx, req: &WebhookRequest, peer: SocketAddr) -> HttpStatus {
    let dial = async {
        let addr = EndpointAddr::new(req.endpoint_id).with_relay_url(ctx.relay_url.clone());
        let conn: Connection = ctx
            .endpoint
            .connect(addr, TRANSPORT_ALPN)
            .await
            .map_err(|e| {
                RelayError::Server(format!("dialing Core {}: {e}", req.endpoint_id_str))
            })?;
        let (send, recv) = conn
            .open_bi()
            .await
            .map_err(|e| RelayError::Server(format!("opening 0x04 bidi to Core: {e}")))?;
        Ok::<_, RelayError>((conn, send, recv))
    };
    let (conn, mut send, mut recv) = match tokio::time::timeout(DIAL_TIMEOUT, dial).await {
        Ok(Ok(parts)) => parts,
        Ok(Err(_)) | Err(_) => {
            // Offline Core: drop + log (metadata only, never the body) + 503.
            ctx.metrics.dropped();
            tracing::warn!(
                endpoint_id = %req.endpoint_id_str,
                delivery_id = %req.delivery_id,
                %peer,
                "webhook route: Core offline / dial failed; dropping (503, no buffering)"
            );
            return HttpStatus::ServiceUnavailable;
        }
    };

    let exchange = async {
        write_envelope(&mut send, req).await?;
        let ack = read_ack(&mut recv).await?;
        Ok::<_, RelayError>(ack)
    };
    let status = match tokio::time::timeout(ACK_TIMEOUT, exchange).await {
        Ok(Ok(ack)) => {
            ctx.metrics.forwarded();
            tracing::debug!(
                endpoint_id = %req.endpoint_id_str,
                delivery_id = %req.delivery_id,
                event = %req.event_type,
                ?ack,
                "webhook route: forwarded; chaining ack to GitHub"
            );
            match ack {
                Ack::Accepted => HttpStatus::Ok,
                Ack::Reject => HttpStatus::BadRequest,
                Ack::Error => HttpStatus::InternalError,
            }
        }
        other => {
            // The bidi opened but the exchange failed (`Ok(Err(_))` early close /
            // unknown ack) or timed out (`Err(_)`). Per §3.4.1: 5xx + drop + log.
            if let Ok(Err(e)) = &other {
                tracing::debug!(error = %e, "webhook route: 0x04 exchange error detail");
            }
            ctx.metrics.dropped();
            tracing::warn!(
                endpoint_id = %req.endpoint_id_str,
                delivery_id = %req.delivery_id,
                %peer,
                "webhook route: 0x04 exchange failed; dropping (503)"
            );
            HttpStatus::ServiceUnavailable
        }
    };

    conn.close(0u32.into(), b"webhook delivered");
    status
}

/// The Core's single-byte ack (`design/11 §3.4.1`), mirroring
/// `concerto_transport::WebhookAck`. Declared locally per the dependency-light
/// discipline.
#[derive(Debug, Clone, Copy)]
enum Ack {
    Accepted,
    Reject,
    Error,
}

/// Write the `0x04` channel-tag byte then the FROZEN `WebhookEnvelope` framing
/// (`design/11 §3.4.1`): five big-endian `u32`-length-prefixed fields. Mirrors
/// `concerto_transport::webhook::write_envelope`.
async fn write_envelope(send: &mut SendStream, req: &WebhookRequest) -> Result<()> {
    let mut frame = Vec::new();
    frame.push(WEBHOOK_CHANNEL_TAG);
    push_field(&mut frame, req.delivery_id.as_bytes());
    push_field(&mut frame, req.signature_256.as_bytes());
    push_field(&mut frame, req.event_type.as_bytes());
    push_field(&mut frame, req.endpoint_id_str.as_bytes());
    push_field(&mut frame, &req.body);
    send.write_all(&frame)
        .await
        .map_err(|e| RelayError::Server(format!("writing 0x04 envelope: {e}")))?;
    send.finish()
        .map_err(|e| RelayError::Server(format!("finishing 0x04 send: {e}")))?;
    Ok(())
}

/// Push one big-endian `u32` length + the bytes.
fn push_field(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(bytes);
}

/// Read the Core's single-byte ack. Any byte other than the three FROZEN values
/// (or an early EOF) is an error the caller maps to `503` + drop.
async fn read_ack(recv: &mut RecvStream) -> Result<Ack> {
    let mut b = [0u8; 1];
    recv.read_exact(&mut b)
        .await
        .map_err(|e| RelayError::Server(format!("reading 0x04 ack: {e}")))?;
    match b[0] {
        0x00 => Ok(Ack::Accepted),
        0x01 => Ok(Ack::Reject),
        0x02 => Ok(Ack::Error),
        other => Err(RelayError::Server(format!(
            "unknown ack byte 0x{other:02x}"
        ))),
    }
}

/// Write a minimal HTTP/1.1 response (the status + a tiny body) and close.
async fn write_response(tls: &mut TlsStream<TcpStream>, status: HttpStatus) -> Result<()> {
    let body = format!("{}\n", status.line());
    let resp = format!(
        "HTTP/1.1 {}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        status.line(),
        body.len()
    );
    tls.write_all(resp.as_bytes())
        .await
        .map_err(|e| RelayError::Server(format!("writing webhook HTTP response: {e}")))?;
    tls.flush()
        .await
        .map_err(|e| RelayError::Server(format!("flushing webhook HTTP response: {e}")))?;
    Ok(())
}

/// Parse + validate the `<endpoint_id>` from a `/webhook/github/<endpoint_id>`
/// request path (`design/11 §3.4.1`), rejecting (HTTP `400`) anything that is not
/// the frozen prefix, is oversized, or does not parse as an Iroh endpoint id —
/// **before** any dial. Mirrors the WSS bridge's `parse_endpoint_id`.
fn parse_endpoint_id(path: &str) -> std::result::Result<(EndpointId, String), HttpStatus> {
    let rest = path
        .strip_prefix(WEBHOOK_PATH_PREFIX)
        .ok_or(HttpStatus::BadRequest)?;
    let id_seg = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if id_seg.is_empty() || id_seg.len() > MAX_ENDPOINT_ID_LEN {
        return Err(HttpStatus::BadRequest);
    }
    let id = id_seg
        .parse::<EndpointId>()
        .map_err(|_| HttpStatus::BadRequest)?;
    Ok((id, id_seg.to_string()))
}

/// Find the first index of `needle` in `haystack` (small, for the `\r\n\r\n`
/// header terminator).
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parses_valid_path() {
        // A real Iroh endpoint id round-trips; the prefix is FROZEN. Bind a
        // throwaway endpoint to mint a syntactically valid id (the same way the
        // WSS-bridge tests do), then close it.
        let ep = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .expect("bind endpoint");
        let id = ep.id();
        let id_hex = id.to_string();
        ep.close().await;

        let path = format!("{WEBHOOK_PATH_PREFIX}{id_hex}");
        let (parsed, parsed_str) = parse_endpoint_id(&path).expect("valid path parses");
        assert_eq!(parsed_str, id_hex);
        assert_eq!(parsed, id);
    }

    #[test]
    fn rejects_bad_paths() {
        assert!(parse_endpoint_id("/wss/abc").is_err()); // wrong prefix
        assert!(parse_endpoint_id(WEBHOOK_PATH_PREFIX).is_err()); // empty id
        assert!(parse_endpoint_id(&format!("{WEBHOOK_PATH_PREFIX}not-an-id")).is_err());
        let oversized = format!(
            "{WEBHOOK_PATH_PREFIX}{}",
            "a".repeat(MAX_ENDPOINT_ID_LEN + 1)
        );
        assert!(parse_endpoint_id(&oversized).is_err());
    }

    #[test]
    fn finds_header_terminator() {
        assert_eq!(find_subslice(b"abc\r\n\r\ndef", b"\r\n\r\n"), Some(3));
        assert_eq!(find_subslice(b"no terminator", b"\r\n\r\n"), None);
    }
}
