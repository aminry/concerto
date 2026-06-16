//! Integration test for the Task 507 `NotificationHandle::notify` orchestration:
//! de-dup, insert, push fan-out over a real device, event emission, dedup-refresh
//! (no second wakeup), and per-workspace opt-out (insert but no push).

use std::sync::{Arc, Mutex};

use concerto_core::notifications::handle::{
    Clock, NotificationEvent, NotificationEvents, NotificationHandle,
};
use concerto_core::notifications::model::{NotificationKind, NotifyRequest, SubjectKind};
use concerto_core::notifications::push::MockPushBackend;
use concerto_persist::{Persistence, PersistenceConfig};

/// Fixed-time clock for deterministic de-dup windows.
struct FixedClock(Mutex<i64>);
impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        *self.0.lock().unwrap()
    }
}

/// Recording events sink.
#[derive(Default)]
struct RecordingEvents(Mutex<Vec<NotificationEvent>>);
impl NotificationEvents for RecordingEvents {
    fn emit(&self, event: NotificationEvent) {
        self.0.lock().unwrap().push(event);
    }
}

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

async fn seed_workspace(persist: &Persistence, opt_out: bool) {
    let settings = if opt_out {
        r#"{"notifications_opt_out": true}"#
    } else {
        "{}"
    };
    let mut w = persist.writer().await;
    sqlx::query("INSERT INTO workspaces (id,name,slug,settings_json,created_at) VALUES ('ws-1','WS','ws',?,1)")
        .bind(settings)
        .execute(&mut *w)
        .await
        .unwrap();
    sqlx::query("INSERT INTO devices (id,name,public_key,paired_at,push_token,push_platform) VALUES ('dev-1','P',?,1,'tok','expo')")
        .bind(vec![0u8; 32])
        .execute(&mut *w)
        .await
        .unwrap();
}

fn req(kind: NotificationKind, body: &str) -> NotifyRequest {
    NotifyRequest {
        kind,
        subject_kind: SubjectKind::Workspace,
        subject_id: "ws-1".into(),
        workspace_id: Some("ws-1".into()),
        workarea_id: None,
        session_id: None,
        title: "T".into(),
        body: body.into(),
        chips: vec![],
        approval: None,
        severity: None,
    }
}

/// A `notify_user`-shaped request: BOTH `workspace_id` and `workarea_id` are
/// None and the subject is the `"maestro"` sentinel — exactly what the dominant
/// V1 producer (`LiveNotifySink::request_for`) builds. Regression coverage for
/// the both-None de-dup-key fix.
fn req_both_none(kind: NotificationKind, body: &str) -> NotifyRequest {
    NotifyRequest {
        kind,
        subject_kind: SubjectKind::Session,
        subject_id: "maestro".into(),
        workspace_id: None,
        workarea_id: None,
        session_id: None,
        title: "T".into(),
        body: body.into(),
        chips: vec![],
        approval: None,
        severity: None,
    }
}

fn handle(
    persist: Arc<Persistence>,
    push: Arc<MockPushBackend>,
    events: Arc<RecordingEvents>,
    clock: Arc<FixedClock>,
) -> NotificationHandle {
    NotificationHandle::new(persist, push, events).with_clock(clock)
}

/// Regression (shared-event-channel + both-None de-dup fix): two `notify()`
/// calls with BOTH ids None + equal `(kind, subject_id)` within the 5-min
/// window must de-dup to ONE row and ONE wakeup (refresh-in-place, no second
/// push). Before the both-None SQL fix the de-dup query never matched (SQLite
/// `workspace_id = NULL` is never true), so the dominant `notify_user` producer
/// inserted two rows + sent two wakeups. design/14 §3.7.
#[tokio::test(flavor = "multi_thread")]
async fn notify_both_ids_none_dedups_to_one_row_and_one_wakeup() {
    let (_dir, persist) = fresh().await;
    // A pushable device so a high-priority kind can wake exactly once.
    {
        let mut w = persist.writer().await;
        sqlx::query("INSERT INTO devices (id,name,public_key,paired_at,push_token,push_platform) VALUES ('dev-1','P',?,1,'tok','expo')")
            .bind(vec![0u8; 32])
            .execute(&mut *w)
            .await
            .unwrap();
    }
    let push = Arc::new(MockPushBackend::new());
    let events = Arc::new(RecordingEvents::default());
    let clock = Arc::new(FixedClock(Mutex::new(1_000_000)));
    let h = handle(persist.clone(), push.clone(), events.clone(), clock.clone());

    // First notify_user-shaped notification (AgentCompletedWithMessage pushes).
    let id1 = h
        .notify(req_both_none(
            NotificationKind::AgentCompletedWithMessage,
            "done",
        ))
        .await
        .unwrap();
    assert_eq!(push.send_count(), 1, "first both-None notify wakes once");

    // Same both-None de-dup key within the window → refresh, NO second wakeup,
    // SAME row id (the both-None branch must match the existing unread row).
    let id2 = h
        .notify(req_both_none(
            NotificationKind::AgentCompletedWithMessage,
            "done again",
        ))
        .await
        .unwrap();
    assert_eq!(id2, id1, "both-None de-dup returns the same id (one row)");
    assert_eq!(
        push.send_count(),
        1,
        "no second wakeup on both-None de-dup refresh"
    );

    // Exactly one inbox row, body refreshed in place.
    let inbox = h.get_inbox(None, None, false, 0).await.unwrap();
    assert_eq!(inbox.len(), 1, "two notify_user calls de-dup to one row");
    assert_eq!(inbox[0].body, "done again", "body refreshed in place");

    // Events: Created then Updated (one of each, not two Created).
    let evs = events.0.lock().unwrap().clone();
    assert!(matches!(evs[0], NotificationEvent::Created(_)));
    assert!(matches!(evs[1], NotificationEvent::Updated(_)));
}

#[tokio::test(flavor = "multi_thread")]
async fn notify_inserts_pushes_and_dedups() {
    let (_dir, persist) = fresh().await;
    seed_workspace(&persist, false).await;
    let push = Arc::new(MockPushBackend::new());
    let events = Arc::new(RecordingEvents::default());
    let clock = Arc::new(FixedClock(Mutex::new(1_000_000)));
    let h = handle(persist.clone(), push.clone(), events.clone(), clock.clone());

    // First notify (high-severity kind → pushes to the eligible device).
    let id1 = h
        .notify(req(NotificationKind::AgentCrashed, "boom"))
        .await
        .unwrap();
    assert_eq!(push.send_count(), 1, "one eligible device woken");
    assert_eq!(push.sends()[0].body.notification_id, id1);
    assert_eq!(push.sends()[0].body.kind, "agent_crashed");

    // Inbox shows it.
    let inbox = h.get_inbox(None, None, false, 0).await.unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].id, id1);

    // Same de-dup key within the 5-min window → refresh, NO second wakeup.
    let id2 = h
        .notify(req(NotificationKind::AgentCrashed, "boom again"))
        .await
        .unwrap();
    assert_eq!(id2, id1, "de-dup returns the same id");
    assert_eq!(push.send_count(), 1, "no second wakeup on de-dup refresh");
    let inbox = h.get_inbox(None, None, false, 0).await.unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].body, "boom again", "body refreshed in place");

    // Events: Created then Updated.
    let evs = events.0.lock().unwrap().clone();
    assert!(matches!(evs[0], NotificationEvent::Created(_)));
    assert!(matches!(evs[1], NotificationEvent::Updated(_)));
}

#[tokio::test(flavor = "multi_thread")]
async fn opted_out_workspace_inserts_but_never_pushes() {
    let (_dir, persist) = fresh().await;
    seed_workspace(&persist, true).await; // enterprise-private
    let push = Arc::new(MockPushBackend::new());
    let events = Arc::new(RecordingEvents::default());
    let clock = Arc::new(FixedClock(Mutex::new(1_000_000)));
    let h = handle(persist.clone(), push.clone(), events.clone(), clock);

    let id = h
        .notify(req(NotificationKind::AgentCrashed, "boom"))
        .await
        .unwrap();
    // Inbox still records (design/14: inbox always populated)...
    let inbox = h.get_inbox(None, None, false, 0).await.unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].id, id);
    // ...but NO wakeup is sent for an opted-out workspace.
    assert_eq!(push.send_count(), 0, "opted-out workspace never pushes");
}

#[tokio::test(flavor = "multi_thread")]
async fn inbox_only_kind_does_not_push() {
    let (_dir, persist) = fresh().await;
    seed_workspace(&persist, false).await;
    let push = Arc::new(MockPushBackend::new());
    let events = Arc::new(RecordingEvents::default());
    let clock = Arc::new(FixedClock(Mutex::new(1_000_000)));
    let h = handle(persist.clone(), push.clone(), events, clock);

    // check_run_failed is inbox-only by default (design/14 §3.1).
    h.notify(req(NotificationKind::CheckRunFailed, "ci red"))
        .await
        .unwrap();
    assert_eq!(push.send_count(), 0, "inbox-only kind does not push");
    assert_eq!(h.get_inbox(None, None, false, 0).await.unwrap().len(), 1);
}
