//! Tier-2 integration test for Task 521 — LAN-direct TLS on the Connect-Web
//! bridge (`design/11 §3.4` Path A, `design/17 §3.3`).
//!
//! **Test double:** the real [`connect_bridge`] serve path bound on a loopback
//! port with TLS enabled (the identity-bound self-signed cert derived from a
//! Core identity pubkey), plus a **native pinning client**: a hand-rolled
//! `tokio-rustls` TLS client whose certificate verifier **pins the Core cert**
//! and refuses anything else. Over that pinned TLS connection the client speaks
//! gRPC-Web (HTTP/1.1) and calls `Runtime.GetServerCapabilities`.
//!
//! It proves the load-bearing Task-521 claims:
//!
//! - **(a)** the bridge serves TLS using a cert derived from / pinned to the
//!   Core identity, and a client that **pins** that cert completes the TLS
//!   handshake and **round-trips a real gRPC-Web request** (the response
//!   decodes and reports `transport_kind == WSS_BRIDGE`, so the request flowed
//!   through the same tagged handler set the plain-HTTP bridge serves).
//! - **(b)** a client pinning a **different** (impostor) Core's cert is
//!   **rejected at the TLS handshake** — the "Core identity mismatch" guarantee
//!   (`design/17 §8`).
//! - **(c)** the published SPKI fingerprint ([`BoundBridge::cert_fingerprint`])
//!   is `Some` (the value a native/LAN client pins) and is stable for the
//!   identity.
//!
//! Native vs browser pinning (`design/17 §3.3`, honest posture): a **native /
//! LAN client** can pin programmatically, as this test does. A **browser**
//! cannot be handed an SPKI pin for a self-signed LAN cert at page-load; it
//! clicks through the one-time interstitial and stores a per-site exception (the
//! published fingerprint lets the user *verify* the cert). That browser path is
//! a Playwright/manual Tier-3 item — see the task Handoff.
//!
//! Unix-only gate: this test drives the live bridge serve loop which (like the
//! Task-204 `connect_web_bridge.rs` double) brings up the `BridgeServices`
//! handler set whose streaming services are `#[cfg(unix)]`. We only exercise the
//! cross-platform `Runtime` service, but we gate the whole file to match the
//! sibling bridge test rather than fork the cfg surface.

#![cfg(unix)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use concerto_core::connect_bridge::{self, BridgeServices, ConnectBridgeConfig};
use concerto_core::connect_bridge_tls::IdentityTlsCert;
use concerto_core::supervisor::SupervisorView;
use concerto_identity::{KeyPair, PublicKey};
use concerto_proto::v1::{ServerCapabilities, TransportKind};
use prost::Message;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// gRPC-Web wire helpers (same framing as `connect_web_bridge.rs`).
// ---------------------------------------------------------------------------

/// Encode a protobuf message into a single gRPC-Web DATA frame.
fn grpc_web_frame<M: Message>(msg: &M) -> Vec<u8> {
    let body = msg.encode_to_vec();
    let mut out = Vec::with_capacity(5 + body.len());
    out.push(0u8); // flag: data frame, uncompressed
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

/// Extract the first DATA-frame payload from a gRPC-Web response body.
fn first_grpc_web_message(body: &[u8]) -> Option<Vec<u8>> {
    let mut i = 0usize;
    while i + 5 <= body.len() {
        let flag = body[i];
        let len = u32::from_be_bytes([body[i + 1], body[i + 2], body[i + 3], body[i + 4]]) as usize;
        i += 5;
        if i + len > body.len() {
            break;
        }
        let payload = &body[i..i + len];
        i += len;
        if flag & 0x80 == 0 {
            return Some(payload.to_vec());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// A pinning TLS client cert verifier (the native-client pinning model).
//
// Pins the EXACT leaf certificate DER the Core serves. Certificate pinning is a
// (stronger) variant of SPKI pinning; it avoids an X.509 SPKI re-parse in the
// test while proving the same property: the client trusts ONLY the one Core's
// cert, and an impostor Core (different cert) is rejected.
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct PinnedCertVerifier {
    expected_cert_der: Vec<u8>,
}

impl tokio_rustls::rustls::client::danger::ServerCertVerifier for PinnedCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[tokio_rustls::rustls::pki_types::CertificateDer<'_>],
        _server_name: &tokio_rustls::rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: tokio_rustls::rustls::pki_types::UnixTime,
    ) -> Result<tokio_rustls::rustls::client::danger::ServerCertVerified, tokio_rustls::rustls::Error>
    {
        if end_entity.as_ref() == self.expected_cert_der.as_slice() {
            Ok(tokio_rustls::rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(tokio_rustls::rustls::Error::General(
                "pinned Core cert mismatch (impostor Core?)".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        tokio_rustls::rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &tokio_rustls::rustls::crypto::ring::default_provider()
                .signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &tokio_rustls::rustls::pki_types::CertificateDer<'_>,
        dss: &tokio_rustls::rustls::DigitallySignedStruct,
    ) -> Result<
        tokio_rustls::rustls::client::danger::HandshakeSignatureValid,
        tokio_rustls::rustls::Error,
    > {
        tokio_rustls::rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &tokio_rustls::rustls::crypto::ring::default_provider()
                .signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<tokio_rustls::rustls::SignatureScheme> {
        tokio_rustls::rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// The leaf cert DER the bridge serves for `core_pubkey` (so the client can pin
/// it). Re-derives the identical deterministic cert and pulls its DER out of the
/// PEM the bridge would serve.
fn served_cert_der(core_pubkey: &PublicKey) -> Vec<u8> {
    let sans = vec![
        "localhost".to_string(),
        "concerto.local".to_string(),
        "127.0.0.1".to_string(),
    ];
    let cert = IdentityTlsCert::derive(core_pubkey, &sans).expect("derive cert");
    use tokio_rustls::rustls::pki_types::pem::PemObject;
    use tokio_rustls::rustls::pki_types::CertificateDer;
    CertificateDer::pem_slice_iter(cert.cert_pem().as_bytes())
        .next()
        .expect("one cert in PEM")
        .expect("parse cert PEM")
        .as_ref()
        .to_vec()
}

/// Build a pinning rustls client config for `expected_cert_der`.
fn pinning_client_config(expected_cert_der: Vec<u8>) -> tokio_rustls::rustls::ClientConfig {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    let mut config = tokio_rustls::rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedCertVerifier { expected_cert_der }))
        .with_no_client_auth();
    // gRPC-Web rides HTTP/1.1; ask for it via ALPN so the TLS layer matches.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config
}

/// Bind + serve the bridge with LAN-direct TLS for `core_pubkey`, returning the
/// bound addr, the published fingerprint, and a shutdown token.
async fn serve_tls_bridge(core_pubkey: &PublicKey) -> (SocketAddr, String, CancellationToken) {
    let cfg = ConnectBridgeConfig {
        enabled: true,
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        tls_requested: true,
        tls: None,
    }
    .with_tls_for(core_pubkey)
    .expect("derive TLS for bridge");

    let fingerprint = cfg
        .cert_fingerprint()
        .expect("TLS bridge must publish a fingerprint")
        .to_string();
    let tls = cfg.tls.clone();

    let services = BridgeServices {
        started_at: Arc::new(SystemTime::now()),
        supervisor_view: SupervisorView::default(),
        repo_manager: None,
        workspace_manager: None,
        workarea_manager: None,
        agent_supervisor: None,
        persistence: None,
        scheduler: None,
        skills_registry: None,
        suggestions: None,
        maestro: None,
        vcs: None,
        vcs_privacy_resolver: None,
    };

    let (listener, bound) = connect_bridge::bind(&cfg).await.expect("bind TLS bridge");
    assert_eq!(
        bound.cert_fingerprint.as_deref(),
        Some(fingerprint.as_str()),
        "bound bridge must report the same pin the config derived"
    );
    let addr = bound.local_addr;

    let shutdown = CancellationToken::new();
    let serve_shutdown = shutdown.clone();
    tokio::spawn(async move {
        let _ = connect_bridge::serve(listener, services, tls, serve_shutdown).await;
    });
    // Give the serve loop a beat to start accepting.
    tokio::time::sleep(Duration::from_millis(100)).await;
    (addr, fingerprint, shutdown)
}

/// Open a pinned TLS connection and POST one gRPC-Web unary request over
/// HTTP/1.1, returning the raw response body bytes.
async fn pinned_grpc_web_unary(
    addr: SocketAddr,
    client_config: tokio_rustls::rustls::ClientConfig,
    grpc_path: &str,
    req_frame: &[u8],
) -> std::io::Result<Vec<u8>> {
    use tokio_rustls::rustls::pki_types::ServerName;
    let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));
    let tcp = tokio::net::TcpStream::connect(addr).await?;
    let server_name = ServerName::try_from("localhost").unwrap();
    let mut tls = connector.connect(server_name, tcp).await?;

    // Minimal HTTP/1.1 gRPC-Web POST. Content-Length framed (single unary
    // request), so the server can dispatch without chunked encoding.
    let request = format!(
        "POST {grpc_path} HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: application/grpc-web+proto\r\n\
         Accept: application/grpc-web+proto\r\n\
         X-Grpc-Web: 1\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        req_frame.len()
    );
    tls.write_all(request.as_bytes()).await?;
    tls.write_all(req_frame).await?;
    tls.flush().await?;

    let mut buf = Vec::new();
    tls.read_to_end(&mut buf).await?;
    Ok(buf)
}

/// Split an HTTP/1.1 response into (status_line, decoded_body). Handles
/// `Transfer-Encoding: chunked` (which tonic-web uses for the trailers-as-body
/// gRPC-Web framing) as well as plain bodies.
fn split_http_body(raw: &[u8]) -> (String, Vec<u8>) {
    let sep = b"\r\n\r\n";
    let pos = raw
        .windows(sep.len())
        .position(|w| w == sep)
        .map(|p| p + sep.len())
        .unwrap_or(raw.len());
    let headers = String::from_utf8_lossy(&raw[..pos]).to_string();
    let status_line = headers.lines().next().unwrap_or("").to_string();
    let raw_body = &raw[pos..];

    let is_chunked = headers
        .lines()
        .any(|l| l.to_ascii_lowercase().starts_with("transfer-encoding:") && l.contains("chunked"));

    let body = if is_chunked {
        dechunk(raw_body)
    } else {
        raw_body.to_vec()
    };
    (status_line, body)
}

/// Decode an HTTP/1.1 chunked transfer-encoded body into the raw payload bytes.
fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    // Read each chunk-size line (hex), terminated by CRLF, until the body ends.
    while let Some(line_end) = data.windows(2).position(|w| w == b"\r\n") {
        let size_str = String::from_utf8_lossy(&data[..line_end]);
        // Strip any chunk extensions after ';'.
        let size_hex = size_str.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16).unwrap_or(0);
        data = &data[line_end + 2..];
        if size == 0 {
            break; // last chunk
        }
        if size > data.len() {
            break;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size..];
        // Skip the trailing CRLF after the chunk data.
        if data.len() >= 2 && &data[..2] == b"\r\n" {
            data = &data[2..];
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// (a) A pinning client completes the TLS handshake against the Core-identity
/// cert and round-trips a real gRPC-Web `Runtime.GetServerCapabilities`.
#[tokio::test]
async fn pinned_client_round_trips_over_lan_direct_tls() {
    let core_pubkey = KeyPair::from_seed(&[42u8; 32]).verifying_key();
    let (addr, fingerprint, shutdown) = serve_tls_bridge(&core_pubkey).await;

    // (c) the published pin is a 64-char SHA-256 hex.
    assert_eq!(fingerprint.len(), 64);

    let pin = served_cert_der(&core_pubkey);
    // `Runtime.GetServerCapabilities` takes `google.protobuf.Empty`, which
    // encodes to zero bytes — frame an empty payload.
    let frame = grpc_web_frame(&());

    let raw = tokio::time::timeout(
        Duration::from_secs(10),
        pinned_grpc_web_unary(
            addr,
            pinning_client_config(pin),
            "/concerto.v1.Runtime/GetServerCapabilities",
            &frame,
        ),
    )
    .await
    .expect("pinned gRPC-Web call did not hang")
    .expect("pinned TLS client must connect + round-trip");

    let (status_line, body) = split_http_body(&raw);
    assert!(
        status_line.contains("200"),
        "expected HTTP 200 over pinned TLS, got status line: {status_line:?}"
    );
    let msg = first_grpc_web_message(&body).expect("a gRPC-Web message frame in the response");
    let caps = ServerCapabilities::decode(&msg[..]).expect("decode ServerCapabilities");
    assert_eq!(
        caps.transport_kind(),
        TransportKind::WssBridge,
        "the request must have flowed through the bridge's WSS_BRIDGE-tagged handler set"
    );

    shutdown.cancel();
}

/// (b) A client pinning a DIFFERENT Core's cert is rejected at the TLS
/// handshake — the impostor-Core ("Core identity mismatch") guarantee.
#[tokio::test]
async fn impostor_pin_is_rejected_at_handshake() {
    let core_pubkey = KeyPair::from_seed(&[1u8; 32]).verifying_key();
    let (addr, _fingerprint, shutdown) = serve_tls_bridge(&core_pubkey).await;

    // Pin a DIFFERENT Core's cert; the served cert won't match → handshake fail.
    let other_pubkey = KeyPair::from_seed(&[2u8; 32]).verifying_key();
    let wrong_pin = served_cert_der(&other_pubkey);

    let frame = grpc_web_frame(&());
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        pinned_grpc_web_unary(
            addr,
            pinning_client_config(wrong_pin),
            "/concerto.v1.Runtime/GetServerCapabilities",
            &frame,
        ),
    )
    .await
    .expect("handshake attempt did not hang");

    assert!(
        result.is_err(),
        "a client pinning a different Core's cert must be rejected at the TLS handshake"
    );

    shutdown.cancel();
}

/// (c) The published fingerprint is stable for a given Core identity (pinned
/// clients keep trusting across restarts).
#[tokio::test]
async fn fingerprint_is_stable_for_identity() {
    let core_pubkey = KeyPair::from_seed(&[7u8; 32]).verifying_key();
    let (_addr1, fp1, sd1) = serve_tls_bridge(&core_pubkey).await;
    let (_addr2, fp2, sd2) = serve_tls_bridge(&core_pubkey).await;
    assert_eq!(fp1, fp2, "same identity ⇒ same pin across (re)binds");
    sd1.cancel();
    sd2.cancel();
}
