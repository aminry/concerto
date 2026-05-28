//! Crate-local error type for `concerto-keychain`.
//!
//! Per the convention from Task 05 (and to avoid a dependency cycle with
//! `concerto-error`, which itself carries a `Secrets(#[from] SecretsError)`
//! variant), this crate owns its own typed error and a crate-local
//! `Result<T>` alias. Callers at module boundaries can `?`-bubble into
//! `concerto_error::Error` via the `From<SecretsError>` impl that lives in
//! `crates/error/src/api.rs`.
//!
//! Wire codes live on `concerto_error::Error::wire_code()`; the new
//! `Secrets` variant maps to `"secrets"`.

use thiserror::Error as ThisError;

/// Crate-local error type. Bridges to `concerto_error::Error` at module
/// boundaries via a `From<SecretsError>` impl in `concerto-error`.
#[derive(Debug, ThisError)]
pub enum SecretsError {
    /// Best-effort mapping of `keyring::Error::NoEntry`. The public API
    /// translates this to `Ok(None)` at the `Secrets::get` boundary; it
    /// only surfaces here so callers of internal helpers see the
    /// platform's distinction explicitly.
    #[error("keychain entry not found")]
    NotFound,

    /// The OS denied access (e.g., user dismissed the Keychain prompt on
    /// macOS, or the keychain is locked).
    #[error("keychain access denied")]
    AccessDenied,

    /// Any other backend-level failure (platform unsupported, malformed
    /// data, IPC error talking to the secret service, etc.). The original
    /// `keyring::Error` is preserved as the source.
    #[error("keychain platform error: {0}")]
    PlatformError(String),
}

/// Crate-local `Result` alias.
pub type Result<T, E = SecretsError> = std::result::Result<T, E>;

impl From<keyring::Error> for SecretsError {
    fn from(e: keyring::Error) -> Self {
        match e {
            keyring::Error::NoEntry => SecretsError::NotFound,
            // `keyring 3`'s `PlatformFailure` and `BadEncoding` and other
            // backend errors all map to PlatformError. macOS's
            // user-cancelled prompt surfaces as `PlatformFailure` with an
            // OS error code; we can't reliably distinguish it from other
            // platform failures here, so callers see `PlatformError`. The
            // `AccessDenied` variant is reserved for cases we can detect
            // explicitly (currently none on macOS via keyring 3's public
            // API; left in place because Linux Secret Service surfaces a
            // distinct `Locked` state that we'll wire up in V1.0).
            other => SecretsError::PlatformError(other.to_string()),
        }
    }
}
