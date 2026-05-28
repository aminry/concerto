//! Thin gRPC client wrapper used by the Tauri command dispatcher.
//!
//! V0.1 keeps the client deliberately simple:
//!
//! - One fresh Tonic `Channel` per RPC call (no long-lived connection,
//!   no multiplexer). Persistent client + subscription multiplexer
//!   come in Phase 2 (Task 18+).
//! - UDS-only. Iroh transport is V1.0 (`design/15 §3.2`,
//!   `design/12 §3.3`).
//! - The UDS connect path is the same one locked by `tasks/13` — see
//!   `crates/core/tests/grpc_runtime.rs::connect_client`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};

/// Errors surfaced from the Tauri-command dispatcher.
///
/// `serde::Serialize` is required so Tauri can pass these to the
/// renderer as a typed JSON error. We do not leak `tonic::Status`
/// internals beyond their human-readable message — the renderer is
/// untrusted in the Tauri trust model and shouldn't see raw gRPC
/// codes until a future task adds a structured error envelope.
#[derive(Debug, thiserror::Error, serde::Serialize)]
#[serde(tag = "kind", content = "message")]
pub enum CoreClientError {
    #[error("not implemented: {0}")]
    NotImplemented(String),
    #[error("transport: {0}")]
    Transport(String),
    #[error("rpc: {0}")]
    Rpc(String),
}

/// Resolve the default UDS path the Core binds at: `<HOME>/.concerto/core.sock`.
///
/// Matches the layout locked by `tasks/11` (`RuntimeConfig::config_dir`)
/// and consumed by `tasks/13` (`ApiServerConfig::socket_path`).
/// Returns `None` if `$HOME` is unset — only realistic on the most
/// stripped-down test environments.
pub fn default_socket_path() -> Option<PathBuf> {
    let home = home::home_dir()?;
    Some(home.join(".concerto").join("core.sock"))
}

/// Build a Tonic `Channel` that routes every connection to a UDS at
/// `socket_path`. Returns an error if the socket can't be opened — the
/// caller maps that into `CoreClientError::Transport`.
///
/// The URI fed to `Endpoint::try_from` is a placeholder; the
/// `connect_with_connector` closure overrides it on every dial. This
/// is the exact pattern the Task 13 integration test pins.
pub async fn connect_uds(socket_path: &Path) -> Result<Channel, CoreClientError> {
    let path: PathBuf = socket_path.to_path_buf();
    let endpoint = Endpoint::try_from("http://[::1]:50051")
        .map_err(|e| CoreClientError::Transport(format!("endpoint init: {e}")))?
        .connect_timeout(Duration::from_secs(2));

    endpoint
        .connect_with_connector(tower::service_fn(move |_: Uri| {
            let p = path.clone();
            async move {
                let stream = UnixStream::connect(&p).await?;
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        }))
        .await
        .map_err(|e| CoreClientError::Transport(format!("connect {}: {e}", socket_path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::net::UnixListener;

    #[tokio::test]
    async fn connect_uds_succeeds_against_a_live_listener() {
        // Spawn a bare `UnixListener` (no gRPC server behind it) and
        // verify the connector dials it successfully. This exercises
        // the connector plumbing without standing up a full Tonic
        // server — that surface is already covered by Task 13's
        // integration tests.
        let tmp = TempDir::new().unwrap();
        let sock = tmp.path().join("test.sock");
        let listener = UnixListener::bind(&sock).expect("bind UDS listener");

        // Accept one connection in the background so the dial completes.
        let accept_task = tokio::spawn(async move {
            let _ = listener.accept().await;
        });

        let channel = connect_uds(&sock).await.expect("connector should dial");
        // Drop the channel to release the connection; the accept task
        // resolves whether the inner stream is fully closed or not.
        drop(channel);
        // Give the accept task up to 500ms to wrap up; non-fatal if
        // it doesn't — the assertion above already proved the dial.
        let _ = tokio::time::timeout(Duration::from_millis(500), accept_task).await;
    }

    #[tokio::test]
    async fn connect_uds_fails_when_socket_missing() {
        let tmp = TempDir::new().unwrap();
        let sock = tmp.path().join("missing.sock");
        let err = connect_uds(&sock).await.expect_err("should fail");
        match err {
            CoreClientError::Transport(_) => {}
            other => panic!("expected Transport error, got {other:?}"),
        }
    }

    #[test]
    fn default_socket_path_is_under_dot_concerto() {
        if let Some(p) = default_socket_path() {
            let s = p.to_string_lossy();
            assert!(
                s.ends_with("/.concerto/core.sock"),
                "default socket path should live under ~/.concerto/, got {s}"
            );
        }
    }
}
