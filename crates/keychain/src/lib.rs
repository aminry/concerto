//! Concerto typed keychain wrapper.
//!
//! Wraps [`keyring`] in a Concerto-specific API that:
//! - Namespaces entries under a single `service` (`"concerto"` in
//!   production) with a typed `account` per [`api::SecretKind`].
//! - Returns typed [`api::SecretValue`]s that zero their memory on drop.
//! - Emits a `tracing` event on every successful access (kind only, never
//!   the value). The structured audit-log writer arrives in Task 44; for
//!   V0.1 Phase 1, a `tracing` event is sufficient.
//!
//! After this crate lands, every later subsystem that needs a secret
//! calls [`api::Secrets::get`] / `set` / `delete` instead of touching
//! `keyring` directly. Provider tokens, GitHub PATs, and the Core's
//! Ed25519 identity all flow through this single API.
//!
//! The wire codes for these secrets are referenced from `design/09 §3.7`;
//! the account-string namespacing scheme is locked by Task 10 and changing
//! any account string would orphan existing keychain entries.

pub mod api;
pub mod error;

pub use api::{Provider, SecretKind, SecretValue, Secrets};
pub use error::{Result, SecretsError};

/// Service name used for every Concerto keychain entry. Tests override
/// this via [`Secrets::with_service_for_test`]; production code never
/// touches the field directly.
pub(crate) const DEFAULT_SERVICE: &str = "concerto";

impl SecretKind {
    /// Account string used for the `(service, account)` keychain entry.
    ///
    /// Locked by Task 10. The strings here are public protocol — changing
    /// any of them would orphan existing entries in users' keychains.
    /// New variants append to this list; never renumber or rename.
    pub fn to_account_string(&self) -> String {
        match self {
            SecretKind::ProviderToken(p) => format!("provider_token.{}", provider_slug(*p)),
            SecretKind::GithubPat => "vcs.github.pat".to_string(),
            SecretKind::DevicePairingKey => "device.pairing_key".to_string(),
            SecretKind::CoreIdentityPrivateKey => "identity.core_private_key".to_string(),
            SecretKind::PushExpoApiKey => "push.expo_api_key".to_string(),
        }
    }
}

/// Stable lowercase slugs for [`Provider`]. Part of the public account
/// namespace; changing a slug orphans existing keychain entries.
fn provider_slug(p: Provider) -> &'static str {
    match p {
        Provider::Anthropic => "anthropic",
        Provider::OpenAI => "openai",
        Provider::Gemini => "gemini",
        Provider::Bedrock => "bedrock",
        Provider::Vertex => "vertex",
    }
}

impl Secrets {
    /// Construct a `Secrets` handle bound to a caller-supplied service
    /// name. **Tests only** — production code uses [`Secrets::new`].
    ///
    /// Per the task spec, tests should use a unique service name (e.g.,
    /// `"concerto-test-<uuid>"`) to avoid colliding with real entries on
    /// developer machines.
    #[doc(hidden)]
    pub fn with_service_for_test(service: impl Into<String>) -> Self {
        Self {
            service: std::borrow::Cow::Owned(service.into()),
        }
    }
}

fn entry(secrets: &Secrets, kind: SecretKind) -> Result<keyring::Entry> {
    let account = kind.to_account_string();
    keyring::Entry::new(secrets.service.as_ref(), &account).map_err(SecretsError::from)
}

pub(crate) async fn get_impl(secrets: &Secrets, kind: SecretKind) -> Result<Option<SecretValue>> {
    let entry = entry(secrets, kind)?;
    match entry.get_password() {
        Ok(s) => {
            tracing::info!(
                target: "concerto::keychain",
                kind = ?kind,
                account = %kind.to_account_string(),
                "secret accessed",
            );
            Ok(Some(SecretValue::new(s)))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(SecretsError::from(e)),
    }
}

pub(crate) async fn set_impl(
    secrets: &Secrets,
    kind: SecretKind,
    value: SecretValue,
) -> Result<()> {
    let entry = entry(secrets, kind)?;
    entry
        .set_password(value.expose())
        .map_err(SecretsError::from)?;
    tracing::info!(
        target: "concerto::keychain",
        kind = ?kind,
        account = %kind.to_account_string(),
        "secret written",
    );
    Ok(())
}

pub(crate) async fn delete_impl(secrets: &Secrets, kind: SecretKind) -> Result<()> {
    let entry = entry(secrets, kind)?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {
            tracing::info!(
                target: "concerto::keychain",
                kind = ?kind,
                account = %kind.to_account_string(),
                "secret deleted",
            );
            Ok(())
        }
        Err(e) => Err(SecretsError::from(e)),
    }
}

#[cfg(test)]
mod account_strings {
    //! Account-string scheme is part of the public protocol (locked by
    //! Task 10). These tests pin every variant so a typo in a future
    //! refactor fails the build instead of silently orphaning users'
    //! keychain entries.

    use super::*;

    #[test]
    fn provider_token_anthropic() {
        assert_eq!(
            SecretKind::ProviderToken(Provider::Anthropic).to_account_string(),
            "provider_token.anthropic"
        );
    }

    #[test]
    fn provider_token_openai() {
        assert_eq!(
            SecretKind::ProviderToken(Provider::OpenAI).to_account_string(),
            "provider_token.openai"
        );
    }

    #[test]
    fn provider_token_gemini() {
        assert_eq!(
            SecretKind::ProviderToken(Provider::Gemini).to_account_string(),
            "provider_token.gemini"
        );
    }

    #[test]
    fn provider_token_bedrock() {
        assert_eq!(
            SecretKind::ProviderToken(Provider::Bedrock).to_account_string(),
            "provider_token.bedrock"
        );
    }

    #[test]
    fn provider_token_vertex() {
        assert_eq!(
            SecretKind::ProviderToken(Provider::Vertex).to_account_string(),
            "provider_token.vertex"
        );
    }

    #[test]
    fn github_pat() {
        assert_eq!(SecretKind::GithubPat.to_account_string(), "vcs.github.pat");
    }

    #[test]
    fn device_pairing_key() {
        assert_eq!(
            SecretKind::DevicePairingKey.to_account_string(),
            "device.pairing_key"
        );
    }

    #[test]
    fn core_identity_private_key() {
        assert_eq!(
            SecretKind::CoreIdentityPrivateKey.to_account_string(),
            "identity.core_private_key"
        );
    }

    #[test]
    fn push_expo_api_key() {
        assert_eq!(
            SecretKind::PushExpoApiKey.to_account_string(),
            "push.expo_api_key"
        );
    }
}
