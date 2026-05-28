//! Public surface of `concerto-error`.
//!
//! Per the convention locked in Task 04, this module is what
//! `scripts/regen-interfaces.sh` reads to produce
//! `docs/interfaces/rust-api.md`. Types live here directly (not as
//! `pub use` re-exports) so the interface generator captures them.

use thiserror::Error as ThisError;

/// The top-level Concerto error type.
///
/// Variants exist to cover the seams where typed errors cross crate
/// boundaries: I/O, persistence, gRPC, pairing/auth, and a catch-all
/// `Internal` for invariants the type system can't capture.
///
/// The [`wire_code`](Error::wire_code) method returns a stable kebab-case
/// identifier per design/00 §7.3. Those identifiers are exposed verbatim
/// over the wire by the gRPC server (Task 13); renaming any of them is a
/// breaking change.
#[derive(Debug, ThisError)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Boxed because `sqlx::Error` is large (~176 bytes); leaving it
    /// unboxed bloats every `Result<_, Error>` past `clippy::result_large_err`.
    /// The `From<sqlx::Error>` impl handles the `Box::new` automatically so
    /// callers can still `err?`.
    #[error("sqlx: {0}")]
    Sqlx(Box<sqlx::Error>),

    /// Boxed for the same reason as `Sqlx`.
    #[error("tonic: {0}")]
    Tonic(Box<tonic::Status>),

    #[error("pairing: {0}")]
    Pairing(String),

    /// OS keychain / secret-store failure. Bridged from
    /// `concerto_keychain::SecretsError` at module boundaries.
    #[error("secrets: {0}")]
    Secrets(#[from] concerto_keychain::SecretsError),

    /// Git operation failure (shell-out or gix). Added in Task 18 so
    /// `concerto-gix-wrap` can bubble structured failures across the
    /// crate boundary without leaking `gix`'s deep error tree.
    #[error("git: {0}")]
    Git(String),

    /// Caller-facing input validation failure (e.g. missing required
    /// field, malformed slug, V0.1 multi-repo workspace request).
    /// Added in Task 19. Surfaces as `Code::InvalidArgument` over gRPC;
    /// the message string may carry a specific wire code embedded as
    /// the prefix (e.g. `workspace.v0_single_repo_only`) for clients
    /// that switch on it.
    #[error("validation: {0}")]
    Validation(String),

    /// Caller-facing "no such entity" failure (e.g. workspace id
    /// doesn't exist, project id missing). Added in Task 19. Surfaces
    /// as `Code::NotFound` over gRPC.
    #[error("not_found: {0}")]
    NotFound(String),

    #[error("internal: {0}")]
    Internal(String),
}

impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        Self::Sqlx(Box::new(e))
    }
}

impl From<tonic::Status> for Error {
    fn from(e: tonic::Status) -> Self {
        Self::Tonic(Box::new(e))
    }
}

/// Concerto-wide `Result` alias. Defaults its error type to [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
