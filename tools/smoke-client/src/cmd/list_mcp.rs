//! `smoke-client list-mcp [--scope <s>] [--repository-id <s>]`
//!
//! Calls `Sessions.ListMcpServers` (Task 35) and prints one server
//! `name` per line. The smoke gate v3 block plants a fake
//! `~/.claude/mcp.json` and then asserts the entry shows up in the
//! list — round-trip on the read-only MCP surface.

use std::path::Path;

use concerto_proto::v1::sessions_client::SessionsClient;
use concerto_proto::v1::McpScopeRequest;

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

pub async fn run(
    socket: &Path,
    scope: Option<&str>,
    repository_id: Option<&str>,
) -> Result<(), String> {
    let channel = connect_to_socket(socket).await?;
    let mut client = SessionsClient::new(channel);

    let resp = tokio::time::timeout(
        RPC_TIMEOUT,
        client.list_mcp_servers(McpScopeRequest {
            scope: scope.map(|s| s.to_string()),
            repository_id: repository_id.map(|s| s.to_string()),
        }),
    )
    .await
    .map_err(|_| format!("ListMcpServers timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|status| format!("ListMcpServers rpc error: {status}"))?;

    for server in resp.into_inner().servers {
        println!("{}", server.name);
    }
    Ok(())
}
