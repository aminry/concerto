//! `smoke-client clone --repo-id <id>` — calls `Repositories.Clone`
//! and drains the progress stream until the server closes it.
//!
//! On success no output goes to stdout. The exit code is the signal
//! the smoke script uses (`|| fail "clone"` in the bash block).

use std::path::Path;

use concerto_proto::v1::repositories_client::RepositoriesClient;
use concerto_proto::v1::CloneRequest;
use futures::StreamExt;
use tonic::transport::Channel;

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

pub async fn run(socket: &Path, repo_id: &str) -> Result<(), String> {
    if repo_id.is_empty() {
        return Err("clone: --repo-id must be non-empty".to_string());
    }

    let channel = connect_to_socket(socket).await?;
    let mut client = RepositoriesClient::new(channel);

    // The opening RPC is bounded by the standard 30 s deadline; the
    // streaming drain that follows is also bounded by the same
    // budget because a small bare-repo clone of the smoke fixture
    // should complete in milliseconds.
    let drain_fut = async {
        // UFCS: the inherent `Clone::clone` shadows the gRPC `clone` method on
        // `RepositoriesClient<Channel>` (see Task 18 Handoff Notes; the
        // integration test uses the same pattern).
        let resp = RepositoriesClient::<Channel>::clone(
            &mut client,
            CloneRequest {
                repository_id: repo_id.to_string(),
            },
        )
        .await
        .map_err(|status| format!("Clone rpc error: {status}"))?;
        let mut stream = resp.into_inner();
        while let Some(item) = stream.next().await {
            let _progress = item.map_err(|status| format!("Clone stream error: {status}"))?;
            // Phase 2 smoke gate does not assert on progress shape;
            // it only requires the stream to close successfully.
        }
        Ok::<(), String>(())
    };

    tokio::time::timeout(RPC_TIMEOUT, drain_fut)
        .await
        .map_err(|_| format!("Clone stream timed out after {RPC_TIMEOUT:?}"))??;

    Ok(())
}
