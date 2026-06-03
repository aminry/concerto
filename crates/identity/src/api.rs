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

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use zeroize::ZeroizeOnDrop;

use crate::error::IdentityError;
use crate::error::Result as IdentityResult;

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

/// Generate a fresh 32-byte Ed25519 seed from OS randomness.
///
/// This is the **persistence path** for the Core's identity (Task 206): the
/// caller (the keychain-backed identity loader in `crates/core`) needs the raw
/// seed to store it durably, then rebuilds the live key with
/// [`KeyPair::from_seed`]. [`KeyPair`] itself never exposes its private bytes —
/// this function exists precisely so the seed is produced once, handed to the
/// secure store, and zeroized by the caller after encoding. Treat the returned
/// array as secret material.
pub fn generate_seed() -> Result<[u8; 32], IdentityError> {
    crate::keys::generate_seed()
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

/// The issuance input the pairing flow hands to [`DeviceCertIssuer::issue`].
///
/// **FROZEN (Task 206).** This is the *issuance-relevant* slice of a pairing
/// request — exactly the fields the issuer needs to mint a cert. Task 207
/// owns the full pairing *message* (the 32-byte token, the nonce, and the
/// device's signature proving possession of `device_pubkey`); it verifies
/// those pairing-channel concerns and then constructs **this exact struct**
/// to call [`DeviceCertIssuer::issue`]. Defining the minimal shape here (rather
/// than importing it from 207) avoids a 206→207 dependency cycle: the issuer
/// crate stays a leaf, and 207 depends on 206.
///
/// `device_id` is *derived* from `device_pubkey` via [`device_id`] inside
/// `issue`; the caller does not supply it (and cannot forge it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingRequest {
    /// The pairing device's Ed25519 public key (32 bytes).
    pub device_pubkey: [u8; 32],
    /// The user-supplied device name captured during pairing.
    pub device_name: String,
}

/// The validated-identity output of [`DeviceCertIssuer::validate`].
///
/// **FROZEN (Task 206).** This is the authenticated principal the auth
/// middleware (Task 210) attaches to every inbound request after a successful
/// cert check. It is the *result* of validation — by the time a caller holds
/// a `DeviceContext`, the signature, expiry, and revocation checks of
/// `design/12 §3.2` have all passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceContext {
    /// `BLAKE2b-256(device_pubkey)` — the canonical device identifier.
    pub device_id: [u8; 32],
    /// The device name carried in the cert.
    pub device_name: String,
    /// The capability tokens granted to this device. V1.0: `["admin"]`.
    pub capabilities: Vec<String>,
}

/// The pluggable device-cert issuer seam (`design/12 §3.10`, locked in
/// `design/18 §3.7`).
///
/// **FROZEN extension seam.** This trait is the boundary through which future
/// enterprise modules (V2.0+ org-managed CA, MDM, OIDC-bridged identity)
/// replace the V1.0 self-issuance flow **without forking the MIT Core**. The
/// MIT Core ships exactly one impl — [`LocalCoreIssuer`] — plus this trait; no
/// MIT code is license-gated.
///
/// V2.0 BSL impls (planned, **not** in the MIT monorepo — reserved here so
/// Task 707's trait-seam completeness check can verify the names without a
/// Core refork, per `design/12 §3.10` / `design/18 §3.7`):
///
/// - `OrgManagedCaIssuer` — `crates/enterprise-managed-ca` (BSL): cert chain
///   Core key → Org root key → Device, root configured via
///   `managed.json.org_root_pubkey`.
/// - `MdmIssuer` — `crates/enterprise-mdm` (BSL): each pairing validated
///   against Jamf / Intune / Workspace ONE; cert lifetime tied to MDM-managed
///   device status.
/// - `OidcBridgeIssuer` — `crates/enterprise-oidc` (BSL): OIDC user assertion
///   required at pairing; user identity bound into the cert; revocation tracks
///   IdP-signaled deprovisioning.
///
/// The Core selects the issuer at startup from `managed.json.identity_issuer`
/// (`"local"` default). All issuers produce wire-compatible
/// [`SignedDeviceCert`] bytes; clients never need to know which issuer minted
/// their cert.
#[async_trait]
pub trait DeviceCertIssuer: Send + Sync + 'static {
    /// Verify the incoming pairing request and produce a signed cert.
    ///
    /// The default impl ([`LocalCoreIssuer`]) signs with the Core's own
    /// Ed25519 identity (`design/12 §3.1`, §3.2). Enterprise impls may
    /// delegate to an org root of trust, an MDM API, or an external CA.
    async fn issue(&self, req: PairingRequest) -> IdentityResult<SignedDeviceCert>;

    /// Validate a cert presented by a client, returning the authenticated
    /// [`DeviceContext`].
    ///
    /// The default impl checks the Core's own signature, expiry (with ±5 min
    /// skew), and revocation membership — the four steps of `design/12 §3.2`,
    /// all in-memory (no DB) to stay within the < 200 µs hot-path budget
    /// (`design/12 §6.1`). Enterprise impls may additionally check an org cert
    /// chain.
    fn validate(&self, raw: &[u8]) -> IdentityResult<DeviceContext>;

    /// List the capability tokens this issuer can attach to certs. V1.0
    /// [`LocalCoreIssuer`] returns `&["admin"]`.
    fn supported_capabilities(&self) -> &'static [&'static str];
}

/// V1.0 MIT device-cert issuer: self-signs device certs with the Core's
/// Ed25519 identity and validates incoming certs against it.
///
/// Construct with [`LocalCoreIssuer::new`]; the implementation lives in
/// [`crate::issuer`]. The revoked-set handle is an [`std::sync::Arc`]-shared
/// `RwLock<HashSet<[u8; 32]>>` that Task 209 also wires into the revoke path,
/// so a `RevokeDevice` RPC and this validator observe the same set (the
/// validator only *reads* it; 209 owns the writes).
pub struct LocalCoreIssuer {
    pub(crate) core_key: KeyPair,
    pub(crate) core_pub: PublicKey,
    pub(crate) revoked: crate::issuer::RevokedSet,
}

impl LocalCoreIssuer {
    /// Build the issuer from the Core's keypair, its public key, and a shared
    /// handle to the revoked-`device_id` set.
    ///
    /// **FROZEN constructor (Task 206).** `core_priv` is the Core's signing
    /// key (loaded from the keychain at boot); `core_pub` is its verifying key
    /// (embedded into every issued cert as `core_pubkey` and checked on
    /// validate); `revoked` is the cheaply-cloneable read handle the validator
    /// consults — Task 209 inserts into the same handle on revoke.
    pub fn new(
        core_priv: KeyPair,
        core_pub: PublicKey,
        revoked: crate::issuer::RevokedSet,
    ) -> Self {
        Self {
            core_key: core_priv,
            core_pub,
            revoked,
        }
    }

    /// The Core's identity public key (mirrors into each issued cert's
    /// `core_pubkey`).
    pub fn core_public_key(&self) -> &PublicKey {
        &self.core_pub
    }
}
