//! `smoke-client send-message --session-id <id> --text <string>` — calls
//! `Sessions.SendMessage`. The payload is the message text plus a trailing
//! newline, encoded as raw UTF-8 bytes and forwarded verbatim to the
//! agent's stdin. Prints `sent N bytes`.

use std::path::Path;

use concerto_proto::v1::sessions_client::SessionsClient;
use concerto_proto::v1::SendMessageRequest;

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

pub async fn run(socket: &Path, session_id: &str, text: &str) -> Result<(), String> {
    if session_id.is_empty() {
        return Err("send-message: --session-id must be non-empty".to_string());
    }

    // Carriage return = "Enter": interactive agent TUIs submit a turn on
    // CR, not LF (a bare \n is a newline inside the prompt editor). Matches
    // the desktop composer.
    let payload: Vec<u8> = format!("{text}\r").into_bytes();
    let n = payload.len();

    let channel = connect_to_socket(socket).await?;
    let mut client = SessionsClient::new(channel);

    let _ = tokio::time::timeout(
        RPC_TIMEOUT,
        client.send_message(SendMessageRequest {
            session_id: session_id.to_string(),
            payload,
        }),
    )
    .await
    .map_err(|_| format!("SendMessage timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|status| format!("SendMessage rpc error: {status}"))?;

    println!("sent {n} bytes");
    Ok(())
}
