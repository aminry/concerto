//! `smoke-client new-workspace --name <s> --repo-id <id>` — calls
//! `Workspaces.CreateWorkspace`. Workspaces own a 1..N repo set drawn from
//! the global registry; the smoke gate attaches exactly one repo with an
//! empty cone spec (seeded from the repo's cone defaults, D4).

use std::path::Path;

use concerto_proto::v1::workspaces_client::WorkspacesClient;
use concerto_proto::v1::{CreateWorkspaceRequest, WorkspaceRepoSpec};

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

pub async fn run(socket: &Path, name: &str, repo_id: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("new-workspace: --name must be non-empty".to_string());
    }
    if repo_id.is_empty() {
        return Err("new-workspace: --repo-id must be non-empty".to_string());
    }

    let channel = connect_to_socket(socket).await?;
    let mut client = WorkspacesClient::new(channel);

    let resp = tokio::time::timeout(
        RPC_TIMEOUT,
        client.create_workspace(CreateWorkspaceRequest {
            name: name.to_string(),
            repos: vec![WorkspaceRepoSpec {
                repository_id: repo_id.to_string(),
                sparse_cones: vec![],
            }],
            permission_mode: None,
            description: None,
            icon: None,
        }),
    )
    .await
    .map_err(|_| format!("CreateWorkspace timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|status| format!("CreateWorkspace rpc error: {status}"))?;

    let ws = resp.into_inner();
    println!("{}", ws.id);
    Ok(())
}
