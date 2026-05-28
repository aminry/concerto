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
    /// service.
    pub fn new() -> Self {
        Self {
            service: std::borrow::Cow::Borrowed(crate::DEFAULT_SERVICE),
        }
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
}
