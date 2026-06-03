//! `concerto-identity` — pure cryptographic primitives for the Phase-2
//! security spine.
//!
//! This crate is the **trust anchor**: every device-cert operation in Tasks
//! 206–211 composes the functions exported here. It holds only
//! side-effect-free primitives — no actor, no DB, no keychain, no async, no
//! gRPC:
//!
//! - **Ed25519 keys** ([`api::KeyPair`] / [`api::PublicKey`] /
//!   [`api::Signature`]): generate, sign, verify. The private key zeroizes on
//!   drop and is never `Debug`/`Clone`/`Serialize` (mirrors the keychain
//!   crate's `zeroize` posture).
//! - **`device_id`** ([`api::device_id`]): `BLAKE2b-256(device_pubkey)`, the
//!   canonical derivation named in `design/12 §3.2`.
//! - **`DeviceCert` / `SignedDeviceCert`** ([`api::DeviceCert`] /
//!   [`api::SignedDeviceCert`]): deterministic-CBOR encode/decode plus
//!   [`api::sign_cert`] / [`api::verify_cert`], and the pure
//!   [`api::DeviceCert::is_expired`] helper.
//!
//! # Frozen wire contract
//!
//! The `DeviceCert` field layout and its canonical-CBOR encoding are a wire
//! contract: the signature is computed over the exact bytes, and recovery
//! tooling decodes them without the proto schema. Field order = wire order.
//! New fields are append-only with a `version` bump; never reorder. The
//! committed known-answer vector in `tests/cert_vectors.rs` freezes the
//! encoding across versions.
//!
//! The public surface (the types `regen-interfaces.sh` indexes) is declared
//! directly in [`api`]; `impl` bodies live in [`keys`] / [`cert`].

pub mod api;
pub mod error;
pub mod issuer;

pub(crate) mod cert;
pub(crate) mod keys;

pub use api::{
    device_id, encode_cert, generate_seed, sign_cert, verify_cert, DeviceCert, DeviceCertIssuer,
    DeviceContext, KeyPair, LocalCoreIssuer, PairingRequest, PublicKey, Signature,
    SignedDeviceCert,
};
pub use error::{IdentityError, Result};
pub use issuer::{new_revoked_set, RevokedSet, CERT_LIFETIME_SECS, SKEW_TOLERANCE_SECS};
