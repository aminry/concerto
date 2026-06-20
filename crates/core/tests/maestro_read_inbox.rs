//! Integration test for the live Maestro `read_inbox_summary` tool
//! (`maestro::tools::read::read_inbox_summary_live`, Task 507b-ii): proves it
//! returns the up-to-20 most-recent UNREAD notifications from persistence.
//!
//! Unix-only: `concerto_core::maestro` is `#[cfg(unix)]`-gated (lib.rs), so this
//! test compiles to empty on the Windows desktop-client lane (matching
//! `maestro_e2e.rs` / `maestro_notify_user.rs`).
#![cfg(unix)]

use concerto_core::maestro::tools::read::read_inbox_summary_live;
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

fn mk(id: &str, kind: &str, severity: &str, created_at: i64) -> NewNotification {
    NewNotification {
        id: id.into(),
        kind: kind.into(),
        subject_kind: "workspace".into(),
        subject_id: "ws".into(),
        workspace_id: None,
        workarea_id: None,
        session_id: None,
        title: format!("title-{id}"),
        body: "b".into(),
        chips_json: None,
        approval_json: None,
        severity: severity.into(),
        created_at,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn read_inbox_summary_live_returns_unread_only() {
    let (_dir, persist) = fresh().await;
    {
        let mut w = persist.writer().await;
        notifications::insert(&mut w, mk("n-1", "agent_crashed", "high", 100))
            .await
            .unwrap();
        notifications::insert(&mut w, mk("n-2", "check_run_failed", "low", 200))
            .await
            .unwrap();
        // n-2 is read → excluded from the unread summary.
        notifications::mark_read(&mut w, "n-2", 300).await.unwrap();
    }

    let v = read_inbox_summary_live(&persist).await.expect("live inbox");
    assert_eq!(v["unread"], 1, "only the unread notification counts");
    let items = v["items"].as_array().expect("items array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "n-1");
    assert_eq!(items[0]["kind"], "agent_crashed");
    assert_eq!(items[0]["severity"], "high");

    // Empty inbox → unread 0.
    let (_dir2, empty) = fresh().await;
    let v = read_inbox_summary_live(&empty).await.unwrap();
    assert_eq!(v["unread"], 0);
    assert!(v["items"].as_array().unwrap().is_empty());
}
