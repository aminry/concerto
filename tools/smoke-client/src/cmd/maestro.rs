//! `smoke-client maestro-*` — drive the live `Maestro.*` gRPC surface (the
//! exact contract the desktop chat's `callRpc("Maestro.*")` bindings hit) end
//! to end against a running Core. This is the backend half of the chat E2E
//! harness (`tools/maestro-chat-e2e.sh`): send a chat turn, read the persisted
//! history + state + digest, and watch the live `maestro.events` stream that
//! carries assistant replies / routing notices.
//!
//! - `maestro-send   --text <t> [--workspace <ws>]` → `Maestro.SendToMaestro`
//! - `maestro-state`                                → `Maestro.GetState` (JSON)
//! - `maestro-history`                              → `Maestro.GetHistory`
//! - `maestro-digest`                               → `Maestro.GetDigest`
//! - `maestro-watch  --timeout <secs>`              → subscribe `maestro.events`,
//!   decode each `Event.checks_opaque` JSON frame, print one per line.

use std::path::Path;
use std::time::Duration;

use concerto_proto::v1::maestro_client::MaestroClient;
use concerto_proto::v1::streams_client::StreamsClient;
use concerto_proto::v1::{
    Event, GetDigestRequest, GetHistoryRequest, GetStateRequest, MaestroMessageRequest,
    SubscribeRequest,
};
use futures::StreamExt;

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

const MAESTRO_EVENTS_SUBJECT: &str = "maestro.events";

/// `Maestro.SendToMaestro` — forward a chat turn (freeform, `@workarea`, or a
/// slash directive). Prints `sent` on success.
pub async fn send(socket: &Path, text: &str, workspace: Option<String>) -> Result<(), String> {
    let channel = connect_to_socket(socket).await?;
    let mut client = MaestroClient::new(channel);
    tokio::time::timeout(
        RPC_TIMEOUT,
        client.send_to_maestro(MaestroMessageRequest {
            text: text.to_string(),
            attachments: vec![],
            workspace_id: workspace,
        }),
    )
    .await
    .map_err(|_| format!("SendToMaestro timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|s| format!("SendToMaestro rpc error: {s}"))?;
    println!("sent");
    Ok(())
}

/// `Maestro.GetState` — print the 9-field read-model as one-line JSON.
pub async fn state(socket: &Path) -> Result<(), String> {
    let channel = connect_to_socket(socket).await?;
    let mut client = MaestroClient::new(channel);
    let st = tokio::time::timeout(RPC_TIMEOUT, client.get_state(GetStateRequest {}))
        .await
        .map_err(|_| format!("GetState timed out after {RPC_TIMEOUT:?}"))?
        .map_err(|s| format!("GetState rpc error: {s}"))?
        .into_inner();
    println!(
        "{{\"enabled\":{},\"daily_in_today\":{},\"daily_out_today\":{},\"in_cap\":{},\"out_cap\":{},\"last_digest_at_ms\":{},\"inert\":{},\"inert_reason\":\"{}\",\"maestro_session_id\":\"{}\"}}",
        st.enabled,
        st.daily_in_today,
        st.daily_out_today,
        st.in_cap,
        st.out_cap,
        st.last_digest_at_ms,
        st.inert,
        st.inert_reason,
        st.maestro_session_id
    );
    Ok(())
}

/// `Maestro.GetHistory` — print one `role\ttext` line per persisted turn
/// (oldest-first), prefixed with a count line so the harness can assert.
pub async fn history(socket: &Path) -> Result<(), String> {
    let channel = connect_to_socket(socket).await?;
    let mut client = MaestroClient::new(channel);
    let hist = tokio::time::timeout(RPC_TIMEOUT, client.get_history(GetHistoryRequest {}))
        .await
        .map_err(|_| format!("GetHistory timed out after {RPC_TIMEOUT:?}"))?
        .map_err(|s| format!("GetHistory rpc error: {s}"))?
        .into_inner();
    println!("turns: {}", hist.turns.len());
    for t in hist.turns {
        // Collapse newlines so each turn stays on one greppable line.
        let one_line = t.text.replace('\n', " ");
        println!("{}\t{}", t.role, one_line);
    }
    Ok(())
}

/// `Maestro.GetDigest` — print the digest text (newlines collapsed) + chip count.
pub async fn digest(socket: &Path) -> Result<(), String> {
    let channel = connect_to_socket(socket).await?;
    let mut client = MaestroClient::new(channel);
    let d = tokio::time::timeout(RPC_TIMEOUT, client.get_digest(GetDigestRequest {}))
        .await
        .map_err(|_| format!("GetDigest timed out after {RPC_TIMEOUT:?}"))?
        .map_err(|s| format!("GetDigest rpc error: {s}"))?
        .into_inner();
    println!("chips: {} stale: {}", d.chips.len(), d.stale);
    println!("text: {}", d.text.replace('\n', " "));
    Ok(())
}

/// Subscribe to `maestro.events` and print each decoded `checks_opaque` JSON
/// frame (one per line) until `timeout_secs` elapses. The harness greps these
/// lines to assert that a freeform turn produced a `maestro.message` and that
/// `@workarea` routing produced the right `routing_executed` / no-session notice.
pub async fn watch(socket: &Path, timeout_secs: u64) -> Result<(), String> {
    let channel = connect_to_socket(socket).await?;
    let mut streams = StreamsClient::new(channel);
    let resp = tokio::time::timeout(
        RPC_TIMEOUT,
        streams.subscribe(SubscribeRequest {
            subject: MAESTRO_EVENTS_SUBJECT.to_string(),
            filter: None,
            since_offset: None,
        }),
    )
    .await
    .map_err(|_| format!("Subscribe timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|s| format!("Subscribe rpc error: {s}"))?;
    let mut stream = resp.into_inner();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(Ok(ev))) => print_frame(&ev),
            Ok(Some(Err(s))) => return Err(format!("maestro.events stream error: {s}")),
            Ok(None) => break, // server closed the stream
            Err(_) => break,   // our window elapsed
        }
    }
    Ok(())
}

/// Print the JSON of an `Event`'s opaque maestro frame (if any) on one line.
fn print_frame(ev: &Event) {
    if let Some(bytes) = &ev.checks_opaque {
        match std::str::from_utf8(bytes) {
            Ok(json) => println!("frame: {}", json.replace('\n', " ")),
            Err(_) => println!("frame: <non-utf8 {} bytes>", bytes.len()),
        }
    }
}
