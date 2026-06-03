//! Integration tests for the `LocalCoreIssuer` device-cert issuer (Task 206).
//!
//! These drive the **public** [`DeviceCertIssuer`] trait surface — the async
//! `issue` and the sync `validate` — exactly as Task 207 (pairing) and Task
//! 210 (auth middleware) will. The fine-grained skew/timing/boundary cases
//! live in the crate's `#[cfg(test)]` unit tests in `src/issuer.rs`; here we
//! prove the end-to-end seam over the frozen public API.
//!
//! The certs issued here use the real wall clock (`issue` derives `issued_at`
//! from `SystemTime::now`), so a freshly-issued 365-day cert is comfortably
//! valid — no time injection needed for the happy path.

use concerto_identity::{
    device_id, DeviceCertIssuer, IdentityError, KeyPair, LocalCoreIssuer, PairingRequest,
    SignedDeviceCert,
};

/// On-wire framing: `cert_bytes || signature` (Task 205 handoff — the form
/// `verify_cert` and the auth middleware expect).
fn on_wire(signed: &SignedDeviceCert) -> Vec<u8> {
    let mut raw = signed.cert_bytes.clone();
    raw.extend_from_slice(&signed.signature);
    raw
}

fn make_issuer(seed: u8) -> LocalCoreIssuer {
    let core_pub = KeyPair::from_seed(&[seed; 32]).verifying_key();
    LocalCoreIssuer::new(
        KeyPair::from_seed(&[seed; 32]),
        core_pub,
        concerto_identity::new_revoked_set(),
    )
}

fn make_request(seed: u8, name: &str) -> PairingRequest {
    PairingRequest {
        device_pubkey: KeyPair::from_seed(&[seed; 32]).verifying_key().to_bytes(),
        device_name: name.to_string(),
    }
}

#[tokio::test]
async fn issue_then_validate_happy_path() {
    let issuer = make_issuer(1);
    let req = make_request(2, "Amin's Phone");
    let signed = issuer.issue(req.clone()).await.expect("issue");

    // Policy: 365-day expiry, admin-only, revocation checked.
    assert_eq!(signed.cert.capabilities, vec!["admin".to_string()]);
    assert!(signed.cert.revocation_check_required);
    assert_eq!(signed.cert.device_id, device_id(&req.device_pubkey));
    assert_eq!(
        signed.cert.expires_at - signed.cert.issued_at,
        365 * 24 * 60 * 60
    );

    let ctx = issuer.validate(&on_wire(&signed)).expect("validate");
    assert_eq!(ctx.device_id, signed.cert.device_id);
    assert_eq!(ctx.device_name, "Amin's Phone");
    assert_eq!(ctx.capabilities, vec!["admin".to_string()]);
}

#[tokio::test]
async fn cert_from_a_different_core_is_rejected() {
    let issuer_a = make_issuer(1);
    let signed = issuer_a
        .issue(make_request(2, "Phone"))
        .await
        .expect("issue");

    let issuer_b = make_issuer(9);
    // Signed by A's key; B's verifier rejects the signature.
    assert!(matches!(
        issuer_b.validate(&on_wire(&signed)),
        Err(IdentityError::BadSignature)
    ));
}

#[tokio::test]
async fn revoked_device_is_rejected() {
    let revoked = concerto_identity::new_revoked_set();
    let core_pub = KeyPair::from_seed(&[1u8; 32]).verifying_key();
    let issuer = LocalCoreIssuer::new(KeyPair::from_seed(&[1u8; 32]), core_pub, revoked.clone());

    let req = make_request(2, "Phone");
    let signed = issuer.issue(req.clone()).await.expect("issue");
    assert!(issuer.validate(&on_wire(&signed)).is_ok());

    // Task 209's revoke path inserts into the shared handle.
    revoked
        .write()
        .unwrap()
        .insert(device_id(&req.device_pubkey));

    assert!(matches!(
        issuer.validate(&on_wire(&signed)),
        Err(IdentityError::Revoked)
    ));
}

#[tokio::test]
async fn garbage_bytes_return_err_never_panic() {
    let issuer = make_issuer(1);
    for raw in [
        &b""[..],
        &b"nope"[..],
        &[0x00u8; 63][..],
        &[0xFFu8; 64][..],
        &[0x42u8; 512][..],
    ] {
        assert!(issuer.validate(raw).is_err());
    }
}

#[tokio::test]
async fn supported_capabilities_reports_admin() {
    let issuer = make_issuer(1);
    assert_eq!(issuer.supported_capabilities(), &["admin"]);
}
