//! Noise IK session-layer tests (Task 208) — Tier-2 double.
//!
//! This suite proves, in-process over a loopback byte channel, the inner Noise
//! IK session crypto of `design/12 §3.4`/§6.3:
//!
//! - **Committed known-answer vectors** that freeze the IK handshake against
//!   `snow`-version drift: fixed statics + fixed ephemerals → fixed handshake
//!   messages + a fixed 64-byte BLAKE2b transport hash. If a `snow` upgrade
//!   ever perturbed the IK protocol these byte arrays change and the test fails
//!   loudly (the cross-version protocol freeze, mirroring the cert vectors).
//! - **Loopback IK handshake** (initiator = device, responder = Core) +
//!   AES-256-GCM frame round-trip in both directions, including the
//!   `establish_*` helpers driven over real channels.
//! - **Rekey accounting** — a deterministic time-threshold trip (the 1 h timer)
//!   leaves both ends in lockstep and the session still usable.
//! - **Replay rejection** — a recorded transport frame replayed against the
//!   session is rejected by the AEAD nonce/counter (`design/12 §10`).
//!
//! ## What this Tier-2 double does NOT cover
//!
//! A real cross-device Noise IK session running inside a live Iroh QUIC stream
//! across a real network (NAT, relay, real RTT). That is exercised by Task 212
//! / Task 220 and is the **Phase-2 Tier-3 manual checklist** (split-host file
//! transfer + real-NAT). The real-WAN-relayed throughput of the second AEAD
//! pass remains the spike's PENDING operator field line.

use std::time::{Duration, Instant};

use concerto_identity::{
    establish_initiator, establish_responder, IdentityError, NoiseIkHandshake, NoiseSession,
    NoiseStatic, NOISE_IK_PARAMS, REKEY_BYTES, REKEY_INTERVAL, TRANSPORT_HASH_LEN,
};

// ---------------------------------------------------------------------------
// Fixed inputs for the known-answer vector. Privates are arbitrary fixed
// bytes; publics are derived deterministically by `NoiseStatic::from_private`.
// ---------------------------------------------------------------------------
const DEVICE_STATIC_PRIV: [u8; 32] = [0x11; 32];
const CORE_STATIC_PRIV: [u8; 32] = [0x22; 32];
const DEVICE_EPHEMERAL: [u8; 32] = [0x33; 32];
const CORE_EPHEMERAL: [u8; 32] = [0x44; 32];

fn device_static() -> NoiseStatic {
    NoiseStatic::from_private(DEVICE_STATIC_PRIV).expect("device static")
}
fn core_static() -> NoiseStatic {
    NoiseStatic::from_private(CORE_STATIC_PRIV).expect("core static")
}

// ---------------------------------------------------------------------------
// FROZEN KNOWN-ANSWER VECTOR (cross-snow-version protocol freeze).
// Filled in from `print_known_answer_vector` (see the bottom of this file).
// ---------------------------------------------------------------------------
// Message 1 (initiator -> responder): `e, es, s, ss` (empty payload).
const EXPECTED_MSG1: &[u8] = &[
    0x7b, 0x0d, 0x47, 0xd9, 0x34, 0x27, 0xf8, 0x31, 0x11, 0x60, 0x78, 0x1c, 0x7c, 0x73, 0x3f, 0xd8,
    0x9f, 0x88, 0x97, 0x0a, 0xef, 0x49, 0x0d, 0x8a, 0xa0, 0xee, 0x19, 0xa4, 0xcb, 0x8a, 0x1b, 0x14,
    0x1f, 0x93, 0x0b, 0x7e, 0x45, 0xb8, 0x90, 0x8c, 0x0d, 0x74, 0x37, 0x4e, 0x9b, 0x44, 0x40, 0x50,
    0x90, 0xf9, 0xb2, 0x75, 0x20, 0x61, 0x0f, 0xfd, 0x24, 0x87, 0x1a, 0x57, 0x81, 0x0b, 0x6b, 0xaa,
    0xb2, 0xf3, 0x50, 0x62, 0x07, 0x73, 0x00, 0x0a, 0x81, 0x87, 0x96, 0xb5, 0x45, 0xa7, 0x44, 0xd9,
    0x07, 0x7a, 0xab, 0x7b, 0xc5, 0x4a, 0xdf, 0xd4, 0x40, 0x34, 0xde, 0xd4, 0xea, 0xf9, 0xb1, 0xa6,
];
// Message 2 (responder -> initiator): `e, ee, se` (empty payload).
const EXPECTED_MSG2: &[u8] = &[
    0xff, 0x2e, 0xe4, 0x56, 0x01, 0xec, 0x1b, 0x67, 0x31, 0x0c, 0x77, 0x90, 0x40, 0x45, 0x85, 0xae,
    0x69, 0x73, 0x31, 0xee, 0xe1, 0xc1, 0xf8, 0xcf, 0x24, 0x19, 0x73, 0x1c, 0x1f, 0xff, 0x3e, 0x6b,
    0xca, 0xf4, 0x4c, 0x81, 0xee, 0x6b, 0x5e, 0x8d, 0x2a, 0x26, 0xfd, 0x1c, 0x37, 0x04, 0xa6, 0x6a,
];
// The derived 64-byte BLAKE2b transport hash — identical on both ends.
const EXPECTED_TRANSPORT_HASH: [u8; TRANSPORT_HASH_LEN] = [
    0xd2, 0x0a, 0x1e, 0x4e, 0xb2, 0x47, 0x98, 0x44, 0x53, 0xb0, 0x03, 0x30, 0xb0, 0xd6, 0xf0, 0x45,
    0x4a, 0x4f, 0xbc, 0xce, 0x52, 0xaa, 0x47, 0x4b, 0x90, 0x99, 0x54, 0xfd, 0x10, 0x40, 0xd0, 0x30,
    0xa4, 0xb9, 0x2c, 0xc2, 0x6c, 0xfe, 0xb9, 0x29, 0xc5, 0x14, 0x6b, 0xda, 0xf6, 0xd2, 0x36, 0xae,
    0x5c, 0xb5, 0xc6, 0xa5, 0x73, 0x3d, 0xaa, 0x27, 0xa3, 0x58, 0xa9, 0xb5, 0xbb, 0x44, 0x29, 0x95,
];

fn ka_initiator() -> NoiseIkHandshake {
    NoiseIkHandshake::initiator_with_fixed_ephemeral(
        &device_static(),
        &core_static().public(),
        &DEVICE_EPHEMERAL,
    )
    .expect("ka initiator")
}
fn ka_responder() -> NoiseIkHandshake {
    NoiseIkHandshake::responder_with_fixed_ephemeral(&core_static(), &CORE_EPHEMERAL)
        .expect("ka responder")
}

#[test]
fn protocol_string_and_thresholds_are_frozen() {
    assert_eq!(NOISE_IK_PARAMS, "Noise_IK_25519_AESGCM_BLAKE2b");
    assert_eq!(REKEY_BYTES, 1_000_000_000);
    assert_eq!(REKEY_INTERVAL, Duration::from_secs(3600));
}

#[test]
fn known_answer_handshake_is_frozen() {
    let mut ini = ka_initiator();
    let mut res = ka_responder();

    let m1 = ini.write_message(&[]).expect("m1");
    assert_eq!(
        m1, EXPECTED_MSG1,
        "IK message 1 changed — FROZEN cross-snow-version protocol vector"
    );
    res.read_message(&m1).expect("responder reads m1");

    let m2 = res.write_message(&[]).expect("m2");
    assert_eq!(
        m2, EXPECTED_MSG2,
        "IK message 2 changed — FROZEN cross-snow-version protocol vector"
    );
    ini.read_message(&m2).expect("initiator reads m2");

    assert!(ini.is_handshake_finished());
    assert!(res.is_handshake_finished());

    let now = Instant::now();
    let ini_s = ini.into_session(now).expect("ini session");
    let res_s = res.into_session(now).expect("res session");
    assert_eq!(ini_s.transport_hash(), res_s.transport_hash());
    assert_eq!(
        ini_s.transport_hash(),
        EXPECTED_TRANSPORT_HASH,
        "transport hash changed — FROZEN vector"
    );
}

#[test]
fn loopback_handshake_and_aead_roundtrip() {
    let now = Instant::now();
    let (mut ini, mut res) = loopback(now);

    assert_eq!(ini.transport_hash(), res.transport_hash());

    // AES-256-GCM round-trip both ways.
    let req = b"unary gRPC request frame";
    let ct = ini.encrypt_at(req, now).unwrap();
    assert_ne!(&ct[..], &req[..]);
    assert_eq!(res.decrypt_at(&ct, now).unwrap(), req);

    let resp = b"unary gRPC response frame";
    let ct2 = res.encrypt_at(resp, now).unwrap();
    assert_eq!(ini.decrypt_at(&ct2, now).unwrap(), resp);
}

#[test]
fn establish_helpers_drive_full_handshake_over_channels() {
    // Drive both sides with the `establish_*` helpers via std mpsc channels,
    // running the responder on a worker thread and the initiator on main.
    use std::sync::mpsc;
    let now = Instant::now();
    let dev = device_static();
    let core = core_static();
    let core_pub = core.public();

    let (i2r_tx, i2r_rx) = mpsc::channel::<Vec<u8>>();
    let (r2i_tx, r2i_rx) = mpsc::channel::<Vec<u8>>();

    let core_handle = std::thread::spawn(move || {
        establish_responder(
            &core,
            now,
            |m| r2i_tx.send(m.to_vec()).map_err(noise_err),
            || r2i_recv(&i2r_rx),
        )
    });

    let mut ini = establish_initiator(
        &dev,
        &core_pub,
        now,
        |m| i2r_tx.send(m.to_vec()).map_err(noise_err),
        || r2i_recv(&r2i_rx),
    )
    .expect("initiator establishes");

    let mut res = core_handle.join().unwrap().expect("responder establishes");
    let ct = ini.encrypt_at(b"hello over IK", now).unwrap();
    assert_eq!(res.decrypt_at(&ct, now).unwrap(), b"hello over IK");
}

#[test]
fn rekey_on_time_threshold_keeps_session_in_sync() {
    let now = Instant::now();
    let (mut ini, mut res) = loopback(now);

    let ct = ini.encrypt_at(b"before", now).unwrap();
    assert_eq!(res.decrypt_at(&ct, now).unwrap(), b"before");

    // Trip the 1 h timer on both ends symmetrically for the next op.
    let later = now + REKEY_INTERVAL + Duration::from_secs(1);
    let ct2 = ini.encrypt_at(b"after rekey", later).unwrap();
    assert_eq!(ini.bytes_since_rekey(), 0);
    assert_eq!(res.decrypt_at(&ct2, later).unwrap(), b"after rekey");
    assert_eq!(res.bytes_since_rekey(), 0);

    // Still usable after the rekey (same advanced clock — no further trip).
    let ct3 = res.encrypt_at(b"post", later).unwrap();
    assert_eq!(ini.decrypt_at(&ct3, later).unwrap(), b"post");
}

#[test]
fn replay_of_a_recorded_frame_is_rejected() {
    let now = Instant::now();
    let (mut ini, mut res) = loopback(now);

    // Record a legitimate frame and deliver it once (accepted).
    let frame = ini.encrypt_at(b"transfer $100", now).unwrap();
    assert_eq!(res.decrypt_at(&frame, now).unwrap(), b"transfer $100");

    // Replaying the SAME ciphertext fails: the AES-GCM nonce/counter has
    // advanced, so the recorded frame no longer authenticates (design/12 §10).
    assert!(
        res.decrypt_at(&frame, now).is_err(),
        "a replayed transport frame must be rejected by the session"
    );
}

#[test]
fn tampered_frame_is_rejected() {
    let now = Instant::now();
    let (mut ini, mut res) = loopback(now);
    let mut frame = ini.encrypt_at(b"authentic", now).unwrap();
    frame[0] ^= 0x01;
    assert!(
        res.decrypt_at(&frame, now).is_err(),
        "a tampered frame must fail AEAD authentication"
    );
}

#[test]
fn wrong_responder_static_fails_handshake() {
    let dev = device_static();
    let core = core_static();
    let wrong = NoiseStatic::from_private([0x99; 32]).unwrap();

    let mut ini = NoiseIkHandshake::initiator(&dev, &wrong.public()).unwrap();
    let mut res = NoiseIkHandshake::responder(&core).unwrap();
    let m1 = ini.write_message(&[]).unwrap();
    assert!(
        res.read_message(&m1).is_err(),
        "a wrong pre-loaded responder static must fail the handshake"
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a fully-established initiator/responder session pair over an
/// in-process lock-step exchange (the fixed vector statics — deterministic
/// statics, random ephemerals).
fn loopback(now: Instant) -> (NoiseSession, NoiseSession) {
    let dev = device_static();
    let core = core_static();
    let core_pub = core.public();

    let mut ini = NoiseIkHandshake::initiator(&dev, &core_pub).unwrap();
    let mut res = NoiseIkHandshake::responder(&core).unwrap();

    let m1 = ini.write_message(&[]).unwrap();
    res.read_message(&m1).unwrap();
    let m2 = res.write_message(&[]).unwrap();
    ini.read_message(&m2).unwrap();

    (
        ini.into_session(now).unwrap(),
        res.into_session(now).unwrap(),
    )
}

fn noise_err<E: std::fmt::Display>(e: E) -> IdentityError {
    IdentityError::Noise(e.to_string())
}

fn r2i_recv(rx: &std::sync::mpsc::Receiver<Vec<u8>>) -> Result<Vec<u8>, IdentityError> {
    rx.recv().map_err(noise_err)
}

/// Print helper to (re)derive the frozen vector after an intentional, reviewed
/// `snow` change. Run with:
/// `cargo test -p concerto-identity --test noise_ik_vectors -- --ignored \
///   --nocapture print_known_answer_vector`
/// then paste the output into the constants above.
#[test]
#[ignore]
fn print_known_answer_vector() {
    let mut ini = ka_initiator();
    let mut res = ka_responder();

    let m1 = ini.write_message(&[]).unwrap();
    print_bytes("EXPECTED_MSG1", &m1);
    res.read_message(&m1).unwrap();
    let m2 = res.write_message(&[]).unwrap();
    print_bytes("EXPECTED_MSG2", &m2);
    ini.read_message(&m2).unwrap();

    let s = ini.into_session(Instant::now()).unwrap();
    print_bytes("EXPECTED_TRANSPORT_HASH", &s.transport_hash());
}

fn print_bytes(name: &str, bytes: &[u8]) {
    println!("const {name} ({} bytes):", bytes.len());
    for chunk in bytes.chunks(16) {
        let line: Vec<String> = chunk.iter().map(|b| format!("0x{b:02x}")).collect();
        println!("    {},", line.join(", "));
    }
}
