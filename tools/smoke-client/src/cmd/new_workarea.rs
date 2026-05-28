//! `smoke-client new-workarea --workspace-id <id>` — calls
//! `Workareas.CreateWorkarea`. Composer-name allocation and on-disk
//! worktree layout happen server-side; the smoke script checks the
//! filesystem afterwards.

use std::path::Path;

use concerto_proto::v1::workareas_client::WorkareasClient;
use concerto_proto::v1::CreateWorkareaRequest;

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

pub async fn run(socket: &Path, workspace_id: &str) -> Result<(), String> {
    if workspace_id.is_empty() {
        return Err("new-workarea: --workspace-id must be non-empty".to_string());
    }

    let channel = connect_to_socket(socket).await?;
    let mut client = WorkareasClient::new(channel);

    let resp = tokio::time::timeout(
        RPC_TIMEOUT,
        client.create_workarea(CreateWorkareaRequest {
            workspace_id: workspace_id.to_string(),
            permission_mode: None,
        }),
    )
    .await
    .map_err(|_| format!("CreateWorkarea timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|status| format!("CreateWorkarea rpc error: {status}"))?;

    let wa = resp.into_inner();
    println!("{}", wa.id);
    Ok(())
}
