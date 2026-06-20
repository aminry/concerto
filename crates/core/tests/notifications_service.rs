//! Integration test for the live `Notifications` gRPC service (Task 507b-3):
//! proves the service is REGISTERED + reachable over a real Core's UDS and that
//! each RPC delegates to `NotificationHandle` correctly.
//!
//! Notification CREATION is internal (notify() called by 04/13/05 + the maestro
//! `notify_user` sink, 507b-ii), so a fresh Core has an empty inbox — this test
//! exercises the read/act/settings RPCs for reachability.
//!
//! Unix-only: the harness spawns a `concerto-core` subprocess over UDS (the
//! locked transport), and `concerto-test-harness` uses `tokio::net::UnixStream`.
//! Gating to empty on Windows keeps the desktop-client lane (which excludes the
//! UDS dev/test crates) from pulling the harness — matches `grpc_runtime.rs`.
#![cfg(unix)]

use concerto_proto::v1::{GetNotificationRequest, InboxFilter, UpdateWorkspaceNotifyRequest};
use concerto_test_harness::CoreUnderTest;

#[tokio::test(flavor = "multi_thread")]
async fn notifications_service_is_live_over_grpc() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let mut client = core
        .notifications_client()
        .await
        .expect("notifications client");

    // GetInbox is reachable and returns an (empty) feed on a fresh Core —
    // proves the service is registered (not unimplemented/absent) + delegates.
    let inbox = client
        .get_inbox(InboxFilter {
            workspace_id: None,
            workarea_id: None,
            unread_only: false,
            limit: 0,
        })
        .await
        .expect("get_inbox reachable")
        .into_inner();
    assert!(
        inbox.notifications.is_empty(),
        "a fresh Core has no notifications"
    );

    // GetNotification on an unknown id → NOT_FOUND (the handler's load path).
    let err = client
        .get_notification(GetNotificationRequest {
            id: "does-not-exist".into(),
            device_id: "dev-x".into(),
        })
        .await
        .expect_err("unknown id must error");
    assert_eq!(err.code(), tonic::Code::NotFound);

    // UpdateWorkspaceSettings is reachable (the write path; a no-op success on a
    // not-yet-existing workspace id — the RMW key setter tolerates it).
    client
        .update_workspace_settings(UpdateWorkspaceNotifyRequest {
            workspace_id: "ws-x".into(),
            opt_out: true,
        })
        .await
        .expect("update_workspace_settings reachable");

    core.shutdown().await.ok();
}
