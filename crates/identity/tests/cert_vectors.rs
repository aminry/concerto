//! Known-answer + tamper tests for the `DeviceCert` wire contract.
//!
//! The known-answer vector freezes the canonical-CBOR encoding and the
//! Ed25519 signature across versions: a fixed 32-byte Core seed + a fixed
//! cert must always produce the exact `cert_bytes` and `signature` below. If
//! a `ciborium`/`serde`/`ed25519-dalek` upgrade ever perturbs the encoding,
//! this test fails — which is the whole point (the encoding is a wire
//! contract, not an implementation detail).
//!
//! The expected byte arrays are filled in once from a `--nocapture` run of
//! `print_known_answer_vector` (see the helper at the bottom) and then
//! frozen.

use concerto_identity::{
    device_id, encode_cert, sign_cert, verify_cert, DeviceCert, KeyPair, PublicKey,
};

/// Fixed Core identity seed for the vector. All-`0x42`.
const CORE_SEED: [u8; 32] = [0x42; 32];
/// Fixed device public key for the vector. All-`0x11`.
const DEVICE_PUBKEY: [u8; 32] = [0x11; 32];

/// Build the exact cert the vector pins.
fn vector_cert() -> DeviceCert {
    DeviceCert {
        version: 1,
        device_id: device_id(&DEVICE_PUBKEY),
        device_pubkey: DEVICE_PUBKEY,
        device_name: "Vector Device".to_string(),
        core_pubkey: KeyPair::from_seed(&CORE_SEED).verifying_key().to_bytes(),
        issued_at: 1_700_000_000,
        expires_at: 1_700_000_000 + 365 * 24 * 60 * 60,
        capabilities: vec!["admin".to_string()],
        revocation_check_required: true,
    }
}

// ---------------------------------------------------------------------------
// FROZEN KNOWN-ANSWER VECTOR
// ---------------------------------------------------------------------------
// Canonical CBOR encoding of `vector_cert()`.
const EXPECTED_CERT_BYTES: &[u8] = &[
    0xa9, 0x67, 0x76, 0x65, 0x72, 0x73, 0x69, 0x6f, 0x6e, 0x01, 0x69, 0x64, 0x65, 0x76, 0x69, 0x63,
    0x65, 0x5f, 0x69, 0x64, 0x98, 0x20, 0x18, 0xd4, 0x18, 0xff, 0x18, 0xae, 0x18, 0xea, 0x18, 0xc4,
    0x18, 0x5a, 0x18, 0xa4, 0x18, 0x18, 0x18, 0x25, 0x18, 0xe0, 0x18, 0xbc, 0x18, 0x3f, 0x18, 0x87,
    0x18, 0x55, 0x18, 0x70, 0x18, 0xaf, 0x06, 0x18, 0x1a, 0x18, 0xcb, 0x18, 0xf0, 0x18, 0xb9, 0x18,
    0x50, 0x18, 0xad, 0x18, 0x75, 0x18, 0x2f, 0x18, 0xf0, 0x18, 0xf9, 0x18, 0x46, 0x18, 0x3f, 0x18,
    0xe1, 0x18, 0x3a, 0x18, 0xd5, 0x6d, 0x64, 0x65, 0x76, 0x69, 0x63, 0x65, 0x5f, 0x70, 0x75, 0x62,
    0x6b, 0x65, 0x79, 0x98, 0x20, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
    0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x11,
    0x11, 0x11, 0x11, 0x11, 0x11, 0x6b, 0x64, 0x65, 0x76, 0x69, 0x63, 0x65, 0x5f, 0x6e, 0x61, 0x6d,
    0x65, 0x6d, 0x56, 0x65, 0x63, 0x74, 0x6f, 0x72, 0x20, 0x44, 0x65, 0x76, 0x69, 0x63, 0x65, 0x6b,
    0x63, 0x6f, 0x72, 0x65, 0x5f, 0x70, 0x75, 0x62, 0x6b, 0x65, 0x79, 0x98, 0x20, 0x18, 0x21, 0x18,
    0x52, 0x18, 0xf8, 0x18, 0xd1, 0x18, 0x9b, 0x18, 0x79, 0x18, 0x1d, 0x18, 0x24, 0x18, 0x45, 0x18,
    0x32, 0x18, 0x42, 0x18, 0xe1, 0x18, 0x5f, 0x18, 0x2e, 0x18, 0xab, 0x18, 0x6c, 0x18, 0xb7, 0x18,
    0xcf, 0x18, 0xfa, 0x18, 0x7b, 0x18, 0x6a, 0x18, 0x5e, 0x18, 0xd3, 0x00, 0x18, 0x97, 0x18, 0x96,
    0x0e, 0x06, 0x18, 0x98, 0x18, 0x81, 0x18, 0xdb, 0x12, 0x69, 0x69, 0x73, 0x73, 0x75, 0x65, 0x64,
    0x5f, 0x61, 0x74, 0x1a, 0x65, 0x53, 0xf1, 0x00, 0x6a, 0x65, 0x78, 0x70, 0x69, 0x72, 0x65, 0x73,
    0x5f, 0x61, 0x74, 0x1a, 0x67, 0x35, 0x24, 0x80, 0x6c, 0x63, 0x61, 0x70, 0x61, 0x62, 0x69, 0x6c,
    0x69, 0x74, 0x69, 0x65, 0x73, 0x81, 0x65, 0x61, 0x64, 0x6d, 0x69, 0x6e, 0x78, 0x19, 0x72, 0x65,
    0x76, 0x6f, 0x63, 0x61, 0x74, 0x69, 0x6f, 0x6e, 0x5f, 0x63, 0x68, 0x65, 0x63, 0x6b, 0x5f, 0x72,
    0x65, 0x71, 0x75, 0x69, 0x72, 0x65, 0x64, 0xf5,
];
// Ed25519 signature of EXPECTED_CERT_BYTES under CORE_SEED.
const EXPECTED_SIGNATURE: [u8; 64] = [
    0x53, 0xbb, 0xd6, 0x2f, 0x92, 0xd8, 0x18, 0x45, 0x5f, 0x2c, 0xdd, 0x24, 0x43, 0xba, 0x6c, 0xb9,
    0x0a, 0x68, 0xd1, 0x55, 0x72, 0xc2, 0xe9, 0xca, 0x89, 0x23, 0xf9, 0xa2, 0xef, 0x25, 0x6f, 0x62,
    0xc5, 0x6c, 0x92, 0x00, 0x43, 0x15, 0xa9, 0xac, 0x16, 0xae, 0x09, 0x5e, 0xfe, 0x5e, 0xc1, 0x15,
    0x19, 0x47, 0x1f, 0xe4, 0xa5, 0xaa, 0x20, 0x33, 0xd3, 0x60, 0xc3, 0x78, 0xa4, 0xbe, 0xdd, 0x08,
];

#[test]
fn known_answer_cert_bytes_are_frozen() {
    let cert = vector_cert();
    let bytes = encode_cert(&cert).expect("encode");
    assert_eq!(
        bytes, EXPECTED_CERT_BYTES,
        "canonical CBOR encoding changed — this is a FROZEN wire contract"
    );
}

#[test]
fn known_answer_signature_is_frozen() {
    let core = KeyPair::from_seed(&CORE_SEED);
    let signed = sign_cert(&core, &vector_cert()).expect("sign");
    assert_eq!(
        signed.signature, EXPECTED_SIGNATURE,
        "signature over the frozen bytes changed — wire contract broken"
    );
}

#[test]
fn sign_then_verify_happy_path() {
    let core = KeyPair::from_seed(&CORE_SEED);
    let core_pub = core.verifying_key();
    let signed = sign_cert(&core, &vector_cert()).expect("sign");

    let mut raw = signed.cert_bytes.clone();
    raw.extend_from_slice(&signed.signature);

    let cert = verify_cert(&raw, &core_pub).expect("verify");
    assert_eq!(cert, vector_cert());
}

#[test]
fn single_bit_tamper_in_body_fails() {
    let core = KeyPair::from_seed(&CORE_SEED);
    let core_pub = core.verifying_key();
    let signed = sign_cert(&core, &vector_cert()).expect("sign");

    let mut raw = signed.cert_bytes.clone();
    raw.extend_from_slice(&signed.signature);

    // Flip one bit in the CBOR body.
    raw[10] ^= 0x01;
    assert!(
        verify_cert(&raw, &core_pub).is_err(),
        "tampered body must fail verification"
    );
}

#[test]
fn single_bit_tamper_in_signature_fails() {
    let core = KeyPair::from_seed(&CORE_SEED);
    let core_pub = core.verifying_key();
    let signed = sign_cert(&core, &vector_cert()).expect("sign");

    let mut raw = signed.cert_bytes.clone();
    raw.extend_from_slice(&signed.signature);

    // Flip one bit in the trailing signature.
    let last = raw.len() - 1;
    raw[last] ^= 0x80;
    assert!(
        verify_cert(&raw, &core_pub).is_err(),
        "tampered signature must fail verification"
    );
}

#[test]
fn wrong_core_pubkey_fails() {
    let core = KeyPair::from_seed(&CORE_SEED);
    let signed = sign_cert(&core, &vector_cert()).expect("sign");

    let mut raw = signed.cert_bytes.clone();
    raw.extend_from_slice(&signed.signature);

    // A different Core key must reject this cert.
    let wrong = KeyPair::from_seed(&[0x99; 32]).verifying_key();
    assert!(
        verify_cert(&raw, &wrong).is_err(),
        "cert signed by a different Core must fail"
    );
}

#[test]
fn truncated_input_errors_not_panics() {
    let core_pub = KeyPair::from_seed(&CORE_SEED).verifying_key();
    // Shorter than the 64-byte signature tail.
    for len in [0usize, 1, 32, 63] {
        let raw = vec![0u8; len];
        assert!(verify_cert(&raw, &core_pub).is_err());
    }
}

#[test]
fn garbage_input_errors_not_panics() {
    let core_pub = KeyPair::from_seed(&CORE_SEED).verifying_key();
    // 64+ bytes of garbage: signature check fails before any decode.
    let raw = vec![0xABu8; 200];
    assert!(verify_cert(&raw, &core_pub).is_err());
}

#[test]
fn non_canonical_trailing_garbage_fails_even_with_valid_sig() {
    // Sign over body+garbage so the signature is valid, but the body is no
    // longer canonical CBOR for the decoded cert -> re-encode mismatch.
    let core = KeyPair::from_seed(&CORE_SEED);
    let core_pub = core.verifying_key();

    let mut body = encode_cert(&vector_cert()).expect("encode");
    body.push(0x00); // trailing byte: decodes (ciborium stops at the map) but re-encode differs
    let sig = core.sign(&body);

    let mut raw = body.clone();
    raw.extend_from_slice(&sig.to_bytes());

    assert!(
        verify_cert(&raw, &core_pub).is_err(),
        "non-canonical body must be rejected even with a valid signature"
    );
}

#[test]
fn public_key_from_cert_core_pubkey_is_usable() {
    // The core_pubkey field round-trips back into a usable PublicKey.
    let cert = vector_cert();
    let pk = PublicKey::from_bytes(&cert.core_pubkey).expect("valid point");
    assert_eq!(pk.to_bytes(), cert.core_pubkey);
}

/// Print helper to (re)derive the frozen vector. Ignored in normal runs;
/// run with `cargo test -p concerto-identity --test cert_vectors -- --ignored
/// --nocapture print_known_answer_vector` after an intentional, reviewed
/// encoding change, then paste the output into the constants above.
#[test]
#[ignore]
fn print_known_answer_vector() {
    let core = KeyPair::from_seed(&CORE_SEED);
    let signed = sign_cert(&core, &vector_cert()).expect("sign");
    println!("cert_bytes ({} bytes):", signed.cert_bytes.len());
    for chunk in signed.cert_bytes.chunks(16) {
        let line: Vec<String> = chunk.iter().map(|b| format!("0x{b:02x}")).collect();
        println!("    {},", line.join(", "));
    }
    println!("signature:");
    for chunk in signed.signature.chunks(16) {
        let line: Vec<String> = chunk.iter().map(|b| format!("0x{b:02x}")).collect();
        println!("    {},", line.join(", "));
    }
}
