//! `smoke-client set-cones --workarea <id> --repo <id> --cone <path> [--cone <path> …]`
//! — calls `Repositories.SetCones` (Task 302). Applies the per-(workarea,
//! repo) sparse cone and prints one applied cone path per line.
//!
//! Drives the `sparse-cone-clone` smoke capability: after a blobless+sparse
//! clone leaves an empty worktree, this sets the cone to (e.g.) `a/` and the
//! Core applies cone-mode + `--sparse-index` + persists `sparse_cones_json`.

use std::path::Path;

use concerto_proto::v1::repositories_client::RepositoriesClient;
use concerto_proto::v1::SetConesRequest;

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

pub async fn run(
    socket: &Path,
    workarea: &str,
    repo: &str,
    cones: &[String],
) -> Result<(), String> {
    if workarea.is_empty() {
        return Err("set-cones: --workarea must be non-empty".to_string());
    }
    if repo.is_empty() {
        return Err("set-cones: --repo must be non-empty".to_string());
    }

    let channel = connect_to_socket(socket).await?;
    let mut client = RepositoriesClient::new(channel);

    let resp = tokio::time::timeout(
        RPC_TIMEOUT,
        client.set_cones(SetConesRequest {
            workarea_id: workarea.to_string(),
            repository_id: repo.to_string(),
            cone_paths: cones.to_vec(),
        }),
    )
    .await
    .map_err(|_| format!("SetCones timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|status| format!("SetCones rpc error: {status}"))?;

    for path in resp.into_inner().cone_paths {
        println!("{path}");
    }
    Ok(())
}
