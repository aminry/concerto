//! `smoke-client stop-session --session-id <id>` — calls
//! `Sessions.StopSession`. The smoke gate uses `"user_request"` as the
//! reason string per `design/04 §6.x`'s conventional vocabulary.

use std::path::Path;

use concerto_proto::v1::sessions_client::SessionsClient;
use concerto_proto::v1::StopSessionRequest;

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

pub async fn run(socket: &Path, session_id: &str) -> Result<(), String> {
    if session_id.is_empty() {
        return Err("stop-session: --session-id must be non-empty".to_string());
    }

    let channel = connect_to_socket(socket).await?;
    let mut client = SessionsClient::new(channel);

    let _ = tokio::time::timeout(
        RPC_TIMEOUT,
        client.stop_session(StopSessionRequest {
            session_id: session_id.to_string(),
            reason: "user_request".to_string(),
        }),
    )
    .await
    .map_err(|_| format!("StopSession timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|status| format!("StopSession rpc error: {status}"))?;

    Ok(())
}
