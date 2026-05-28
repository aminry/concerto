//! Implementation details for [`crate::api::Error`].
//!
//! The enum declaration itself lives in `api.rs` (so the interface generator
//! picks it up). The `impl` block lives here to keep the public surface
//! free of method bodies.

use crate::api::Error;

impl Error {
    /// Stable kebab-case identifier for this error category, surfaced on
    /// the wire by the gRPC server (Task 13). These strings are part of
    /// the public protocol — renaming any of them is a breaking change.
    pub fn wire_code(&self) -> &'static str {
        match self {
            Error::Io(_) => "io",
            Error::Sqlx(_) => "sqlx",
            Error::Tonic(_) => "tonic",
            Error::Pairing(_) => "pairing",
            Error::Secrets(_) => "secrets",
            Error::Git(_) => "git",
            Error::Internal(_) => "internal",
        }
    }
}
