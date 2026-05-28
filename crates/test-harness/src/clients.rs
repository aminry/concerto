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

/// `ProjectsClient` over a UDS-backed Tonic channel (Task 24).
pub type ProjectsClient = concerto_proto::v1::projects_client::ProjectsClient<Channel>;

/// `RepositoriesClient` over a UDS-backed Tonic channel (Task 18).
pub type RepositoriesClient = concerto_proto::v1::repositories_client::RepositoriesClient<Channel>;

/// `WorkspacesClient` over a UDS-backed Tonic channel (Task 19).
pub type WorkspacesClient = concerto_proto::v1::workspaces_client::WorkspacesClient<Channel>;

/// `WorkareasClient` over a UDS-backed Tonic channel (Task 20).
pub type WorkareasClient = concerto_proto::v1::workareas_client::WorkareasClient<Channel>;

/// `SessionsClient` over a UDS-backed Tonic channel (Task 23).
pub type SessionsClient = concerto_proto::v1::sessions_client::SessionsClient<Channel>;

/// `StreamsClient` over a UDS-backed Tonic channel (Task 23).
pub type StreamsClient = concerto_proto::v1::streams_client::StreamsClient<Channel>;

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

/// Build a Tonic `ProjectsClient` dialed to `socket_path` (Task 24).
pub async fn projects_client(socket_path: PathBuf) -> Result<ProjectsClient, ClientError> {
    let channel = uds_channel(socket_path).await?;
    Ok(ProjectsClient::new(channel))
}

/// Build a Tonic `RepositoriesClient` dialed to `socket_path` (Task 18).
pub async fn repositories_client(socket_path: PathBuf) -> Result<RepositoriesClient, ClientError> {
    let channel = uds_channel(socket_path).await?;
    Ok(RepositoriesClient::new(channel))
}

/// Build a Tonic `WorkspacesClient` dialed to `socket_path` (Task 19).
pub async fn workspaces_client(socket_path: PathBuf) -> Result<WorkspacesClient, ClientError> {
    let channel = uds_channel(socket_path).await?;
    Ok(WorkspacesClient::new(channel))
}

/// Build a Tonic `WorkareasClient` dialed to `socket_path` (Task 20).
pub async fn workareas_client(socket_path: PathBuf) -> Result<WorkareasClient, ClientError> {
    let channel = uds_channel(socket_path).await?;
    Ok(WorkareasClient::new(channel))
}

/// Build a Tonic `SessionsClient` dialed to `socket_path` (Task 23).
pub async fn sessions_client(socket_path: PathBuf) -> Result<SessionsClient, ClientError> {
    let channel = uds_channel(socket_path).await?;
    Ok(SessionsClient::new(channel))
}

/// Build a Tonic `StreamsClient` dialed to `socket_path` (Task 23).
pub async fn streams_client(socket_path: PathBuf) -> Result<StreamsClient, ClientError> {
    let channel = uds_channel(socket_path).await?;
    Ok(StreamsClient::new(channel))
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
