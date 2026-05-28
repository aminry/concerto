//! `smoke-client stream-session-io --session-id <id> --timeout <s>` —
//! subscribes to BOTH `session.io.<sid>` and `session.events.<sid>` on
//! the same channel, writes every `SessionIoChunk.data` payload
//! verbatim to stdout, and exits 0 the moment a `SessionEvent::Exited`
//! frame arrives. If the timeout fires before that, exit 1.
//!
//! The two subscribers + the timeout are raced via `tokio::select!`.
//! Both streams share the underlying gRPC channel so the dial cost is
//! paid once. We deliberately treat the session.io stream's EOS as a
//! signal that the agent is gone — if the supervisor closes the
//! broadcast (`AgentExited` -> drop tx), the `BroadcastStream` ends.
//! That fallback covers the case where the events stream is closed
//! ahead of the io stream.

use std::path::Path;
use std::time::Duration;

use concerto_proto::v1::event::Body as EventBody;
use concerto_proto::v1::session_event::Kind as SessionEventKind;
use concerto_proto::v1::streams_client::StreamsClient;
use concerto_proto::v1::SubscribeRequest;
use futures::StreamExt;
use tokio::io::{AsyncWriteExt, BufWriter};

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

pub async fn run(socket: &Path, session_id: &str, timeout_secs: u64) -> Result<(), String> {
    if session_id.is_empty() {
        return Err("stream-session-io: --session-id must be non-empty".to_string());
    }

    let channel = connect_to_socket(socket).await?;
    let mut io_client = StreamsClient::new(channel.clone());
    let mut ev_client = StreamsClient::new(channel);

    // Opening the two streams is bounded by the standard 30 s
    // deadline so a wedged Core fails fast even before the
    // user-supplied `--timeout` kicks in.
    let io_subject = format!("session.io.{session_id}");
    let ev_subject = format!("session.events.{session_id}");

    let io_resp = tokio::time::timeout(
        RPC_TIMEOUT,
        io_client.subscribe(SubscribeRequest {
            subject: io_subject,
            filter: None,
            since_offset: None,
        }),
    )
    .await
    .map_err(|_| format!("Subscribe(session.io) timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|status| format!("Subscribe(session.io) rpc error: {status}"))?;

    let ev_resp = tokio::time::timeout(
        RPC_TIMEOUT,
        ev_client.subscribe(SubscribeRequest {
            subject: ev_subject,
            filter: None,
            since_offset: None,
        }),
    )
    .await
    .map_err(|_| format!("Subscribe(session.events) timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|status| format!("Subscribe(session.events) rpc error: {status}"))?;

    let mut io_stream = io_resp.into_inner();
    let mut ev_stream = ev_resp.into_inner();

    let mut stdout = BufWriter::new(tokio::io::stdout());
    let deadline = Duration::from_secs(timeout_secs);

    let outcome = tokio::time::timeout(deadline, async {
        loop {
            tokio::select! {
                // session.io chunks → stdout. EOS (`None`) means the
                // supervisor closed the broadcast; fall back to waiting
                // for the events stream to confirm AgentExited.
                io_item = io_stream.next() => {
                    match io_item {
                        Some(Ok(event)) => {
                            if let Some(EventBody::SessionIo(chunk)) = event.body {
                                stdout
                                    .write_all(&chunk.data)
                                    .await
                                    .map_err(|e| format!("stdout write: {e}"))?;
                            }
                        }
                        Some(Err(status)) => {
                            return Err(format!("session.io stream error: {status}"));
                        }
                        None => {
                            // EOS on the io subject — the supervisor
                            // dropped the broadcast tx. Treat as a
                            // successful end and stop reading.
                            return Ok(());
                        }
                    }
                }
                // session.events frames → look for AgentExited.
                ev_item = ev_stream.next() => {
                    match ev_item {
                        Some(Ok(event)) => {
                            if let Some(EventBody::Session(sess)) = event.body {
                                if matches!(sess.kind, Some(SessionEventKind::Exited(_))) {
                                    return Ok(());
                                }
                            }
                        }
                        Some(Err(status)) => {
                            return Err(format!("session.events stream error: {status}"));
                        }
                        None => {
                            // EOS on the events subject. Same logic as
                            // the io path: the supervisor is gone.
                            return Ok(());
                        }
                    }
                }
            }
        }
    })
    .await;

    // Flush stdout before returning so the caller's `$(...)` capture
    // sees the full agent output even on the timeout path.
    let _ = stdout.flush().await;

    match outcome {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(format!(
            "stream-session-io timed out after {timeout_secs}s without AgentExited"
        )),
    }
}
