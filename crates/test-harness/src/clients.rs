//! Tonic client builders for the harness.
//!
//! Single canonical pattern, copied from
//! `crates/core/tests/grpc_runtime.rs::connect_client` and
//! `tools/smoke-client/src/main.rs::connect`: a placeholder HTTP URI
//! feeds Tonic's `Endpoint`, and `connect_with_connector` overrides
//! every dial with a `UnixStream::connect` wrapped in
//! `hyper_util::rt::TokioIo`.

use std::path::PathBuf;
use std::time::Duration;

use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;
use tonic::transport::{Channel, Endpoint, Uri};

/// `RuntimeClient` over a UDS-backed Tonic channel.
pub type RuntimeClient = concerto_proto::v1::runtime_client::RuntimeClient<Channel>;

/// `RepositoriesClient` over a UDS-backed Tonic channel (Task 18).
pub type RepositoriesClient = concerto_proto::v1::repositories_client::RepositoriesClient<Channel>;

/// Connect timeout used by every client builder. 5 s matches the smoke
/// client's budget; integration tests time out their own RPC calls on
/// top of this.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Errors emitted by the client builders.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// `Endpoint::try_from` rejected the placeholder URI. Should never
    /// happen in practice — surfaced for completeness.
    #[error("endpoint init: {0}")]
    EndpointInit(String),
    /// The actual dial failed (socket not present, permission denied,
    /// etc.).
    #[error("connect: {0}")]
    Connect(tonic::transport::Error),
    /// The connect attempt exceeded [`CONNECT_TIMEOUT`].
    #[error("connect timed out after {0:?}")]
    ConnectTimeout(Duration),
}

/// Build a Tonic `RuntimeClient` dialed to `socket_path`.
pub async fn runtime_client(socket_path: PathBuf) -> Result<RuntimeClient, ClientError> {
    let channel = uds_channel(socket_path).await?;
    Ok(RuntimeClient::new(channel))
}

/// Build a Tonic `RepositoriesClient` dialed to `socket_path` (Task 18).
pub async fn repositories_client(socket_path: PathBuf) -> Result<RepositoriesClient, ClientError> {
    let channel = uds_channel(socket_path).await?;
    Ok(RepositoriesClient::new(channel))
}

async fn uds_channel(socket_path: PathBuf) -> Result<Channel, ClientError> {
    let endpoint = Endpoint::try_from("http://[::1]:50051")
        .map_err(|e| ClientError::EndpointInit(e.to_string()))?
        .connect_timeout(CONNECT_TIMEOUT);

    let channel = tokio::time::timeout(
        CONNECT_TIMEOUT,
        endpoint.connect_with_connector(tower::service_fn(move |_: Uri| {
            let p = socket_path.clone();
            async move {
                let stream = UnixStream::connect(&p).await?;
                Ok::<_, std::io::Error>(TokioIo::new(stream))
            }
        })),
    )
    .await
    .map_err(|_| ClientError::ConnectTimeout(CONNECT_TIMEOUT))?
    .map_err(ClientError::Connect)?;

    Ok(channel)
}
