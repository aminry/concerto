//! `smoke-client get-inbox` — calls `Notifications.GetInbox` (Task 507) and
//! prints one JSON object per returned notification (newest-first), with the
//! `id`, `kind` (proto enum string name), `severity`, and `body`. The
//! `notifications` smoke check seeds a row into the live Core's DB and greps
//! this output for the seeded id, proving the live `Notifications` gRPC service
//! round-trips over the loopback UDS.

use std::path::Path;
use std::time::Duration;

use concerto_proto::v1::notifications_client::NotificationsClient;
use concerto_proto::v1::streams_client::StreamsClient;
use concerto_proto::v1::{Event, InboxFilter, NotificationKind, SubscribeRequest};
use futures::StreamExt;

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

const NOTIFICATION_EVENTS_SUBJECT: &str = "notification.events";

/// Run `GetInbox` with an all-scopes filter (`unread_only` toggled by the
/// caller) and print one JSON line per notification.
pub async fn run(socket: &Path, unread_only: bool, limit: u32) -> Result<(), String> {
    let channel = connect_to_socket(socket).await?;
    let mut client = NotificationsClient::new(channel);

    let filter = InboxFilter {
        workspace_id: None,
        workarea_id: None,
        unread_only,
        limit,
    };

    let resp = tokio::time::timeout(RPC_TIMEOUT, client.get_inbox(filter))
        .await
        .map_err(|_| format!("GetInbox timed out after {RPC_TIMEOUT:?}"))?
        .map_err(|status| format!("GetInbox rpc error: {status}"))?;

    for n in resp.into_inner().notifications {
        let kind_str = NotificationKind::try_from(n.kind)
            .map(|k| k.as_str_name().to_string())
            .unwrap_or_else(|_| format!("UNKNOWN({})", n.kind));
        let out = serde_json::json!({
            "id": n.id,
            "kind": kind_str,
            "severity": n.severity,
            "subject_id": n.subject_id,
            "body": n.body,
        });
        println!("{out}");
    }
    Ok(())
}

/// Subscribe to `notification.events` and print each decoded frame (one per
/// line) until `timeout_secs` elapses. Proves the live notification stream:
/// a `MarkRead`/`ActOnChip`/`notify` on ANY transport emits here, because all
/// front doors share one `notification.events` broadcast (the design/14 R-8
/// cross-device sync). Mark a notification read in the web client and watch the
/// `notification.read` frame arrive over this UDS subscription.
pub async fn watch(socket: &Path, timeout_secs: u64) -> Result<(), String> {
    let channel = connect_to_socket(socket).await?;
    let mut streams = StreamsClient::new(channel);
    let resp = tokio::time::timeout(
        RPC_TIMEOUT,
        streams.subscribe(SubscribeRequest {
            subject: NOTIFICATION_EVENTS_SUBJECT.to_string(),
            filter: None,
            since_offset: None,
        }),
    )
    .await
    .map_err(|_| format!("Subscribe timed out after {RPC_TIMEOUT:?}"))?
    .map_err(|s| format!("Subscribe rpc error: {s}"))?;
    let mut stream = resp.into_inner();
    println!("watching {NOTIFICATION_EVENTS_SUBJECT} for {timeout_secs}s…");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(Ok(ev))) => print_frame(&ev),
            Ok(Some(Err(s))) => return Err(format!("notification.events stream error: {s}")),
            Ok(None) => break,
            Err(_) => break,
        }
    }
    Ok(())
}

/// Print an `Event`'s opaque notification frame JSON on one line.
fn print_frame(ev: &Event) {
    match &ev.checks_opaque {
        Some(bytes) => match std::str::from_utf8(bytes) {
            Ok(json) => println!("frame: {}", json.replace('\n', " ")),
            Err(_) => println!("frame: <non-utf8 {} bytes>", bytes.len()),
        },
        None => println!("frame: <empty>"),
    }
}
