//! `LocalCoreIssuer` — the V1.0 MIT [`DeviceCertIssuer`] impl.
//!
//! This is the issuance + validation seam the rest of Phase 2 authenticates
//! against (`design/12 §3.10`). It composes the Task 205 primitives
//! ([`sign_cert`] / [`verify_cert`] / [`device_id`] / [`DeviceCert::is_expired`])
//! with the Core's Ed25519 identity, a clock, and the V1.0 issuance policy.
//!
//! # Issuance policy (FROZEN, V1.0)
//!
//! - `expires_at = issued_at + 365 days` (`design/12 §3.2`).
//! - `capabilities = ["admin"]` (V1.0 has a single capability).
//! - `revocation_check_required = true` (always consult the revoked set).
//!
//! # The < 200 µs validation hot path (`design/12 §6.1`)
//!
//! [`LocalCoreIssuer::validate`] is **sync** and allocation-light. It does, in
//! order, the four steps of `design/12 §3.2`:
//!
//! 1. `verify_cert(raw, core_pub)` — signature + structural validity (205).
//! 2. `core_pubkey` match — the cert must name *this* Core (`design/12 §8`).
//! 3. expiry with ±5 min skew tolerance (`design/12 §8`).
//! 4. revoked-set membership on `device_id` (`design/12 §3.11`) — an in-memory
//!    `RwLock<HashSet>` read, **no DB hit**.
//!
//! On success it builds the [`DeviceContext`] output (which *does* allocate —
//! that is the result, not hot-path overhead). No other heap work happens on
//! the happy path beyond what `verify_cert` needs.
//!
//! ## Reserved V2.0 BSL issuers
//!
//! The V2.0 BSL impls (`OrgManagedCaIssuer`, `MdmIssuer`, `OidcBridgeIssuer`)
//! are reserved — not implemented in the MIT monorepo — and documented on the
//! [`crate::api::DeviceCertIssuer`] trait so Task 707's trait-seam registry
//! completeness check can verify the names without a Core refork
//! (`design/12 §3.10` / `design/18 §3.7`).

use std::collections::HashSet;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;

use crate::api::{
    device_id, sign_cert, verify_cert, DeviceCert, DeviceCertIssuer, DeviceContext,
    LocalCoreIssuer, PairingRequest, SignedDeviceCert,
};
use crate::error::{IdentityError, Result};

/// Shared, cheaply-cloneable handle to the set of revoked `device_id`s.
///
/// **FROZEN handle type (Task 206).** A `std::sync::RwLock<HashSet<…>>` behind
/// an `Arc`: the validator takes a *read* lock per call (near-lock-free under
/// the read-mostly workload of the hot path), and Task 209's `RevokeDevice`
/// path takes a *write* lock to insert. Both the issuer and the revoke path
/// hold a clone of the **same** `Arc`, so a revoke is observed by the next
/// `validate` with no DB round-trip. We use `std::sync` (not `tokio`) because
/// `validate` is sync and must not `.await`.
pub type RevokedSet = Arc<RwLock<HashSet<[u8; 32]>>>;

/// Default device-cert lifetime: 365 days, in seconds (`design/12 §3.2`).
pub const CERT_LIFETIME_SECS: u64 = 365 * 24 * 60 * 60;

/// Clock-skew tolerance applied to the expiry check (`design/12 §8`): a cert
/// is accepted until `expires_at + SKEW_TOLERANCE_SECS`.
pub const SKEW_TOLERANCE_SECS: u64 = 5 * 60;

/// The single V1.0 capability every `LocalCoreIssuer` cert carries.
const V1_CAPABILITIES: &[&str] = &["admin"];

/// Construct an empty, shareable revoked set. Task 209 will populate it from
/// the `devices` table on revoke; until then the issuer starts with an empty
/// set (no device revoked) and reads it on every `validate`.
pub fn new_revoked_set() -> RevokedSet {
    Arc::new(RwLock::new(HashSet::new()))
}

/// Current unix epoch seconds. Saturates at 0 before the epoch (never
/// underflows; `SystemTime::now` is always ≥ epoch in practice).
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl LocalCoreIssuer {
    /// Build the [`DeviceCert`] for a pairing request under the V1.0 policy,
    /// signing it with the Core's identity. Split out from [`Self::issue`] so
    /// tests can drive issuance with a fixed `issued_at`.
    fn issue_at(&self, req: &PairingRequest, issued_at: u64) -> Result<SignedDeviceCert> {
        let cert = DeviceCert {
            version: 1,
            device_id: device_id(&req.device_pubkey),
            device_pubkey: req.device_pubkey,
            device_name: req.device_name.clone(),
            core_pubkey: self.core_pub.to_bytes(),
            issued_at,
            expires_at: issued_at.saturating_add(CERT_LIFETIME_SECS),
            capabilities: V1_CAPABILITIES.iter().map(|c| c.to_string()).collect(),
            revocation_check_required: true,
        };
        sign_cert(&self.core_key, &cert)
    }

    /// The sync core of [`Self::validate`], parameterized on `now` so a test
    /// can pin the clock and exercise the ±5 min skew boundary deterministically.
    fn validate_at(&self, raw: &[u8], now: u64) -> Result<DeviceContext> {
        // Step 1 (§3.2): signature + structural validity (Task 205). This also
        // rejects truncated/garbage/non-canonical bytes without panicking.
        let cert = verify_cert(raw, &self.core_pub)?;

        // Step 1b (§8): the cert must name *this* Core. `verify_cert` already
        // proved the signature is ours, so a mismatch here is defence in depth
        // (a cert signed by us but carrying a foreign `core_pubkey` is
        // malformed); it also gives the auth layer a precise reason string.
        if cert.core_pubkey != self.core_pub.to_bytes() {
            return Err(IdentityError::WrongCore);
        }

        // Step 2 (§3.2 + §8): expiry with ±5 min skew tolerance. The pure
        // helper is exact (`now >= expires_at`); we push the boundary out by
        // the skew so a slightly-fast client clock near expiry still validates.
        let skewed_now = now.saturating_sub(SKEW_TOLERANCE_SECS);
        if cert.is_expired(skewed_now) {
            return Err(IdentityError::Expired);
        }

        // Step 3 (§3.11): revoked-set membership on `device_id`. In-memory
        // read lock, no DB hit. V1.0 certs always set
        // `revocation_check_required = true`; we honour the flag so a future
        // cert that opts out skips the lookup.
        if cert.revocation_check_required {
            let revoked = self.revoked.read().map_err(|_| IdentityError::Revoked)?;
            if revoked.contains(&cert.device_id) {
                return Err(IdentityError::Revoked);
            }
        }

        // Step 4 (§3.2): extract the validated identity. This allocates — it is
        // the output, not hot-path overhead.
        Ok(DeviceContext {
            device_id: cert.device_id,
            device_name: cert.device_name,
            capabilities: cert.capabilities,
        })
    }
}

#[async_trait]
impl DeviceCertIssuer for LocalCoreIssuer {
    async fn issue(&self, req: PairingRequest) -> Result<SignedDeviceCert> {
        self.issue_at(&req, now_unix())
    }

    fn validate(&self, raw: &[u8]) -> Result<DeviceContext> {
        self.validate_at(raw, now_unix())
    }

    fn supported_capabilities(&self) -> &'static [&'static str] {
        V1_CAPABILITIES
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::KeyPair;

    fn issuer_with(revoked: RevokedSet) -> (LocalCoreIssuer, KeyPair) {
        let core = KeyPair::from_seed(&[42u8; 32]);
        let core_pub = core.verifying_key();
        // The issuer owns one keypair; tests that need the Core's *private*
        // half (none do here) would reconstruct from the seed.
        let issuer = LocalCoreIssuer::new(KeyPair::from_seed(&[42u8; 32]), core_pub, revoked);
        (issuer, core)
    }

    fn sample_request() -> PairingRequest {
        let device = KeyPair::from_seed(&[7u8; 32]);
        PairingRequest {
            device_pubkey: device.verifying_key().to_bytes(),
            device_name: "Unit Test Phone".to_string(),
        }
    }

    #[test]
    fn issue_then_validate_roundtrip() {
        let (issuer, _) = issuer_with(new_revoked_set());
        let req = sample_request();
        let signed = issuer.issue_at(&req, 1_700_000_000).expect("issue");

        // Sanity on the issued cert policy.
        assert_eq!(signed.cert.version, 1);
        assert_eq!(signed.cert.capabilities, vec!["admin".to_string()]);
        assert!(signed.cert.revocation_check_required);
        assert_eq!(signed.cert.expires_at, 1_700_000_000 + CERT_LIFETIME_SECS);
        assert_eq!(signed.cert.device_id, device_id(&req.device_pubkey));

        let raw = on_wire(&signed);
        let ctx = issuer
            .validate_at(&raw, 1_700_000_001)
            .expect("validate fresh cert");
        assert_eq!(ctx.device_id, signed.cert.device_id);
        assert_eq!(ctx.device_name, "Unit Test Phone");
        assert_eq!(ctx.capabilities, vec!["admin".to_string()]);
    }

    #[test]
    fn supported_capabilities_is_admin_only() {
        let (issuer, _) = issuer_with(new_revoked_set());
        assert_eq!(issuer.supported_capabilities(), &["admin"]);
    }

    #[test]
    fn expired_cert_rejected() {
        let (issuer, _) = issuer_with(new_revoked_set());
        let signed = issuer
            .issue_at(&sample_request(), 1_700_000_000)
            .expect("issue");
        let raw = on_wire(&signed);
        // Well past expiry + skew.
        let now = 1_700_000_000 + CERT_LIFETIME_SECS + SKEW_TOLERANCE_SECS + 10;
        assert!(matches!(
            issuer.validate_at(&raw, now),
            Err(IdentityError::Expired)
        ));
    }

    #[test]
    fn skew_tolerance_accepts_just_past_expiry() {
        let (issuer, _) = issuer_with(new_revoked_set());
        let issued = 1_700_000_000;
        let signed = issuer.issue_at(&sample_request(), issued).expect("issue");
        let raw = on_wire(&signed);
        let expires = issued + CERT_LIFETIME_SECS;

        // 1s past nominal expiry but within the ±5 min skew window → accepted.
        assert!(issuer.validate_at(&raw, expires + 1).is_ok());
        // Exactly at the skew boundary (expires + 300) → is_expired(now-300)
        // == is_expired(expires) == true → rejected.
        assert!(matches!(
            issuer.validate_at(&raw, expires + SKEW_TOLERANCE_SECS),
            Err(IdentityError::Expired)
        ));
        // Just inside the boundary → accepted.
        assert!(issuer
            .validate_at(&raw, expires + SKEW_TOLERANCE_SECS - 1)
            .is_ok());
    }

    #[test]
    fn cert_from_different_core_rejected() {
        // Issue with one Core, validate with another.
        let (issuer_a, _) = issuer_with(new_revoked_set());
        let signed = issuer_a
            .issue_at(&sample_request(), 1_700_000_000)
            .expect("issue");
        let raw = on_wire(&signed);

        let other_core = KeyPair::from_seed(&[99u8; 32]);
        let issuer_b = LocalCoreIssuer::new(
            KeyPair::from_seed(&[99u8; 32]),
            other_core.verifying_key(),
            new_revoked_set(),
        );
        // verify_cert against B's key fails the signature first → BadSignature.
        assert!(matches!(
            issuer_b.validate_at(&raw, 1_700_000_001),
            Err(IdentityError::BadSignature)
        ));
    }

    #[test]
    fn revoked_device_rejected() {
        let revoked = new_revoked_set();
        let (issuer, _) = issuer_with(revoked.clone());
        let req = sample_request();
        let signed = issuer.issue_at(&req, 1_700_000_000).expect("issue");
        let raw = on_wire(&signed);

        // Fresh cert validates before revocation.
        assert!(issuer.validate_at(&raw, 1_700_000_001).is_ok());

        // Task 209's revoke path inserts the device_id into the shared set.
        revoked
            .write()
            .unwrap()
            .insert(device_id(&req.device_pubkey));

        assert!(matches!(
            issuer.validate_at(&raw, 1_700_000_001),
            Err(IdentityError::Revoked)
        ));
    }

    #[test]
    fn garbage_bytes_error_never_panic() {
        let (issuer, _) = issuer_with(new_revoked_set());
        // Empty, too-short, and random-length junk all return Err, no panic.
        for raw in [
            &b""[..],
            &b"short"[..],
            &[0xFFu8; 63][..],
            &[0xABu8; 200][..],
        ] {
            assert!(issuer.validate_at(raw, 1_700_000_001).is_err());
        }
    }

    /// Informational hot-path timing for `validate` (NOT a CI gate — loopback
    /// timing is environment-sensitive; see Task 102's spike treatment of
    /// sub-ms numbers). Documents the < 200 µs budget from `design/12 §6.1`.
    #[test]
    fn validate_hot_path_timing_informational() {
        use std::time::Instant;

        let (issuer, _) = issuer_with(new_revoked_set());
        let signed = issuer
            .issue_at(&sample_request(), 1_700_000_000)
            .expect("issue");
        let raw = on_wire(&signed);

        // Warm up.
        for _ in 0..100 {
            let _ = issuer.validate_at(&raw, 1_700_000_001);
        }

        let iters = 2_000u32;
        let start = Instant::now();
        for _ in 0..iters {
            issuer
                .validate_at(&raw, 1_700_000_001)
                .expect("validate in timing loop");
        }
        let per_call = start.elapsed() / iters;
        eprintln!(
            "validate hot-path: {:?}/call (budget <200µs, design/12 §6.1) — informational, not a gate",
            per_call
        );
    }

    /// On-wire framing: `cert_bytes || signature` (Task 205 handoff).
    fn on_wire(signed: &SignedDeviceCert) -> Vec<u8> {
        let mut raw = signed.cert_bytes.clone();
        raw.extend_from_slice(&signed.signature);
        raw
    }
}
