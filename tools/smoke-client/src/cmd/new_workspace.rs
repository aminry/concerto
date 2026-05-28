//! `smoke-client new-workspace --project-id <id> --name <s> --repo-id <id>`
//! — calls `Workspaces.CreateWorkspace`. V0.1 enforces single-repo
//! workspaces, so we always pass exactly one repo id.

use std::path::Path;

use concerto_proto::v1::workspaces_client::WorkspacesClient;
use concerto_proto::v1::CreateWorkspaceRequest;

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

pub async fn run(socket: &Path, project_id: &str, name: &str, repo_id: &str) -> Result<(), String> {
    if project_id.is_empty() {
        return Err("new-workspace: --project-id must be non-empty".to_string());
    }
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
            project_id: project_id.to_string(),
            name: name.to_string(),
            repository_ids: vec![repo_id.to_string()],
            permission_mode: None,
            description: None,
        }),
    )
    .await
    .map_err(|_| format!("CreateWorkspace timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|status| format!("CreateWorkspace rpc error: {status}"))?;

    let ws = resp.into_inner();
    println!("{}", ws.id);
    Ok(())
}
