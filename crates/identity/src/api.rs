//! Public surface of `concerto-identity`.
//!
//! Per the keychain convention (Task 04 / locked in `tasks/v1.0/205`), this
//! file is what `scripts/regen-interfaces.sh` reads to produce
//! `docs/interfaces/rust-api.md`. The canonical types and function
//! signatures are declared **directly here** (not as `pub use` re-exports)
//! so the interface generator captures them; their `impl` blocks live in
//! `keys.rs` / `cert.rs`.
//!
//! These are the pure, side-effect-free crypto primitives the rest of the
//! Phase-2 security spine (Tasks 206–211) composes: Ed25519 key/sign/verify,
//! BLAKE2b `device_id`, and the deterministic-CBOR `DeviceCert` sign/verify.
//! No actor, no DB, no keychain, no async, no gRPC.

use serde::{Deserialize, Serialize};
use zeroize::ZeroizeOnDrop;

use crate::error::IdentityError;

/// An Ed25519 signing (private) key.
///
/// The secret scalar lives inside `ed25519-dalek`'s `SigningKey`, which
/// implements `Zeroize`; this wrapper derives [`ZeroizeOnDrop`] so the key
/// material is wiped when the value is dropped. The type deliberately does
/// **not** implement `Debug`, `Display`, `Clone`, `Serialize`, or any other
/// trait that could leak the private bytes — the only export path is
/// [`KeyPair::verifying_key`] (which yields the *public* half).
#[derive(ZeroizeOnDrop)]
pub struct KeyPair {
    pub(crate) signing: ed25519_dalek::SigningKey,
}

/// An Ed25519 verifying (public) key — 32 bytes.
///
/// Cheap to copy and safe to log/serialize (it is public material).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicKey {
    pub(crate) verifying: ed25519_dalek::VerifyingKey,
}

/// A 64-byte Ed25519 signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature {
    pub(crate) inner: ed25519_dalek::Signature,
}

/// The unsigned device certificate.
///
/// **Wire contract — FROZEN.** The field order below IS the canonical CBOR
/// encoding order: a `serde`-derived struct serializes fields in declaration
/// order, and the byte-stability + known-answer tests in
/// `tests/cert_vectors.rs` pin the exact bytes across versions and platforms.
/// The Core's signature is computed over these exact bytes, and recovery
/// tooling decodes them without the proto schema (`design/12 §3.2`, R-1).
///
/// New fields are **append-only** and require a `version` bump; never
/// reorder or remove a field. `capabilities` is an ordered `Vec` (never a
/// map/set) so the encoding stays unambiguous; V1.0 always emits `["admin"]`
/// (`design/12 §3.2`, `design/10` R-7: missing/empty ⇒ "admin").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCert {
    /// Format version. V1.0 = `1`. Bump on any append-only field addition.
    pub version: u8,
    /// `BLAKE2b-256(device_pubkey)` — the canonical [`device_id`] derivation.
    pub device_id: [u8; 32],
    /// The device's Ed25519 public key.
    pub device_pubkey: [u8; 32],
    /// User-supplied name captured at pairing time.
    pub device_name: String,
    /// The issuing Core's Ed25519 identity public key (for cross-machine
    /// validation; clients carry this from pairing).
    pub core_pubkey: [u8; 32],
    /// Issuance time, unix epoch seconds.
    pub issued_at: u64,
    /// Expiry time, unix epoch seconds (default issuer policy: +365 days,
    /// owned by Task 206 — 205 only exposes the pure [`DeviceCert::is_expired`]
    /// helper).
    pub expires_at: u64,
    /// Capability tokens. V1.0: `["admin"]`. Ordered; never a set/map.
    pub capabilities: Vec<String>,
    /// Whether the auth path must consult the revocation set. V1.0: `true`.
    pub revocation_check_required: bool,
}

/// A [`DeviceCert`] plus the Core's Ed25519 signature over its canonical
/// CBOR encoding.
///
/// **Wire contract — FROZEN.** `cert_bytes` holds the exact canonical-CBOR
/// bytes the signature was computed over; storing the bytes (rather than
/// re-encoding on demand) guarantees the signature can be re-verified
/// byte-for-byte regardless of future serde/ciborium behaviour. The
/// signature is `core_priv`'s Ed25519 over `cert_bytes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedDeviceCert {
    /// The canonical CBOR encoding of [`SignedDeviceCert::cert`] — the exact
    /// bytes the signature covers and the bytes sent on the wire.
    pub cert_bytes: Vec<u8>,
    /// The decoded certificate (a convenience view of `cert_bytes`).
    pub cert: DeviceCert,
    /// The Core's Ed25519 signature over `cert_bytes`.
    pub signature: [u8; 64],
}

impl KeyPair {
    /// Generate a fresh Ed25519 keypair from OS randomness.
    pub fn generate() -> Result<Self, IdentityError> {
        crate::keys::generate()
    }

    /// Reconstruct a keypair from a 32-byte Ed25519 seed (the private key).
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        crate::keys::from_seed(seed)
    }

    /// The public (verifying) half of this keypair.
    pub fn verifying_key(&self) -> PublicKey {
        crate::keys::verifying_key(self)
    }

    /// Sign `msg` with the private key, producing a 64-byte signature.
    pub fn sign(&self, msg: &[u8]) -> Signature {
        crate::keys::sign(self, msg)
    }
}

impl PublicKey {
    /// Construct a public key from its 32-byte encoding.
    ///
    /// Returns `Err` if the bytes are not a valid Ed25519 point.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, IdentityError> {
        crate::keys::public_from_bytes(bytes)
    }

    /// The 32-byte encoding of this public key.
    pub fn to_bytes(&self) -> [u8; 32] {
        crate::keys::public_to_bytes(self)
    }

    /// Verify `sig` over `msg` against this public key.
    ///
    /// Returns `Err(IdentityError::BadSignature)` on any failure; never
    /// panics.
    pub fn verify(&self, msg: &[u8], sig: &Signature) -> Result<(), IdentityError> {
        crate::keys::verify(self, msg, sig)
    }
}

impl Signature {
    /// The 64-byte encoding of this signature.
    pub fn to_bytes(&self) -> [u8; 64] {
        self.inner.to_bytes()
    }

    /// Construct a signature from its 64-byte encoding (always succeeds;
    /// validity is checked at verify time).
    pub fn from_bytes(bytes: &[u8; 64]) -> Self {
        Self {
            inner: ed25519_dalek::Signature::from_bytes(bytes),
        }
    }
}

/// Derive the canonical `device_id` from an Ed25519 public key:
/// `BLAKE2b-256(device_pubkey)` (`design/12 §3.2`). Deterministic.
pub fn device_id(pubkey: &[u8; 32]) -> [u8; 32] {
    crate::cert::device_id(pubkey)
}

/// Encode a [`DeviceCert`] to its canonical CBOR bytes.
///
/// Same input always yields byte-identical output (frozen wire contract).
pub fn encode_cert(cert: &DeviceCert) -> Result<Vec<u8>, IdentityError> {
    crate::cert::encode_cert(cert)
}

/// Sign a [`DeviceCert`] with the Core's keypair, producing a
/// [`SignedDeviceCert`] whose signature covers the canonical CBOR bytes.
pub fn sign_cert(core_key: &KeyPair, cert: &DeviceCert) -> Result<SignedDeviceCert, IdentityError> {
    crate::cert::sign_cert(core_key, cert)
}

/// Verify raw canonical-CBOR cert bytes against `core_pub`: decode, check the
/// Ed25519 signature, and return the structurally-valid [`DeviceCert`].
///
/// This is signature + structural validity only — expiry and revocation are
/// the caller's job (Tasks 206/209). Garbage/truncated input returns `Err`,
/// never panics; shaped so Task 208 can drop a `cargo-fuzz` target on it.
///
/// `raw` is the [`SignedDeviceCert::cert_bytes`] concatenated with the
/// 64-byte signature (`cert_bytes || signature`) — the on-wire form.
pub fn verify_cert(raw: &[u8], core_pub: &PublicKey) -> Result<DeviceCert, IdentityError> {
    crate::cert::verify_cert(raw, core_pub)
}
