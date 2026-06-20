//! Integration test for the LIVE Maestro `notify_user` side-channel sink
//! (`maestro::tools::side::LiveNotifySink`, Task 507b-ii): proves that driving
//! the frozen `notify_user` tool through `dispatch_side` lands a REAL
//! notification — a row that surfaces via the same `NotificationHandle` inbox
//! the gRPC `Notifications` service and the live `read_inbox_summary` read.
//!
//! `maestro` is `cfg(unix)` (it sits over the `cfg(unix)` agent supervisor), so
//! this whole test is unix-gated to match the module's availability.
#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use concerto_core::maestro::tools::side::{dispatch_side, ChipSlate, LiveNotifySink};
use concerto_core::notifications::handle::{NoEvents, NotificationHandle};
use concerto_core::notifications::push::ExpoPushBackend;
use concerto_persist::{Persistence, PersistenceConfig};
use serde_json::{json, Map};

async fn fresh() -> (tempfile::TempDir, Arc<Persistence>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let persist = Persistence::open(PersistenceConfig {
        db_path: dir.path().join("t.db"),
        max_readers: 2,
    })
    .await
    .expect("open");
    (dir, Arc::new(persist))
}

/// Poll the inbox until at least `want` notifications are present (the live sink
/// records via a spawned task), failing fast under a short guard.
async fn await_inbox(
    handle: &NotificationHandle,
    want: usize,
) -> Vec<concerto_proto::v1::Notification> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let inbox = handle
            .get_inbox(None, None, false, 50)
            .await
            .expect("get_inbox");
        if inbox.len() >= want {
            return inbox;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "notify_user live row never landed (have {}, want {want})",
            inbox.len()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn notify_user_live_sink_creates_a_notification_row() {
    let (_dir, persist) = fresh().await;
    let handle = NotificationHandle::new(
        Arc::clone(&persist),
        Arc::new(ExpoPushBackend::new(None)),
        Arc::new(NoEvents),
    );
    let sink = LiveNotifySink::new(handle.clone(), Some("sess-abc".to_string()));
    let slate = ChipSlate::new();

    // Drive the FROZEN notify_user tool through the live dispatch.
    let mut args = Map::new();
    args.insert("text".into(), json!("build is green"));
    args.insert("severity".into(), json!("high"));
    let out = dispatch_side("notify_user", Some(args), &sink, &slate, 1_000)
        .expect("notify_user succeeds (the frozen Ok)");
    // The frozen output is the empty object `{}` — a real Ok, never an error.
    assert!(out.as_object().expect("object").is_empty());

    // The spawned `notify()` lands a real row that surfaces via the inbox.
    let inbox = await_inbox(&handle, 1).await;
    assert_eq!(inbox.len(), 1, "exactly one notification was created");
    let n = &inbox[0];
    assert_eq!(n.body, "build is green");
    assert_eq!(n.title, "Concerto");
    // notify_user maps to AgentCompletedWithMessage / Session subject.
    assert_eq!(
        n.kind,
        concerto_proto::v1::NotificationKind::AgentCompletedWithMessage as i32
    );
    assert_eq!(
        n.subject_kind,
        concerto_proto::v1::NotificationSubjectKind::Session as i32
    );
    assert_eq!(n.subject_id, "sess-abc");
    // The high intent severity carries onto the stored notification.
    assert_eq!(n.severity, "high");
    assert!(n.read_at_ms.is_none(), "a fresh notification is unread");
}

#[tokio::test(flavor = "multi_thread")]
async fn notify_user_live_sink_defaults_subject_to_maestro_sentinel() {
    let (_dir, persist) = fresh().await;
    let handle = NotificationHandle::new(
        Arc::clone(&persist),
        Arc::new(ExpoPushBackend::new(None)),
        Arc::new(NoEvents),
    );
    // No subject id supplied → the `"maestro"` sentinel (Task 507b-ii).
    let sink = LiveNotifySink::new(handle.clone(), None);
    let slate = ChipSlate::new();

    let mut args = Map::new();
    args.insert("text".into(), json!("fyi"));
    args.insert("severity".into(), json!("whatever")); // unknown ⇒ medium, never errors
    dispatch_side("notify_user", Some(args), &sink, &slate, 2_000)
        .expect("notify_user succeeds even with an unknown severity");

    let inbox = await_inbox(&handle, 1).await;
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].subject_id, "maestro");
    assert_eq!(inbox[0].body, "fyi");
    // An unknown severity defaults to medium on the wire path.
    assert_eq!(inbox[0].severity, "medium");
}
