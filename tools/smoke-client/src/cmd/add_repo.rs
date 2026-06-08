//! `smoke-client add-repo --url <url> [--name <s>] [--clone-strategy <s>]
//! [--with-sparse]` — calls `Repositories.AddRepository`. Prints the new
//! repository id to stdout. Repositories are a GLOBAL registry after the
//! Project→Workspace collapse (no `--project-id`), so `name` is globally
//! unique — pass a distinct `--name` per repo (default `"smoke-repo"`).

use std::path::Path;

use concerto_proto::v1::repositories_client::RepositoriesClient;
use concerto_proto::v1::AddRepoRequest;

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

pub async fn run(
    socket: &Path,
    url: &str,
    name: &str,
    clone_strategy: &str,
    with_sparse: bool,
) -> Result<(), String> {
    if url.is_empty() {
        return Err("add-repo: --url must be non-empty".to_string());
    }
    let name = if name.is_empty() { "smoke-repo" } else { name };

    let channel = connect_to_socket(socket).await?;
    let mut client = RepositoriesClient::new(channel);

    let resp = tokio::time::timeout(
        RPC_TIMEOUT,
        client.add_repository(AddRepoRequest {
            name: name.to_string(),
            url: url.to_string(),
            default_branch: "main".to_string(),
            // Task 301 added clone_strategy/with_sparse. Empty
            // clone_strategy → Full (preserves the existing
            // `repo-clone` smoke check). Task 302's
            // `sparse-cone-clone` check passes `blobless` + `--with-sparse`
            // so the worktree lands empty for the cone-set step.
            clone_strategy: clone_strategy.to_string(),
            with_sparse,
            // Empty `local_path` → clone the `url` (the adopt-in-place path
            // is not exercised by the smoke gate).
            local_path: String::new(),
        }),
    )
    .await
    .map_err(|_| format!("AddRepository timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|status| format!("AddRepository rpc error: {status}"))?;

    let repo = resp.into_inner();
    println!("{}", repo.id);
    Ok(())
}
