//! Ed25519 key primitives — implementation behind the [`crate::api`] surface.
//!
//! `generate` fills a 32-byte seed from the OS RNG (`getrandom`) and builds a
//! `SigningKey` from it; this avoids coupling to a particular `rand_core`
//! major version. The private key never leaves this crate except as its
//! public half ([`KeyPair::verifying_key`]).

use ed25519_dalek::{Signer, SigningKey, Verifier, VerifyingKey};

use crate::api::{KeyPair, PublicKey, Signature};
use crate::error::IdentityError;

/// Generate a fresh keypair from 32 OS-random bytes.
pub(crate) fn generate() -> Result<KeyPair, IdentityError> {
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| IdentityError::Rng(e.to_string()))?;
    let kp = from_seed(&seed);
    // The seed has been copied into the SigningKey; wipe our stack copy.
    use zeroize::Zeroize;
    seed.zeroize();
    Ok(kp)
}

/// Build a keypair from a fixed 32-byte seed (deterministic — used by the
/// known-answer test vector).
pub(crate) fn from_seed(seed: &[u8; 32]) -> KeyPair {
    KeyPair {
        signing: SigningKey::from_bytes(seed),
    }
}

pub(crate) fn verifying_key(kp: &KeyPair) -> PublicKey {
    PublicKey {
        verifying: kp.signing.verifying_key(),
    }
}

pub(crate) fn sign(kp: &KeyPair, msg: &[u8]) -> Signature {
    Signature {
        inner: kp.signing.sign(msg),
    }
}

pub(crate) fn public_from_bytes(bytes: &[u8; 32]) -> Result<PublicKey, IdentityError> {
    let verifying = VerifyingKey::from_bytes(bytes).map_err(|_| IdentityError::BadPublicKey)?;
    Ok(PublicKey { verifying })
}

pub(crate) fn public_to_bytes(pk: &PublicKey) -> [u8; 32] {
    pk.verifying.to_bytes()
}

pub(crate) fn verify(pk: &PublicKey, msg: &[u8], sig: &Signature) -> Result<(), IdentityError> {
    pk.verifying
        .verify(msg, &sig.inner)
        .map_err(|_| IdentityError::BadSignature)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_sign_verify_roundtrip() {
        let kp = generate().expect("rng");
        let pk = verifying_key(&kp);
        let msg = b"concerto identity test";
        let sig = sign(&kp, msg);
        assert!(verify(&pk, msg, &sig).is_ok());
    }

    #[test]
    fn verify_rejects_tampered_message() {
        let kp = generate().expect("rng");
        let pk = verifying_key(&kp);
        let sig = sign(&kp, b"original");
        assert!(matches!(
            verify(&pk, b"tampered", &sig),
            Err(IdentityError::BadSignature)
        ));
    }

    #[test]
    fn verify_rejects_wrong_key() {
        let kp = generate().expect("rng");
        let other = generate().expect("rng");
        let pk_other = verifying_key(&other);
        let sig = sign(&kp, b"msg");
        assert!(verify(&pk_other, b"msg", &sig).is_err());
    }

    #[test]
    fn from_seed_is_deterministic() {
        let seed = [7u8; 32];
        let a = verifying_key(&from_seed(&seed)).to_bytes();
        let b = verifying_key(&from_seed(&seed)).to_bytes();
        assert_eq!(a, b);
    }

    #[test]
    fn public_from_bytes_rejects_non_curve_point() {
        // A compressed Edwards point whose y-coordinate is not a residue on
        // the curve fails decompression. `0x02 || 0x00*31` encodes y=2, which
        // is not a valid Ed25519 point (no matching x). dalek's `from_bytes`
        // attempts decompression and returns Err.
        let mut bad = [0u8; 32];
        bad[0] = 0x02;
        assert!(public_from_bytes(&bad).is_err());
    }

    #[test]
    fn public_key_roundtrip() {
        let kp = generate().expect("rng");
        let pk = verifying_key(&kp);
        let bytes = pk.to_bytes();
        let pk2 = public_from_bytes(&bytes).expect("valid point");
        assert_eq!(pk, pk2);
    }
}
