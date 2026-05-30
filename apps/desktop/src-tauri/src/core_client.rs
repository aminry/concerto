//! Thin gRPC client wrapper used by the Tauri command dispatcher.
//!
//! Phase 2 (Task 24) replaces the Task 14 "fresh client per call" with
//! a lazily-initialised, process-wide gRPC channel via [`OnceCell`].
//! On any RPC error the cell is reset so the next call dials afresh —
//! this is the cheapest possible reconnect strategy that still avoids
//! re-handshaking on every keystroke. Sophisticated exponential
//! backoff arrives with the V1.0 transport switch (Iroh).
//!
//! - UDS-only. Iroh transport is V1.0 (`design/15 §3.2`,
//!   `design/12 §3.3`).
//! - The UDS connect path mirrors the pattern locked by `tasks/13`
//!   (see `crates/core/tests/grpc_runtime.rs::connect_client`).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;
use tokio::sync::OnceCell;
use tonic::transport::{Channel, Endpoint, Uri};

/// Process-wide override for the socket path. Set once at startup by
/// embedded mode (`embedded::start`). When unset, `default_socket_path`
/// falls back to `<HOME>/.concerto/core.sock`.
static SOCKET_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Set the socket path that embedded Core binds to. Idempotent — the
/// first call wins (Core is booted exactly once per process). Called by
/// `embedded::start` (Task 4); unused in the lean non-embedded build,
/// hence the `dead_code` allowance.
#[allow(dead_code)]
pub fn set_socket_override(path: PathBuf) {
    let _ = SOCKET_OVERRIDE.set(path);
}

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
    if let Some(p) = SOCKET_OVERRIDE.get() {
        return Some(p.clone());
    }
    let home = home::home_dir()?;
    Some(home.join(".concerto").join("core.sock"))
}

/// Process-wide persistent channel slot. Populated lazily on first
/// successful dial. Callers MUST reset via [`reset_channel`] on any
/// RPC error so the next call re-dials.
///
/// `std::sync::Mutex<OnceCell<Channel>>` rather than a plain
/// `OnceCell` so the reset path can swap the cell out atomically.
/// The outer mutex is uncontended on the happy path (we only hold it
/// while replacing or initialising the cell).
static CHANNEL: Mutex<Option<Channel>> = Mutex::new(None);

/// Static OnceCell used during initial population. The mutex-guarded
/// path above takes precedence; this is the slow-path init guard.
static CHANNEL_INIT: OnceCell<()> = OnceCell::const_new();

/// Acquire a persistent gRPC channel, lazily dialing on first use.
///
/// On the happy path this is two `Mutex::lock` operations and a
/// `Channel::clone` — cheap enough to call once per RPC. The Tonic
/// `Channel` is itself a smart-pointer over the inner connection
/// pool, so cloning is the canonical way to share it across tasks.
pub async fn get_or_connect(socket_path: &Path) -> Result<Channel, CoreClientError> {
    // Happy path: channel cached.
    if let Some(ch) = CHANNEL.lock().expect("channel mutex poisoned").clone() {
        return Ok(ch);
    }

    // Cold-start path: dial under the OnceCell so concurrent callers
    // share one dial. After the first successful dial the OnceCell stays
    // initialised forever (stable Rust has no `OnceCell::take`), which
    // means subsequent `reset_channel` callers can leave the Mutex empty
    // even though the cell is "initialised". The cell-already-set branch
    // below dials directly to keep RPCs working after a reset.
    let path = socket_path.to_path_buf();
    let dial_path = path.clone();
    let init_result = CHANNEL_INIT
        .get_or_try_init(|| async move {
            let ch = dial_uds(&dial_path).await?;
            *CHANNEL.lock().expect("channel mutex poisoned") = Some(ch);
            Ok::<_, CoreClientError>(())
        })
        .await;
    init_result?;

    if let Some(ch) = CHANNEL.lock().expect("channel mutex poisoned").clone() {
        return Ok(ch);
    }

    // OnceCell was already set (so the closure above was skipped) but
    // the Mutex is empty because a prior `reset_channel` cleared it.
    // Re-dial and install. Multiple concurrent callers may race here;
    // last-write-wins is fine — tonic Channels are cheap clones.
    let ch = dial_uds(&path).await?;
    *CHANNEL.lock().expect("channel mutex poisoned") = Some(ch.clone());
    Ok(ch)
}

/// Drop the cached channel so the next [`get_or_connect`] dials fresh.
///
/// Call this on any RPC error — V0.1 makes no attempt to distinguish
/// transient from terminal failures; a re-dial is the simplest
/// correct response. The OnceCell is re-armed via internal reset.
pub fn reset_channel() {
    *CHANNEL.lock().expect("channel mutex poisoned") = None;
    // The OnceCell cannot be reset on stable, so future init attempts
    // re-populate the Mutex directly via `dial_uds`; see the fallback
    // path in [`get_or_connect`].
}

/// Build a Tonic `Channel` that routes every connection to a UDS at
/// `socket_path`. Returns an error if the socket can't be opened — the
/// caller maps that into `CoreClientError::Transport`.
async fn dial_uds(socket_path: &Path) -> Result<Channel, CoreClientError> {
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

/// Back-compat helper retained for the connector unit tests.
/// Equivalent to [`dial_uds`].
#[cfg(test)]
pub async fn connect_uds(socket_path: &Path) -> Result<Channel, CoreClientError> {
    dial_uds(socket_path).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::net::UnixListener;

    #[test]
    fn rpc_error_serializes_with_kind_and_message_fields() {
        // The renderer's `errorMessage` (apps/desktop/src/api/client.ts)
        // reads the `message` field of this serialized error to show the
        // real failure text. The adjacent tag (`kind`/`message`) is a wire
        // contract between the shell and the renderer: if it regresses to a
        // flat `{ Rpc: "..." }` map, the renderer surfaces a bare,
        // undebuggable "Rpc" again (the original "Couldn't start session:
        // Rpc" bug). Lock the shape here.
        let err = CoreClientError::Rpc("Internal: spawn agent-host: io".into());
        let v = serde_json::to_value(&err).expect("serialize");
        assert_eq!(v["kind"], "Rpc");
        assert_eq!(v["message"], "Internal: spawn agent-host: io");
    }

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

    // Both the default-path and override behaviors live in one test
    // because `SOCKET_OVERRIDE` is a process-global `OnceLock`: once set
    // it stays set for the rest of the process. Splitting these into two
    // tests would race under libtest's parallel runner (whichever set the
    // override first would break the other). A single ordered test owns
    // the global — assert the default first, then install the override.
    #[test]
    fn default_socket_path_defaults_then_honors_override() {
        // Before any override: falls back to ~/.concerto/core.sock (only
        // asserted when $HOME resolves — None is acceptable on bare envs).
        if let Some(p) = default_socket_path() {
            let s = p.to_string_lossy();
            assert!(
                s.ends_with("/.concerto/core.sock"),
                "default socket path should live under ~/.concerto/, got {s}"
            );
        }

        // After installing an override it takes precedence everywhere.
        let overridden = std::path::PathBuf::from("/tmp/concerto-test/core.sock");
        set_socket_override(overridden.clone());
        assert_eq!(default_socket_path(), Some(overridden));
    }
}
