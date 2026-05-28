//! `smoke-client start-session --workarea-id <id> --agent-kind echo` —
//! calls `Sessions.CreateSession`. Per Task 22 + 23 the supervisor
//! treats `"echo"` as a test-only agent kind that spawns
//! `concerto-agent-host --agent-bin /bin/echo --agent-arg "hello"`.
//! Prints the server-assigned session id (UUIDv7).

use std::path::Path;

use concerto_proto::v1::sessions_client::SessionsClient;
use concerto_proto::v1::CreateSessionRequest;

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

pub async fn run(socket: &Path, workarea_id: &str, agent_kind: &str) -> Result<(), String> {
    if workarea_id.is_empty() {
        return Err("start-session: --workarea-id must be non-empty".to_string());
    }
    if agent_kind.is_empty() {
        return Err("start-session: --agent-kind must be non-empty".to_string());
    }

    let channel = connect_to_socket(socket).await?;
    let mut client = SessionsClient::new(channel);

    let resp = tokio::time::timeout(
        RPC_TIMEOUT,
        client.create_session(CreateSessionRequest {
            workarea_id: workarea_id.to_string(),
            agent_kind: agent_kind.to_string(),
            model: None,
            permission_mode: None,
        }),
    )
    .await
    .map_err(|_| format!("CreateSession timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|status| format!("CreateSession rpc error: {status}"))?;

    let session = resp.into_inner();
    println!("{}", session.id);
    Ok(())
}
