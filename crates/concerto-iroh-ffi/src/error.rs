//! The uniffi-exported error enum (Task 509).
//!
//! Every fallible FFI function returns `Result<_, IrohFfiError>`; uniffi maps
//! the variants to native exceptions on the foreign side. The variants are
//! coarse-grained (the foreign side keys on the kind, the `String` carries the
//! detail) and stable so 510/511 can match on them.

/// Errors surfaced across the FFI boundary.
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum IrohFfiError {
    /// A connect-blob field (endpoint id, relay url, direct addr, token, pubkey)
    /// was malformed.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// Binding the client Iroh endpoint or reconstructing the server address
    /// failed.
    #[error("endpoint error: {0}")]
    Endpoint(String),
    /// The Noise-XX device-pairing exchange failed (or was refused).
    #[error("pairing error: {0}")]
    Pairing(String),
    /// Establishing the Noise-IK + tonic API channel failed.
    #[error("connect error: {0}")]
    Connect(String),
    /// An RPC over the channel failed (carries the gRPC status text).
    #[error("rpc error: {0}")]
    Rpc(String),
    /// The session handle is unknown (already closed / never issued).
    #[error("unknown session handle: {0}")]
    UnknownHandle(u64),
    /// OS randomness / key generation failed.
    #[error("crypto error: {0}")]
    Crypto(String),
}
