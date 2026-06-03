//! Crate-local error type for `concerto-identity`.
//!
//! Mirrors the keychain crate's pattern: this crate owns its own typed
//! error so it stays a pure leaf with no dependency on `concerto-error`.
//! The wiring tasks (206+) bridge this into `concerto_error::Error` at their
//! module boundaries.
//!
//! Every variant is a *value* error (bad input, failed check) — these
//! functions never panic on attacker-controlled bytes, which is what lets
//! Task 208 fuzz `verify_cert`.

use thiserror::Error as ThisError;

/// Crate-local error type for the identity primitives.
#[derive(Debug, ThisError)]
pub enum IdentityError {
    /// The OS randomness source failed during key generation.
    #[error("failed to read OS randomness: {0}")]
    Rng(String),

    /// A 32-byte slice was not a valid Ed25519 public key (not a curve
    /// point).
    #[error("invalid Ed25519 public key encoding")]
    BadPublicKey,

    /// Signature verification failed (wrong key, tampered message, or
    /// malformed signature).
    #[error("signature verification failed")]
    BadSignature,

    /// The raw cert bytes could not be split into `cert_bytes || signature`
    /// (too short to contain the trailing 64-byte signature).
    #[error("cert too short to contain a 64-byte signature")]
    Truncated,

    /// The CBOR body did not decode into a `DeviceCert`.
    #[error("malformed CBOR cert: {0}")]
    BadCbor(String),
}
