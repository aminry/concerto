//! Integration test for the Task 504 post-wakeup fetch path
//! (`notifications::fanout::fetch_for_device`).
//!
//! Opens a real `Persistence`, seeds a device + a notification (no other FK
//! parents are required — the scoping FKs are nullable), then proves the fetch
//! returns the wire payload and records the per-device `fetched_at` delivery row.

use concerto_core::notifications::fanout::fetch_for_device;
use concerto_persist::notifications::{self, NewNotification};
use concerto_persist::{Persistence, PersistenceConfig};

async fn fresh() -> (tempfile::TempDir, Persistence) {
    let dir = tempfile::tempdir().expect("tempdir");
    let persist = Persistence::open(PersistenceConfig {
        db_path: dir.path().join("t.db"),
        max_readers: 2,
    })
    .await
    .expect("open");
    (dir, persist)
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_for_device_returns_payload_and_records_fetched_at() {
    let (_dir, persist) = fresh().await;
    {
        let mut w = persist.writer().await;
        // a device for the delivery FK
        sqlx::query("INSERT INTO devices (id,name,public_key,paired_at,push_token,push_platform) VALUES ('dev-1','P',?,1,'tok','expo')")
            .bind(vec![0u8; 32])
            .execute(&mut *w)
            .await
            .unwrap();
        // a workspace-scoped notification (no FK parents needed)
        notifications::insert(
            &mut w,
            NewNotification {
                id: "n-1".into(),
                kind: "agent_crashed".into(),
                subject_kind: "workspace".into(),
                subject_id: "ws-x".into(),
                workspace_id: None,
                workarea_id: None,
                session_id: None,
                title: "Agent crashed".into(),
                body: "panic in tool loop".into(),
                chips_json: None,
                approval_json: None,
                severity: "high".into(),
                created_at: 1700000000000,
            },
        )
        .await
        .unwrap();
    }

    // Unknown id → None.
    assert!(
        fetch_for_device(&persist, "missing", "dev-1", 1700000050000)
            .await
            .unwrap()
            .is_none()
    );

    // Known id → payload + recorded fetched_at.
    let payload = fetch_for_device(&persist, "n-1", "dev-1", 1700000050000)
        .await
        .unwrap()
        .expect("payload present");
    assert_eq!(payload.id, "n-1");
    assert_eq!(payload.title, "Agent crashed");
    assert_eq!(payload.severity, "high");
    // kind enum mapped from the DB string.
    assert_eq!(
        payload.kind,
        concerto_proto::v1::NotificationKind::AgentCrashed as i32
    );

    let deliveries = notifications::list_deliveries(persist.readers(), "n-1")
        .await
        .unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].device_id, "dev-1");
    assert_eq!(deliveries[0].fetched_at, Some(1700000050000));
}
