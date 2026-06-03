//! `DeviceCert` canonical-CBOR codec + sign/verify — implementation behind
//! the [`crate::api`] surface.
//!
//! # Canonicalization (the load-bearing detail)
//!
//! The [`DeviceCert`] is a `serde`-derived struct. `ciborium` serializes a
//! struct's fields **in declaration order** as a CBOR map keyed by the field
//! names; for a fixed struct the byte output is deterministic across runs and
//! platforms because there is no map-key reordering, no float, and no set.
//! Field order in the struct therefore IS the frozen wire order. This is not
//! merely assumed — the byte-stability test and the committed known-answer
//! vector in `tests/cert_vectors.rs` pin the exact bytes; if a future
//! `ciborium`/`serde` upgrade ever perturbed them, those tests fail loudly.
//!
//! The signature is computed over these canonical bytes; the on-wire form is
//! `cert_bytes || signature` (CBOR body then the raw 64-byte Ed25519
//! signature). Recovery tooling can decode `cert_bytes` with any CBOR reader
//! and verify the trailing signature without the proto schema (`design/12
//! §3.2`, R-1).

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};

use crate::api::{DeviceCert, KeyPair, PublicKey, SignedDeviceCert};
use crate::error::IdentityError;

/// BLAKE2b-256 (32-byte output), matching the digest Task 203 pins for the
/// `Files` checksum.
type Blake2b256 = Blake2b<U32>;

pub(crate) fn device_id(pubkey: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Blake2b256::new();
    hasher.update(pubkey);
    hasher.finalize().into()
}

pub(crate) fn encode_cert(cert: &DeviceCert) -> Result<Vec<u8>, IdentityError> {
    let mut buf = Vec::new();
    ciborium::into_writer(cert, &mut buf).map_err(|e| IdentityError::BadCbor(e.to_string()))?;
    Ok(buf)
}

fn decode_cert(bytes: &[u8]) -> Result<DeviceCert, IdentityError> {
    ciborium::from_reader(bytes).map_err(|e| IdentityError::BadCbor(e.to_string()))
}

pub(crate) fn sign_cert(
    core_key: &KeyPair,
    cert: &DeviceCert,
) -> Result<SignedDeviceCert, IdentityError> {
    let cert_bytes = encode_cert(cert)?;
    let sig = core_key.sign(&cert_bytes);
    Ok(SignedDeviceCert {
        cert_bytes,
        cert: cert.clone(),
        signature: sig.to_bytes(),
    })
}

pub(crate) fn verify_cert(raw: &[u8], core_pub: &PublicKey) -> Result<DeviceCert, IdentityError> {
    // The on-wire form is `cert_bytes || signature`; the signature is the
    // trailing 64 bytes. Reject anything too short before touching crypto.
    if raw.len() < 64 {
        return Err(IdentityError::Truncated);
    }
    let split = raw.len() - 64;
    let (cert_bytes, sig_bytes) = raw.split_at(split);

    let mut sig_arr = [0u8; 64];
    sig_arr.copy_from_slice(sig_bytes);
    let sig = crate::api::Signature::from_bytes(&sig_arr);

    // Verify the signature over the raw CBOR body BEFORE decoding, so we
    // never run the decoder on bytes that didn't come from the Core. (Decode
    // is panic-safe regardless, but this keeps the trust order strict.)
    core_pub.verify(cert_bytes, &sig)?;

    // Structural validity: it must decode as a DeviceCert, and re-encoding
    // must reproduce the exact bytes the signature covered (rejects any
    // non-canonical or trailing-garbage CBOR that happened to decode).
    let cert = decode_cert(cert_bytes)?;
    let reencoded = encode_cert(&cert)?;
    if reencoded != cert_bytes {
        return Err(IdentityError::BadCbor(
            "cert bytes are not canonical".to_string(),
        ));
    }
    Ok(cert)
}

impl DeviceCert {
    /// Pure expiry helper: `now_unix >= expires_at`.
    ///
    /// Skew tolerance is the caller's policy (Task 206 applies ±5min per
    /// `design/12 §8`); 205 exposes only the exact comparison.
    pub fn is_expired(&self, now_unix: u64) -> bool {
        now_unix >= self.expires_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::device_id as api_device_id;

    fn sample_cert(core_pub: [u8; 32], device_pub: [u8; 32]) -> DeviceCert {
        DeviceCert {
            version: 1,
            device_id: device_id(&device_pub),
            device_pubkey: device_pub,
            device_name: "Test Phone".to_string(),
            core_pubkey: core_pub,
            issued_at: 1_700_000_000,
            expires_at: 1_700_000_000 + 365 * 24 * 60 * 60,
            capabilities: vec!["admin".to_string()],
            revocation_check_required: true,
        }
    }

    #[test]
    fn device_id_is_blake2b256_and_deterministic() {
        let pk = [9u8; 32];
        let a = device_id(&pk);
        let b = api_device_id(&pk);
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
    }

    #[test]
    fn encode_is_byte_stable() {
        let cert = sample_cert([1u8; 32], [2u8; 32]);
        let a = encode_cert(&cert).unwrap();
        let b = encode_cert(&cert).unwrap();
        assert_eq!(a, b, "canonical CBOR must be byte-stable");
    }

    #[test]
    fn cbor_roundtrip() {
        let cert = sample_cert([3u8; 32], [4u8; 32]);
        let bytes = encode_cert(&cert).unwrap();
        let back = decode_cert(&bytes).unwrap();
        assert_eq!(cert, back);
    }

    #[test]
    fn is_expired_boundary() {
        let cert = sample_cert([1u8; 32], [2u8; 32]);
        assert!(!cert.is_expired(cert.expires_at - 1));
        assert!(cert.is_expired(cert.expires_at));
        assert!(cert.is_expired(cert.expires_at + 1));
    }
}
