//! Self-contained UDS gRPC client for the `concerto` CLI.
//!
//! This module is the single source of truth for:
//!
//! 1. **Default socket-path derivation.** [`default_socket_path`] resolves
//!    the same path the Core binds and the desktop reads
//!    (`apps/desktop/src-tauri/src/core_client.rs::default_socket_path`):
//!    `$CONCERTO_SOCKET` → `<HOME>/.concerto/core.sock`. The CLI's global
//!    `--socket` flag overrides both.
//! 2. **Dialing the Core over UDS.** [`connect`] builds a Tonic [`Channel`]
//!    using the placeholder-URI + `connect_with_connector` pattern locked in
//!    `tasks/13` and reused by `tools/smoke-client` and the test-harness.
//!    The approach is *copied*, not shared — per Task 109's scope we do not
//!    take a dependency on `concerto-smoke-client`.
//!
//! Tasks 111 (`concerto backup`) and 713 (`concerto pair`) reuse this module
//! within `crates/cli`. The reusable entry point is:
//!
//! ```ignore
//! pub async fn connect(socket: &Path) -> Result<Channel, ClientError>;
//! ```
//!
//! Build the typed service clients on top of the returned `Channel`, e.g.
//! `RuntimeClient::new(client::connect(&socket).await?)`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tonic::transport::Channel;

// The UDS dial pulls in the Unix-only socket type and the Tonic connector
// glue; it only compiles (and is only meaningful) on Unix. On other platforms
// the CLI still builds — `connect` returns a clear `ClientError::Unsupported`.
#[cfg(unix)]
use hyper_util::rt::TokioIo;
#[cfg(unix)]
use tokio::net::UnixStream;
#[cfg(unix)]
use tonic::transport::{Endpoint, Uri};

/// Environment variable that overrides the default socket path. Mirrors the
/// desktop's `set_socket_override` convention but in env-var form so the CLI
/// can be pointed at a non-default Core without a flag. The global
/// `--socket` flag takes precedence over this; this takes precedence over
/// the `<HOME>/.concerto/core.sock` default.
pub const SOCKET_ENV: &str = "CONCERTO_SOCKET";

/// Connect timeout for the UDS dial. Matches the 5 s budget the smoke client
/// and test-harness use so a wedged Core can't hang the CLI indefinitely.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Errors surfaced while resolving or dialing the Core socket.
///
/// Variants are constructed by platform-conditional code paths — the UDS-dial
/// variants (`EndpointInit`/`ConnectTimeout`/`Connect`) only on `#[cfg(unix)]`,
/// and `Unsupported` only on `#[cfg(not(unix))]` — so `dead_code` is allowed at
/// the enum level to keep the public error surface identical across platforms
/// without per-variant cfg attributes.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum ClientError {
    /// `$HOME` could not be resolved and no explicit socket was given, so
    /// the default `<HOME>/.concerto/core.sock` path can't be derived.
    #[error(
        "could not determine the default Concerto socket: $HOME is unset. \
         Pass --socket <path> or set ${env} to point at the Core's UDS socket.",
        env = SOCKET_ENV
    )]
    NoHome,
    /// The placeholder endpoint URI was rejected. Should never happen — the
    /// URI is a compile-time constant.
    #[error("endpoint init: {0}")]
    EndpointInit(String),
    /// The dial exceeded [`CONNECT_TIMEOUT`].
    #[error(
        "timed out after {timeout:?} connecting to the Concerto Core at {socket}. \
         Is the Core running?",
        timeout = CONNECT_TIMEOUT,
    )]
    ConnectTimeout { socket: PathBuf },
    /// The dial failed — typically because the Core is not running (the
    /// socket file is absent) or the path is wrong. The message names the
    /// socket path that was tried.
    #[error(
        "could not connect to the Concerto Core at {socket}: {source}. \
         Is the Core running? (override with --socket <path> or ${env})",
        env = SOCKET_ENV,
    )]
    Connect {
        socket: PathBuf,
        #[source]
        source: tonic::transport::Error,
    },
    /// The current platform has no Unix-domain-socket transport. In Phase 1 the
    /// `concerto` CLI dials the Core exclusively over a local UDS; remote /
    /// other transports arrive in a later phase. This variant lets the CLI
    /// build on non-Unix targets (e.g. the Windows CI lane) and fail at runtime
    /// with a clear message instead of failing to compile.
    // Only constructed by the `#[cfg(not(unix))]` `connect`; on Unix the
    // variant is intentionally never built but must still exist so the public
    // `ClientError` surface is identical across platforms (dead_code allowed at
    // the enum level above).
    #[error(
        "the `concerto` CLI uses a local Unix-domain socket, which is not \
         available on this platform; remote transport support arrives in a \
         later phase"
    )]
    Unsupported,
}

/// Resolve the default UDS path the Core binds at, honoring the
/// `$CONCERTO_SOCKET` override.
///
/// Resolution order (the global `--socket` flag is applied by the caller
/// *before* this is consulted):
///
/// 1. `$CONCERTO_SOCKET` if set and non-empty.
/// 2. `<HOME>/.concerto/core.sock`.
///
/// Step 2 matches the layout locked by `tasks/11` (`RuntimeConfig`),
/// consumed by `tasks/13` (`ApiServerConfig::socket_path`), and read by the
/// desktop (`core_client::default_socket_path`). This is the single source
/// of truth for the CLI — there is no second hardcoded path.
///
/// Returns [`ClientError::NoHome`] only when `$CONCERTO_SOCKET` is unset and
/// `$HOME` cannot be resolved.
pub fn default_socket_path() -> Result<PathBuf, ClientError> {
    if let Some(env) = std::env::var_os(SOCKET_ENV) {
        if !env.is_empty() {
            return Ok(PathBuf::from(env));
        }
    }
    let home = home::home_dir().ok_or(ClientError::NoHome)?;
    Ok(home.join(".concerto").join("core.sock"))
}

/// Resolve the socket path the CLI should dial, applying the flag → env →
/// default precedence in one place.
///
/// `flag` is the value of the global `--socket` argument (if the user passed
/// it). When `None`, falls back to [`default_socket_path`].
pub fn resolve_socket_path(flag: Option<PathBuf>) -> Result<PathBuf, ClientError> {
    match flag {
        Some(path) => Ok(path),
        None => default_socket_path(),
    }
}

/// Dial the Core over UDS and return a Tonic [`Channel`].
///
/// This is the reusable connect entry point Tasks 111/713 build their
/// service clients on. Wrap the returned channel in the generated client,
/// e.g. `RuntimeClient::new(connect(&socket).await?)`.
///
/// The placeholder `http://[::1]:50051` URI is required by Tonic's
/// HTTP-shaped builder but never dialed — `connect_with_connector`
/// short-circuits every connection to a `UnixStream::connect`.
///
/// On failure the error names the socket path that was tried and hints that
/// the Core may not be running.
#[cfg(unix)]
pub async fn connect(socket: &Path) -> Result<Channel, ClientError> {
    let owned: PathBuf = socket.to_path_buf();
    let endpoint = Endpoint::try_from("http://[::1]:50051")
        .map_err(|e| ClientError::EndpointInit(e.to_string()))?
        .connect_timeout(CONNECT_TIMEOUT);

    let dial = endpoint.connect_with_connector(tower::service_fn(move |_: Uri| {
        let p = owned.clone();
        async move {
            let stream = UnixStream::connect(&p).await?;
            Ok::<_, std::io::Error>(TokioIo::new(stream))
        }
    }));

    match tokio::time::timeout(CONNECT_TIMEOUT, dial).await {
        Err(_) => Err(ClientError::ConnectTimeout {
            socket: socket.to_path_buf(),
        }),
        Ok(Err(source)) => Err(ClientError::Connect {
            socket: socket.to_path_buf(),
            source,
        }),
        Ok(Ok(channel)) => Ok(channel),
    }
}

/// Non-Unix fallback: the CLI has no UDS transport on this platform.
///
/// Phase 1 dials the Core exclusively over a local Unix-domain socket, which
/// only exists on Unix. This keeps the crate compiling on non-Unix targets
/// (e.g. the Windows CI lane) with an identical public signature; it fails at
/// runtime with [`ClientError::Unsupported`] rather than at compile time.
#[cfg(not(unix))]
pub async fn connect(_socket: &Path) -> Result<Channel, ClientError> {
    Err(ClientError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    // `$CONCERTO_SOCKET` and `$HOME` are process-global; this single test
    // owns the ordering so libtest's parallel runner can't race two tests
    // mutating the same env vars. SAFETY: std::env::set_var/remove_var are
    // unsafe in edition 2024+; on 2021 they are safe fns, and we hold the
    // sole reference to these vars for the duration of the test.
    #[test]
    fn socket_resolution_precedence() {
        // --socket flag wins over everything.
        let flag = PathBuf::from("/tmp/flag.sock");
        assert_eq!(
            resolve_socket_path(Some(flag.clone())).unwrap(),
            flag,
            "explicit --socket must take precedence"
        );

        // $CONCERTO_SOCKET wins over the HOME default.
        std::env::set_var(SOCKET_ENV, "/tmp/env.sock");
        assert_eq!(
            default_socket_path().unwrap(),
            PathBuf::from("/tmp/env.sock"),
            "$CONCERTO_SOCKET must override the HOME default"
        );

        // Empty $CONCERTO_SOCKET is ignored — falls through to the default.
        std::env::set_var(SOCKET_ENV, "");
        if let Some(home) = home::home_dir() {
            let expected = home.join(".concerto").join("core.sock");
            assert_eq!(
                default_socket_path().unwrap(),
                expected,
                "empty $CONCERTO_SOCKET must fall through to ~/.concerto/core.sock"
            );
        }

        std::env::remove_var(SOCKET_ENV);
    }
}
