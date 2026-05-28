//! Shared UDS dial helper for every smoke-client subcommand.
//!
//! The pattern is the same one locked in `tasks/13-grpc-uds-server.md`
//! and reused by `crates/test-harness/src/clients.rs`: a placeholder
//! HTTP URI feeds Tonic's `Endpoint`, and `connect_with_connector`
//! overrides every dial with a `UnixStream::connect` wrapped in
//! `hyper_util::rt::TokioIo`.

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};

/// Connect timeout matches the Phase 1 budget (`tools/smoke-client`
/// pre-Task-27) so a wedged Core can't hang the smoke gate.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Build a Tonic [`Channel`] dialed to `socket_path` over UDS.
///
/// The placeholder `http://[::1]:50051` URI is required by Tonic's
/// HTTP-shaped builder but never actually used — `connect_with_connector`
/// short-circuits every dial.
pub async fn connect_to_socket(socket_path: &Path) -> Result<Channel, String> {
    let owned: PathBuf = socket_path.to_path_buf();
    let endpoint = Endpoint::try_from("http://[::1]:50051")
        .map_err(|e| format!("endpoint init: {e}"))?
        .connect_timeout(CONNECT_TIMEOUT);

    let channel = tokio::time::timeout(
        CONNECT_TIMEOUT,
        endpoint.connect_with_connector(tower::service_fn(move |_: Uri| {
            let p = owned.clone();
            async move {
                let stream = UnixStream::connect(&p).await?;
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        })),
    )
    .await
    .map_err(|_| format!("connect timed out after {CONNECT_TIMEOUT:?}"))?
    .map_err(|e| format!("connect: {e}"))?;

    Ok(channel)
}
