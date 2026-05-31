//! Subcommand implementations for the `concerto` CLI.
//!
//! Each subcommand module exposes a single `pub async fn run(...)` that:
//!
//! 1. Dials the Core via [`crate::client::connect`].
//! 2. Performs its read-only RPC(s) and collects the result into a plain
//!    serde-serializable view struct.
//! 3. Hands the view to a renderer.
//!
//! Rendering is deliberately separated from the RPC calls so that `--json`
//! is a thin switch: every `run` takes an [`OutputFormat`] and dispatches to
//! either a human table renderer or `serde_json` on the same view struct.

pub mod session;
pub mod status;
pub mod workspace;

use std::time::Duration;

/// Per-RPC deadline. 30 s mirrors the smoke client's budget; the connect
/// timeout in [`crate::client`] guards the dial separately.
pub const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// Output mode selected by the global `--json` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// Human-readable tables / key-value lines (the default).
    Text,
    /// A single machine-readable JSON document on stdout.
    Json,
}

impl OutputFormat {
    /// Map the global `--json` boolean onto the format enum.
    pub fn from_json_flag(json: bool) -> Self {
        if json {
            OutputFormat::Json
        } else {
            OutputFormat::Text
        }
    }

    /// True when JSON output was requested.
    pub fn is_json(self) -> bool {
        matches!(self, OutputFormat::Json)
    }
}

/// Errors surfaced by command execution. Wraps the client-layer error plus
/// the RPC-level failures (timeout / gRPC status). `main` renders these to
/// stderr and maps them to a non-zero exit code.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    /// Resolving or dialing the Core socket failed (Core not running, bad
    /// path, …). The inner error names the socket path it tried.
    #[error(transparent)]
    Client(#[from] crate::client::ClientError),
    /// A gRPC call returned an error status. The `Status` is boxed to keep
    /// `CommandError` small (it is ~200 bytes inline, which trips
    /// clippy's `result_large_err` on every `Result<(), CommandError>`).
    #[error("{rpc} failed: {status}")]
    Rpc {
        rpc: &'static str,
        status: Box<tonic::Status>,
    },
    /// A gRPC call exceeded [`RPC_TIMEOUT`].
    #[error("{rpc} timed out after {RPC_TIMEOUT:?}")]
    Timeout { rpc: &'static str },
    /// Serializing the `--json` view failed. Effectively unreachable for
    /// our plain structs, surfaced for completeness rather than a panic.
    #[error("serializing JSON output: {0}")]
    Json(#[from] serde_json::Error),
}

/// Await a unary RPC future with the shared timeout, mapping both the
/// timeout and the gRPC status into a [`CommandError`] tagged with the RPC
/// name (so error messages name the call that failed).
pub(crate) async fn call<T, F>(rpc: &'static str, fut: F) -> Result<T, CommandError>
where
    F: std::future::Future<Output = Result<tonic::Response<T>, tonic::Status>>,
{
    match tokio::time::timeout(RPC_TIMEOUT, fut).await {
        Err(_) => Err(CommandError::Timeout { rpc }),
        Ok(Err(status)) => Err(CommandError::Rpc {
            rpc,
            status: Box::new(status),
        }),
        Ok(Ok(resp)) => Ok(resp.into_inner()),
    }
}
