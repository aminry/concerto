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

/// Crate-local result alias.
///
/// The FROZEN [`crate::api::DeviceCertIssuer`] trait signatures
/// (`design/12 §3.10`) are written as `Result<SignedDeviceCert>` /
/// `Result<DeviceContext>`; this alias supplies the `IdentityError` error
/// type those signatures elide.
pub type Result<T> = std::result::Result<T, IdentityError>;

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

    // ---- Issuer / validator policy errors (Task 206) ----
    // These are the validation-step failures the `LocalCoreIssuer::validate`
    // hot path (`design/12 §3.2`) returns. They are distinct from the pure
    // `verify_cert` errors above so the auth middleware (Task 210) can map
    // each to a precise `UNAUTHENTICATED` reason string (`design/12 §8`).
    /// The cert's `expires_at` is in the past (beyond the ±5 min skew
    /// tolerance of `design/12 §8`).
    #[error("device cert expired")]
    Expired,

    /// The cert's `device_id` is present in the in-memory revoked set
    /// (`design/12 §3.11`).
    #[error("device cert revoked")]
    Revoked,

    /// The cert's embedded `core_pubkey` does not match this Core's identity
    /// — the client was paired with a different Core (`design/12 §8`).
    #[error("device cert issued by a different Core")]
    WrongCore,
}
