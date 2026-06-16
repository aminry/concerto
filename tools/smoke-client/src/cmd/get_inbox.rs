//! `smoke-client get-inbox` — calls `Notifications.GetInbox` (Task 507) and
//! prints one JSON object per returned notification (newest-first), with the
//! `id`, `kind` (proto enum string name), `severity`, and `body`. The
//! `notifications` smoke check seeds a row into the live Core's DB and greps
//! this output for the seeded id, proving the live `Notifications` gRPC service
//! round-trips over the loopback UDS.

use std::path::Path;

use concerto_proto::v1::notifications_client::NotificationsClient;
use concerto_proto::v1::{InboxFilter, NotificationKind};

use super::RPC_TIMEOUT;
use crate::connect::connect_to_socket;

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
