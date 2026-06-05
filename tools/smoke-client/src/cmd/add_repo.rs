//! `smoke-client add-repo --project-id <id> --url <url>` — calls
//! `Repositories.AddRepository`. The repo is named `"smoke-repo"`
//! (a fixed name in V0.1 since the smoke gate only adds one).
//! Prints the new repository id to stdout.

use std::path::Path;

use concerto_proto::v1::repositories_client::RepositoriesClient;
use concerto_proto::v1::AddRepoRequest;

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

pub async fn run(socket: &Path, project_id: &str, url: &str) -> Result<(), String> {
    if project_id.is_empty() {
        return Err("add-repo: --project-id must be non-empty".to_string());
    }
    if url.is_empty() {
        return Err("add-repo: --url must be non-empty".to_string());
    }

    let channel = connect_to_socket(socket).await?;
    let mut client = RepositoriesClient::new(channel);

    let resp = tokio::time::timeout(
        RPC_TIMEOUT,
        client.add_repository(AddRepoRequest {
            project_id: project_id.to_string(),
            name: "smoke-repo".to_string(),
            url: url.to_string(),
            default_branch: "main".to_string(),
            // Task 301 added clone_strategy/with_sparse. Leaving them at
            // their defaults (empty → Full) preserves the existing
            // `project-repo-clone` smoke check's full-clone behaviour.
            ..Default::default()
        }),
    )
    .await
    .map_err(|_| format!("AddRepository timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|status| format!("AddRepository rpc error: {status}"))?;

    let repo = resp.into_inner();
    println!("{}", repo.id);
    Ok(())
}
