//! Tier-2 loopback tests for Task 207 — the device-pairing ceremony.
//!
//! **Test double:** two **in-process endpoints** — a *device* side and the
//! *Core* side — complete the full Noise XX pairing over a `tokio::io::duplex`
//! channel (the loopback duplex), then the Core's
//! [`PairingCoordinator`](concerto_core::security::pairing::PairingCoordinator)
//! verifies the device's signature, consumes the one-shot token, mints a
//! `DeviceCert` via Task 206's `LocalCoreIssuer`, and INSERTs the `devices`
//! row. The issuer + persistence are constructed directly in-test (no
//! `boot::start`, no keychain), so the **KEYCHAIN-IN-CI hazard does not apply**
//! to this file.
//!
//! It proves: token mint / 60 s TTL / one-shot consume / ≤ 3-active eviction,
//! the real Noise XX handshake over a byte duplex, the FROZEN
//! `pairing_token || nonce || device_pubkey` signature verification, cert
//! issuance via 206 (the issued cert `validate`s), the `devices` INSERT, and
//! replay / expiry / bad-signature rejection.
//!
//! What this double does **NOT** cover (→ Phase-2 Tier-3 manual checklist line
//! "pair a real second machine over LAN (mDNS direct)"): real cross-device
//! QR-scan pairing over a real Iroh LAN/relay transport — no NAT, no camera, no
//! real endpoint, no relay. Those are physical/external and are signed off at
//! the phase gate.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use concerto_core::security::pairing::{
    CompletePairingInput, PairingCoordinator, PAIRING_NONCE_LEN, PAIRING_TOKEN_TTL,
};
use concerto_identity::{
    new_revoked_set, verify_cert, DeviceCertIssuer, KeyPair, LocalCoreIssuer, NoiseHandshake,
    PublicKey,
};
use concerto_persist::{Persistence, PersistenceConfig};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

// ---------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------

/// A fresh in-memory-on-disk `Persistence` (the `devices` table exists from
/// migration 0001) plus a `LocalCoreIssuer` from a fixed Core seed.
async fn fixtures() -> (TempDir, Arc<Persistence>, PublicKey, LocalCoreIssuer) {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("concerto.db");
    let cfg = PersistenceConfig {
        db_path,
        max_readers: 2,
    };
    let persistence = Arc::new(Persistence::open(cfg).await.expect("open persistence"));

    let core_seed = [0x11u8; 32];
    let core_pub = KeyPair::from_seed(&core_seed).verifying_key();
    let issuer = LocalCoreIssuer::new(KeyPair::from_seed(&core_seed), core_pub, new_revoked_set());
    (tmp, persistence, core_pub, issuer)
}

fn make_coordinator(persistence: Arc<Persistence>, issuer: LocalCoreIssuer) -> PairingCoordinator {
    PairingCoordinator::new(
        issuer,
        persistence,
        concerto_core::audit::AuditWriter::noop(),
        String::new(),
        String::new(),
    )
}

// ---------------------------------------------------------------------------
// Length-framed message helpers for the loopback duplex.
//
// Each side writes a 4-byte big-endian length prefix then the bytes. This is
// the trivial framing the loopback double uses to carry the Noise handshake
// messages + the encrypted PairingRequest; the production transport (Iroh)
// supplies its own framing.
// ---------------------------------------------------------------------------

async fn send_frame(stream: &mut DuplexStream, bytes: &[u8]) {
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await
        .expect("write len");
    stream.write_all(bytes).await.expect("write body");
    stream.flush().await.expect("flush");
}

async fn recv_frame(stream: &mut DuplexStream) -> Vec<u8> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).await.expect("read len");
    let n = u32::from_be_bytes(len) as usize;
    let mut buf = vec![0u8; n];
    stream.read_exact(&mut buf).await.expect("read body");
    buf
}

/// The device's signed `PairingRequest`, serialized for the encrypted
/// transport frame. Layout: `device_pubkey(32) || nonce(32) || signature(64) ||
/// device_name(utf8)`.
fn encode_pairing_request(
    device_pubkey: &[u8; 32],
    nonce: &[u8; 32],
    signature: &[u8; 64],
    device_name: &str,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(device_pubkey);
    out.extend_from_slice(nonce);
    out.extend_from_slice(signature);
    out.extend_from_slice(device_name.as_bytes());
    out
}

fn decode_pairing_request(bytes: &[u8]) -> ([u8; 32], [u8; 32], [u8; 64], String) {
    let device_pubkey: [u8; 32] = bytes[0..32].try_into().unwrap();
    let nonce: [u8; 32] = bytes[32..64].try_into().unwrap();
    let signature: [u8; 64] = bytes[64..128].try_into().unwrap();
    let device_name = String::from_utf8(bytes[128..].to_vec()).unwrap();
    (device_pubkey, nonce, signature, device_name)
}

/// Build the FROZEN signed payload + sign it with the device key.
fn sign_pairing(
    device_key: &KeyPair,
    pairing_token: &[u8; 32],
    nonce: &[u8; 32],
    device_pubkey: &[u8; 32],
) -> [u8; 64] {
    let mut payload = Vec::with_capacity(96);
    payload.extend_from_slice(pairing_token);
    payload.extend_from_slice(nonce);
    payload.extend_from_slice(device_pubkey);
    device_key.sign(&payload).to_bytes()
}

/// Run the full loopback pairing: the device side runs the Noise XX initiator
/// over `device_stream`, sends its encrypted PairingRequest; the Core side runs
/// the responder over `core_stream`, decrypts, and returns the decoded
/// [`CompletePairingInput`]. The two streams are the two ends of one
/// `tokio::io::duplex`. Returns the input the Core would hand to the
/// coordinator (the test then drives `complete_pairing_at`).
async fn loopback_handshake(
    pairing_token: [u8; 32],
    device_key: &KeyPair,
    device_name: &str,
    nonce: [u8; 32],
    // Allow a deliberately-wrong signing key for the bad-sig test.
    sign_with: &KeyPair,
) -> CompletePairingInput {
    let (mut device_stream, mut core_stream) = tokio::io::duplex(64 * 1024);
    let device_pubkey = device_key.verifying_key().to_bytes();

    // Device task: Noise XX initiator + send encrypted PairingRequest.
    let device_token = pairing_token;
    let device_name_owned = device_name.to_string();
    let sign_bytes = sign_pairing(sign_with, &device_token, &nonce, &device_pubkey);
    let device = tokio::spawn(async move {
        let mut hs = NoiseHandshake::initiator(&device_token).expect("initiator");
        // -> e
        let m1 = hs.write_message(&[]).expect("m1");
        send_frame(&mut device_stream, &m1).await;
        // <- e, ee, s, es
        let m2 = recv_frame(&mut device_stream).await;
        hs.read_message(&m2).expect("read m2");
        // -> s, se
        let m3 = hs.write_message(&[]).expect("m3");
        send_frame(&mut device_stream, &m3).await;

        let mut transport = hs.into_transport().expect("transport");
        let req = encode_pairing_request(&device_pubkey, &nonce, &sign_bytes, &device_name_owned);
        let ct = transport.write_message(&req).expect("encrypt req");
        send_frame(&mut device_stream, &ct).await;
    });

    // Core task: Noise XX responder + decrypt the PairingRequest.
    let core_token = pairing_token;
    let core: tokio::task::JoinHandle<CompletePairingInput> = tokio::spawn(async move {
        let mut hs = NoiseHandshake::responder(&core_token).expect("responder");
        // -> e
        let m1 = recv_frame(&mut core_stream).await;
        hs.read_message(&m1).expect("read m1");
        // <- e, ee, s, es
        let m2 = hs.write_message(&[]).expect("m2");
        send_frame(&mut core_stream, &m2).await;
        // -> s, se
        let m3 = recv_frame(&mut core_stream).await;
        hs.read_message(&m3).expect("read m3");

        let mut transport = hs.into_transport().expect("transport");
        let ct = recv_frame(&mut core_stream).await;
        let req = transport.read_message(&ct).expect("decrypt req");
        let (device_pubkey, nonce, signature, device_name) = decode_pairing_request(&req);
        CompletePairingInput {
            device_pubkey,
            device_name,
            nonce: nonce.to_vec(),
            signature,
            pairing_token: core_token.to_vec(),
        }
    });

    device.await.expect("device task");
    core.await.expect("core task")
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

/// Happy path: full Noise XX pairing over the loopback duplex yields a valid
/// `SignedDeviceCert` that 206's `validate` accepts, and a `devices` row
/// exists.
#[tokio::test]
async fn loopback_happy_path_issues_validatable_cert_and_inserts_row() {
    let (_tmp, persistence, core_pub, issuer) = fixtures().await;
    // A second issuer (same Core key + a fresh revoked set) for validation.
    let validator = LocalCoreIssuer::new(
        KeyPair::from_seed(&[0x11u8; 32]),
        core_pub,
        new_revoked_set(),
    );
    let coordinator = make_coordinator(Arc::clone(&persistence), issuer);

    let now = SystemTime::now();
    let challenge = coordinator.start_pairing_at(now).expect("start");

    let device_key = KeyPair::from_seed(&[0x22u8; 32]);
    let nonce = [0x33u8; PAIRING_NONCE_LEN];
    let input = loopback_handshake(
        challenge.pairing_token,
        &device_key,
        "Loopback Phone",
        nonce,
        &device_key,
    )
    .await;

    let device_id = concerto_identity::device_id(&device_key.verifying_key().to_bytes());
    let outcome = coordinator
        .complete_pairing_at(input, now + Duration::from_secs(1))
        .await
        .expect("complete pairing");

    // The Core's pubkey is echoed.
    assert_eq!(outcome.core_pubkey, core_pub.to_bytes());

    // The returned bytes are the on-wire `cert_bytes || signature` form and
    // validate against the Core key (206's hot path).
    let cert = verify_cert(&outcome.signed_device_cert, &core_pub).expect("verify_cert");
    assert_eq!(cert.device_pubkey, device_key.verifying_key().to_bytes());
    assert_eq!(cert.device_name, "Loopback Phone");
    let ctx = validator
        .validate(&outcome.signed_device_cert)
        .expect("validate");
    assert_eq!(ctx.device_id, device_id);
    assert_eq!(ctx.capabilities, vec!["admin".to_string()]);

    // A `devices` row exists, keyed by the device_id hex fingerprint.
    let id_hex = hex::encode(device_id);
    let row: (String, Vec<u8>) =
        sqlx::query_as("SELECT name, public_key FROM devices WHERE id = ?")
            .bind(&id_hex)
            .fetch_one(persistence.readers())
            .await
            .expect("devices row exists");
    assert_eq!(row.0, "Loopback Phone");
    assert_eq!(row.1, device_key.verifying_key().to_bytes().to_vec());
}

/// One-shot: a second `CompletePairing` with the same token is rejected with
/// `pairing.consumed`.
#[tokio::test]
async fn token_is_one_shot() {
    let (_tmp, persistence, _core_pub, issuer) = fixtures().await;
    let coordinator = make_coordinator(persistence, issuer);

    let now = SystemTime::now();
    let challenge = coordinator.start_pairing_at(now).expect("start");
    let device_key = KeyPair::from_seed(&[0x44u8; 32]);
    let nonce = [0x55u8; PAIRING_NONCE_LEN];

    let first = loopback_handshake(
        challenge.pairing_token,
        &device_key,
        "Phone",
        nonce,
        &device_key,
    )
    .await;
    coordinator
        .complete_pairing_at(first, now)
        .await
        .expect("first pairing succeeds");

    // Replay the same token (rebuild a fresh signed input — the token bytes are
    // identical, which is what the one-shot store rejects).
    let signature = sign_pairing(
        &device_key,
        &challenge.pairing_token,
        &nonce,
        &device_key.verifying_key().to_bytes(),
    );
    let replay = CompletePairingInput {
        device_pubkey: device_key.verifying_key().to_bytes(),
        device_name: "Phone".to_string(),
        nonce: nonce.to_vec(),
        signature,
        pairing_token: challenge.pairing_token.to_vec(),
    };
    let err = coordinator
        .complete_pairing_at(replay, now)
        .await
        .expect_err("replay rejected");
    assert!(err.to_string().contains("pairing.consumed"), "got {err}");
}

/// Expiry: advancing the clock past the 60 s TTL rejects with
/// `pairing.expired`.
#[tokio::test]
async fn token_expires_after_ttl() {
    let (_tmp, persistence, _core_pub, issuer) = fixtures().await;
    let coordinator = make_coordinator(persistence, issuer);

    let now = SystemTime::now();
    let challenge = coordinator.start_pairing_at(now).expect("start");
    let device_key = KeyPair::from_seed(&[0x66u8; 32]);
    let nonce = [0x77u8; PAIRING_NONCE_LEN];
    let input = loopback_handshake(
        challenge.pairing_token,
        &device_key,
        "Phone",
        nonce,
        &device_key,
    )
    .await;

    // Complete *after* the token has expired (injected clock — no real sleep).
    let later = now + PAIRING_TOKEN_TTL + Duration::from_secs(1);
    let err = coordinator
        .complete_pairing_at(input, later)
        .await
        .expect_err("expired token rejected");
    assert!(err.to_string().contains("pairing.expired"), "got {err}");
}

/// ≤ 3 active: a 4th `StartPairing` evicts the oldest token, so the first
/// token can no longer be completed (`pairing.consumed` — it was evicted).
#[tokio::test]
async fn at_most_three_active_tokens() {
    let (_tmp, persistence, _core_pub, issuer) = fixtures().await;
    let coordinator = make_coordinator(persistence, issuer);

    let base = SystemTime::now();
    // Mint 3 with strictly-increasing issued_at so eviction order is
    // deterministic, then a 4th evicts the oldest (challenge 0).
    let c0 = coordinator.start_pairing_at(base).expect("c0");
    let _c1 = coordinator
        .start_pairing_at(base + Duration::from_secs(1))
        .expect("c1");
    let _c2 = coordinator
        .start_pairing_at(base + Duration::from_secs(2))
        .expect("c2");
    let _c3 = coordinator
        .start_pairing_at(base + Duration::from_secs(3))
        .expect("c3 evicts c0");

    // Completing the evicted token 0 fails — it is gone from the store.
    let device_key = KeyPair::from_seed(&[0x88u8; 32]);
    let nonce = [0x99u8; PAIRING_NONCE_LEN];
    let input =
        loopback_handshake(c0.pairing_token, &device_key, "Phone", nonce, &device_key).await;
    let err = coordinator
        .complete_pairing_at(input, base + Duration::from_secs(3))
        .await
        .expect_err("evicted token rejected");
    assert!(err.to_string().contains("pairing.consumed"), "got {err}");
}

/// Bad signature: a signature made with the WRONG key over the token payload is
/// rejected, no `devices` row is inserted, and the token is NOT consumed (the
/// device can retry).
#[tokio::test]
async fn bad_signature_rejected_no_row() {
    let (_tmp, persistence, _core_pub, issuer) = fixtures().await;
    let coordinator = make_coordinator(Arc::clone(&persistence), issuer);

    let now = SystemTime::now();
    let challenge = coordinator.start_pairing_at(now).expect("start");
    let device_key = KeyPair::from_seed(&[0xAAu8; 32]);
    let wrong_key = KeyPair::from_seed(&[0xBBu8; 32]);
    let nonce = [0xCCu8; PAIRING_NONCE_LEN];

    // The device_pubkey is the real device key, but the signature is made with
    // a DIFFERENT key → verification against device_pubkey fails.
    let input = loopback_handshake(
        challenge.pairing_token,
        &device_key,
        "Phone",
        nonce,
        &wrong_key,
    )
    .await;
    let device_id = concerto_identity::device_id(&device_key.verifying_key().to_bytes());

    let err = coordinator
        .complete_pairing_at(input, now)
        .await
        .expect_err("bad signature rejected");
    assert!(
        err.to_string().contains("pairing.bad_signature"),
        "got {err}"
    );

    // No devices row.
    let id_hex = hex::encode(device_id);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE id = ?")
        .bind(&id_hex)
        .fetch_one(persistence.readers())
        .await
        .expect("count");
    assert_eq!(count, 0, "no devices row on bad signature");

    // The token was NOT consumed — a correctly-signed retry still succeeds.
    let good_sig = sign_pairing(
        &device_key,
        &challenge.pairing_token,
        &nonce,
        &device_key.verifying_key().to_bytes(),
    );
    let retry = CompletePairingInput {
        device_pubkey: device_key.verifying_key().to_bytes(),
        device_name: "Phone".to_string(),
        nonce: nonce.to_vec(),
        signature: good_sig,
        pairing_token: challenge.pairing_token.to_vec(),
    };
    coordinator
        .complete_pairing_at(retry, now)
        .await
        .expect("retry after bad-sig succeeds (token was not burned)");
}

/// Replay of a *recorded handshake*: capturing the device's exact
/// CompletePairing material and replaying it after the token is consumed is
/// rejected (`design/12 §10` security test). The Noise XX channel is fresh per
/// handshake (new ephemeral keys), so a recorded handshake cannot be replayed
/// at the transport layer either; here we assert the token-level one-shot
/// defence, which is the authority the coordinator owns.
#[tokio::test]
async fn recorded_handshake_replay_rejected() {
    let (_tmp, persistence, _core_pub, issuer) = fixtures().await;
    let coordinator = make_coordinator(persistence, issuer);

    let now = SystemTime::now();
    let challenge = coordinator.start_pairing_at(now).expect("start");
    let device_key = KeyPair::from_seed(&[0xDDu8; 32]);
    let nonce = [0xEEu8; PAIRING_NONCE_LEN];

    // Record the device's signed material once.
    let signature = sign_pairing(
        &device_key,
        &challenge.pairing_token,
        &nonce,
        &device_key.verifying_key().to_bytes(),
    );
    let recorded = || CompletePairingInput {
        device_pubkey: device_key.verifying_key().to_bytes(),
        device_name: "Phone".to_string(),
        nonce: nonce.to_vec(),
        signature,
        pairing_token: challenge.pairing_token.to_vec(),
    };

    // First use succeeds.
    coordinator
        .complete_pairing_at(recorded(), now)
        .await
        .expect("first use");
    // Replaying the exact recorded material → rejected (token consumed).
    let err = coordinator
        .complete_pairing_at(recorded(), now)
        .await
        .expect_err("recorded replay rejected");
    assert!(err.to_string().contains("pairing.consumed"), "got {err}");
}
