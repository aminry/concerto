//! Public surface of `concerto-keychain`.
//!
//! Per the Task 04 convention, this file is what `regen-interfaces.sh`
//! reads to produce `docs/interfaces/rust-api.md`. The types live here
//! directly (not as `pub use` re-exports) so the interface generator
//! captures them. Implementation details (impl blocks, account-string
//! mapping, the actual keyring calls) live in `lib.rs`.
//!
//! The wire codes for these secrets are referenced from `design/09 §3.7`;
//! the account-string namespacing scheme is locked by Task 10.

use secrecy::SecretString;

use crate::error::SecretsError;

/// Cloud LLM providers whose API tokens Concerto stores in the OS keychain.
///
/// Stable identifiers: changing any variant name changes the account-string
/// namespacing scheme (see [`SecretKind::to_account_string`]) and would
/// orphan existing keychain entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    Anthropic,
    OpenAI,
    Gemini,
    Bedrock,
    Vertex,
}

/// Typed enumeration of every secret Concerto knows about.
///
/// Each variant maps to exactly one `(service, account)` entry in the OS
/// keychain. The `service` is the constant string `"concerto"`; the
/// `account` is produced by [`SecretKind::to_account_string`].
///
/// Per design/09 §3.7: secret material never leaves the keychain except
/// via [`Secrets::get`]; SQLite only stores references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretKind {
    /// API token for a cloud LLM provider.
    ProviderToken(Provider),
    /// GitHub Personal Access Token (used by Task 45's `gh` CLI shell-out).
    GithubPat,
    /// Symmetric key for the device-pairing handshake (V1.0; placeholder
    /// in V0.1).
    DevicePairingKey,
    /// Ed25519 private key for the Core's persistent identity (per
    /// design/00 §6.7).
    CoreIdentityPrivateKey,
    /// X25519 Noise static **private** key for the Core's Iroh transport
    /// (`design/11 §3.1`, `design/12 §3.1`, Task 217.5). Distinct from
    /// [`SecretKind::CoreIdentityPrivateKey`] (the Ed25519 signing identity):
    /// this is the Noise IK responder static the transport presents on every
    /// Iroh session, persisted so the Core keeps a stable Noise public key
    /// across reboots (the QR's responder static). Stored as the lowercase-hex
    /// of the 32-byte X25519 private key, mirroring the Ed25519-seed encoding.
    CoreNoiseStaticPrivateKey,
    /// API key for the Expo push notification service (V1.0; placeholder
    /// in V0.1).
    PushExpoApiKey,
}

/// A secret value held in process memory.
///
/// Wraps [`secrecy::SecretString`] so the bytes are zeroed on drop and so
/// the `Debug` impl prints `[REDACTED]` instead of the value. The only way
/// to read the underlying string is [`SecretValue::expose`] — the name is
/// deliberate to make callers think about whether they're crossing a
/// trust boundary.
pub struct SecretValue(pub(crate) SecretString);

/// Handle to the OS keychain.
///
/// Cheap to construct and clone (the underlying `keyring::Entry` is built
/// per-call, not cached). All three methods are async to leave room for a
/// future writer-queue indirection; the current implementation runs the
/// blocking `keyring` call on the current task.
#[derive(Debug, Default, Clone)]
pub struct Secrets {
    /// Service name used for every keychain entry. Always `"concerto"` in
    /// production; tests inject a unique service to avoid colliding with
    /// real installations.
    pub(crate) service: std::borrow::Cow<'static, str>,
}

impl SecretValue {
    /// Wrap an owned string as a secret. The original `String` is moved
    /// into the underlying `SecretString` and its memory is zeroed on
    /// drop.
    pub fn new(s: String) -> Self {
        Self(SecretString::from(s))
    }

    /// Reveal the secret as a `&str`.
    ///
    /// This is the ONLY way to extract the inner string from a
    /// `SecretValue`. The name is deliberately verb-y to make callers
    /// think about whether they're crossing a trust boundary; do not use
    /// it casually.
    pub fn expose(&self) -> &str {
        use secrecy::ExposeSecret;
        self.0.expose_secret()
    }
}

impl Secrets {
    /// Construct a `Secrets` handle bound to the default `"concerto"`
    /// service — or to the service named by the `CONCERTO_KEYCHAIN_SERVICE`
    /// environment variable when it is set and non-empty.
    ///
    /// The override exists for **isolation**: integration tests (and
    /// parallel/headless CI jobs) can bind a unique throwaway service so the
    /// process only ever touches a keychain item *it* created. Accessing the
    /// shared `"concerto"` login-keychain item from a *different* unsigned
    /// binary triggers a blocking macOS Keychain Access prompt — and a
    /// headless CI runner has no GUI to answer it, so the call hangs forever.
    /// Production leaves the variable unset and uses `"concerto"`.
    pub fn new() -> Self {
        let service = match std::env::var("CONCERTO_KEYCHAIN_SERVICE") {
            Ok(s) if !s.is_empty() => std::borrow::Cow::Owned(s),
            _ => std::borrow::Cow::Borrowed(crate::DEFAULT_SERVICE),
        };
        Self { service }
    }

    /// Read a secret. Returns `Ok(None)` if no entry exists for `kind`.
    ///
    /// On macOS the first access may trigger a Keychain Access prompt;
    /// subsequent accesses are silent for the lifetime of the user
    /// session.
    pub async fn get(
        &self,
        kind: SecretKind,
    ) -> crate::error::Result<Option<SecretValue>, SecretsError> {
        crate::get_impl(self, kind).await
    }

    /// Write or overwrite the secret for `kind`.
    pub async fn set(
        &self,
        kind: SecretKind,
        value: SecretValue,
    ) -> crate::error::Result<(), SecretsError> {
        crate::set_impl(self, kind, value).await
    }

    /// Delete the entry for `kind`. Idempotent: deleting a missing entry
    /// returns `Ok(())`.
    pub async fn delete(&self, kind: SecretKind) -> crate::error::Result<(), SecretsError> {
        crate::delete_impl(self, kind).await
    }

    /// Read a **per-paired-Core** secret, keyed by `core_id` (`design/15
    /// §3.10.1`).
    ///
    /// Unlike the singleton [`SecretKind`] entries, a paired-Core secret is
    /// parameterized by the Core it belongs to (`core_id = BLAKE2b(core_pubkey)`,
    /// lowercase hex) and the secret [`CoreSecretSlot`] (the device cert or the
    /// device private key). The Desktop's connected-Core registry (Task 218)
    /// stores `cores.json` cleartext metadata and **these** secrets in the OS
    /// keychain so the split-host `IrohCoreClient` can present its device cert
    /// without the secrets ever touching disk. Returns `Ok(None)` when no entry
    /// exists.
    ///
    /// Added as a parameterized accessor (rather than a new [`SecretKind`]
    /// variant) so the closed, `Copy` `SecretKind` enum stays unchanged while
    /// the per-Core keying lands; the Windows keychain backend (Task 608) swaps
    /// underneath this same API.
    pub async fn get_core_secret(
        &self,
        core_id: &str,
        slot: CoreSecretSlot,
    ) -> crate::error::Result<Option<SecretValue>, SecretsError> {
        crate::core_secret_get_impl(self, core_id, slot).await
    }

    /// Write or overwrite a per-paired-Core secret keyed by `core_id`.
    pub async fn set_core_secret(
        &self,
        core_id: &str,
        slot: CoreSecretSlot,
        value: SecretValue,
    ) -> crate::error::Result<(), SecretsError> {
        crate::core_secret_set_impl(self, core_id, slot, value).await
    }

    /// Delete a per-paired-Core secret keyed by `core_id`. Idempotent.
    pub async fn delete_core_secret(
        &self,
        core_id: &str,
        slot: CoreSecretSlot,
    ) -> crate::error::Result<(), SecretsError> {
        crate::core_secret_delete_impl(self, core_id, slot).await
    }
}

/// Which per-paired-Core secret a [`Secrets::get_core_secret`] call addresses
/// (`design/15 §3.10.1`). Each `(core_id, slot)` pair maps to exactly one
/// keychain `(service, account)` entry; the `account` embeds the `core_id` so
/// every paired Core's secrets are isolated.
///
/// Stable identifiers: the account-string slug for each slot is public protocol
/// (changing one orphans existing keychain entries), mirroring the
/// [`SecretKind::to_account_string`] discipline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreSecretSlot {
    /// The `SignedDeviceCert` issued to this Desktop by the paired Core (the
    /// CBOR-encoded `cert_bytes || signature`, base64). Presented in request
    /// metadata by the split-host `IrohCoreClient`.
    DeviceCert,
    /// This Desktop's device Ed25519 private key for the paired Core (the seed,
    /// base64). Never leaves the keychain.
    DevicePrivateKey,
}

impl CoreSecretSlot {
    /// The stable account-string slug for this slot. **Public protocol** —
    /// changing it orphans existing keychain entries.
    pub fn slug(self) -> &'static str {
        match self {
            CoreSecretSlot::DeviceCert => "device_cert",
            CoreSecretSlot::DevicePrivateKey => "device_private_key",
        }
    }
}
