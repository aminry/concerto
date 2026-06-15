//! Integration tests for the 0017_notifications migration + the
//! `notifications` persist module (Task 501).
//!
//! Opens a fresh tempdir DB via `Persistence::open` (runs `sqlx::migrate!`),
//! sets up the FK parents (workspace / workarea / device) with raw SQL, then
//! exercises the CRUD: insert/get round-trip, the inbox feed + unread filter,
//! mark-read + first-wins action idempotency, the CHECK constraints, the
//! delivery ledger, and FK cascade on workarea/notification delete.

use concerto_persist::notifications::{self, NewDelivery, NewNotification};
use concerto_persist::{Persistence, PersistenceConfig};

async fn fresh_db() -> (tempfile::TempDir, Persistence) {
    let dir = tempfile::tempdir().expect("tempdir");
    let persist = Persistence::open(PersistenceConfig {
        db_path: dir.path().join("test.db"),
        max_readers: 2,
    })
    .await
    .expect("open");
    (dir, persist)
}

/// Insert the FK parents a notification can reference.
async fn seed_parents(persist: &Persistence) {
    let mut w = persist.writer().await;
    sqlx::query("INSERT INTO workspaces (id, name, slug, created_at) VALUES ('ws-1','WS','ws',1700000000000)")
        .execute(&mut *w)
        .await
        .expect("insert workspace");
    sqlx::query(
        "INSERT INTO workareas (id, workspace_id, composer_name, branch_name, worktree_root, status, created_at)
         VALUES ('wa-1','ws-1','bach','concerto/bach','/tmp/wa-1','active',1700000001000)",
    )
    .execute(&mut *w)
    .await
    .expect("insert workarea");
    sqlx::query("INSERT INTO devices (id, name, public_key, paired_at) VALUES ('dev-1','iPhone',?,1700000002000)")
        .bind(vec![0u8; 32])
        .execute(&mut *w)
        .await
        .expect("insert device");
}

fn sample(id: &str, created_at: i64) -> NewNotification {
    NewNotification {
        id: id.to_string(),
        kind: "tool_approval_needed".into(),
        subject_kind: "workarea".into(),
        subject_id: "wa-1".into(),
        workspace_id: Some("ws-1".into()),
        workarea_id: Some("wa-1".into()),
        session_id: None,
        title: "Approve Bash?".into(),
        body: "ls -la".into(),
        chips_json: Some(r#"[{"rule_id":"approve","workarea_id":"wa-1","title":"Approve","priority":90,"created_at_ms":1700000000000,"action":"approve"}]"#.into()),
        approval_json: Some(r#"{"approval_id":"ta-1","session_id":"sess-1","tool_name":"Bash","payload_json":"{}","urgent":false}"#.into()),
        severity: "high".into(),
        created_at,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn schema_tables_exist() {
    let (_dir, persist) = fresh_db().await;
    let pool = persist.readers();
    for table in ["notifications", "notification_deliveries"] {
        let found: Option<String> =
            sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table' AND name=?")
                .bind(table)
                .fetch_optional(pool)
                .await
                .expect("query sqlite_master");
        assert_eq!(found.as_deref(), Some(table), "{table} table must exist");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_get_roundtrip() {
    let (_dir, persist) = fresh_db().await;
    seed_parents(&persist).await;
    {
        let mut w = persist.writer().await;
        let id = notifications::insert(&mut w, sample("n-1", 1700000010000))
            .await
            .expect("insert");
        assert_eq!(id, "n-1");
    }
    let got = notifications::get(persist.readers(), "n-1")
        .await
        .expect("get")
        .expect("present");
    assert_eq!(got.kind, "tool_approval_needed");
    assert_eq!(got.subject_kind, "workarea");
    assert_eq!(got.workarea_id.as_deref(), Some("wa-1"));
    assert_eq!(got.severity, "high");
    assert!(got.chips_json.is_some());
    assert!(got.approval_json.is_some());
    assert_eq!(got.read_at, None);
    assert_eq!(got.action_taken, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn list_inbox_filters_and_unread() {
    let (_dir, persist) = fresh_db().await;
    seed_parents(&persist).await;
    {
        let mut w = persist.writer().await;
        notifications::insert(&mut w, sample("n-1", 1700000010000))
            .await
            .unwrap();
        notifications::insert(&mut w, sample("n-2", 1700000020000))
            .await
            .unwrap();
        // mark n-1 read
        assert_eq!(
            notifications::mark_read(&mut w, "n-1", 1700000030000)
                .await
                .unwrap(),
            1
        );
    }
    let pool = persist.readers();
    let all = notifications::list_inbox(pool, None, None, false, 0)
        .await
        .unwrap();
    assert_eq!(all.len(), 2, "both rows in the full feed");
    assert_eq!(all[0].id, "n-2", "newest first");
    let unread = notifications::list_inbox(pool, None, None, true, 0)
        .await
        .unwrap();
    assert_eq!(unread.len(), 1, "only the unread row");
    assert_eq!(unread[0].id, "n-2");
    let by_wa = notifications::list_inbox(pool, None, Some("wa-1"), false, 0)
        .await
        .unwrap();
    assert_eq!(by_wa.len(), 2);
    let other_wa = notifications::list_inbox(pool, None, Some("wa-none"), false, 0)
        .await
        .unwrap();
    assert!(other_wa.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn mark_read_and_action_are_idempotent() {
    let (_dir, persist) = fresh_db().await;
    seed_parents(&persist).await;
    let mut w = persist.writer().await;
    notifications::insert(&mut w, sample("n-1", 1700000010000))
        .await
        .unwrap();
    assert_eq!(notifications::mark_read(&mut w, "n-1", 1).await.unwrap(), 1);
    assert_eq!(
        notifications::mark_read(&mut w, "n-1", 2).await.unwrap(),
        0,
        "second mark_read is a no-op"
    );
    assert_eq!(
        notifications::set_action_taken(&mut w, "n-1", "approve", 3, Some("dev-1"))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        notifications::set_action_taken(&mut w, "n-1", "deny", 4, Some("dev-1"))
            .await
            .unwrap(),
        0,
        "first-wins: second action is a no-op"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn checks_reject_bad_values() {
    let (_dir, persist) = fresh_db().await;
    seed_parents(&persist).await;
    let mut w = persist.writer().await;
    let bad_kind = sqlx::query(
        "INSERT INTO notifications (id, kind, subject_kind, subject_id, title, body, severity, created_at)
         VALUES ('bk','bogus','workarea','wa-1','t','b','high',1)",
    )
    .execute(&mut *w)
    .await;
    assert!(bad_kind.is_err(), "kind CHECK must reject 'bogus'");

    let bad_subject = sqlx::query(
        "INSERT INTO notifications (id, kind, subject_kind, subject_id, title, body, severity, created_at)
         VALUES ('bs','agent_crashed','bogus','wa-1','t','b','high',1)",
    )
    .execute(&mut *w)
    .await;
    assert!(
        bad_subject.is_err(),
        "subject_kind CHECK must reject 'bogus'"
    );

    let bad_sev = sqlx::query(
        "INSERT INTO notifications (id, kind, subject_kind, subject_id, title, body, severity, created_at)
         VALUES ('bv','agent_crashed','workarea','wa-1','t','b','bogus',1)",
    )
    .execute(&mut *w)
    .await;
    assert!(bad_sev.is_err(), "severity CHECK must reject 'bogus'");
}

#[tokio::test(flavor = "multi_thread")]
async fn fk_cascade_on_workarea_delete() {
    let (_dir, persist) = fresh_db().await;
    seed_parents(&persist).await;
    {
        let mut w = persist.writer().await;
        notifications::insert(&mut w, sample("n-1", 1700000010000))
            .await
            .unwrap();
        sqlx::query("DELETE FROM workareas WHERE id='wa-1'")
            .execute(&mut *w)
            .await
            .expect("delete workarea");
    }
    let got = notifications::get(persist.readers(), "n-1").await.unwrap();
    assert!(
        got.is_none(),
        "deleting the workarea cascades to its notifications"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn deliveries_upsert_and_cascade() {
    let (_dir, persist) = fresh_db().await;
    seed_parents(&persist).await;
    {
        let mut w = persist.writer().await;
        notifications::insert(&mut w, sample("n-1", 1700000010000))
            .await
            .unwrap();
        notifications::upsert_delivery(
            &mut w,
            NewDelivery {
                notification_id: "n-1".into(),
                device_id: "dev-1".into(),
                delivered_at: Some(1700000011000),
                fetched_at: None,
            },
        )
        .await
        .unwrap();
        // upsert again with fetched_at; delivered_at must survive (COALESCE).
        notifications::upsert_delivery(
            &mut w,
            NewDelivery {
                notification_id: "n-1".into(),
                device_id: "dev-1".into(),
                delivered_at: None,
                fetched_at: Some(1700000012000),
            },
        )
        .await
        .unwrap();
    }
    let deliveries = notifications::list_deliveries(persist.readers(), "n-1")
        .await
        .unwrap();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].delivered_at, Some(1700000011000));
    assert_eq!(deliveries[0].fetched_at, Some(1700000012000));

    // Deleting the notification cascades to its deliveries.
    {
        let mut w = persist.writer().await;
        sqlx::query("DELETE FROM notifications WHERE id='n-1'")
            .execute(&mut *w)
            .await
            .unwrap();
    }
    let after = notifications::list_deliveries(persist.readers(), "n-1")
        .await
        .unwrap();
    assert!(
        after.is_empty(),
        "deliveries cascade on notification delete"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn dedup_query_matches_within_key_and_ignores_others() {
    let (_dir, persist) = fresh_db().await;
    seed_parents(&persist).await;
    {
        let mut w = persist.writer().await;
        // A second workarea so we can prove the key is workarea-scoped.
        sqlx::query(
            "INSERT INTO workareas (id, workspace_id, composer_name, branch_name, worktree_root, status, created_at)
             VALUES ('wa-2','ws-1','bee','concerto/bee','/tmp/wa-2','active',1700000001000)",
        )
        .execute(&mut *w)
        .await
        .unwrap();

        let mut n = sample("n-1", 1700000010000);
        n.kind = "check_run_failed".into();
        notifications::insert(&mut w, n).await.unwrap();
        // Same key but READ — must be ignored by the de-dup query.
        let mut n_read = sample("n-read", 1700000011000);
        n_read.kind = "check_run_failed".into();
        notifications::insert(&mut w, n_read).await.unwrap();
        notifications::mark_read(&mut w, "n-read", 1700000012000)
            .await
            .unwrap();
        // Same key but a DIFFERENT workarea — must be ignored.
        let mut n_other = sample("n-other", 1700000013000);
        n_other.kind = "check_run_failed".into();
        n_other.workarea_id = Some("wa-2".into());
        notifications::insert(&mut w, n_other).await.unwrap();
    }
    let pool = persist.readers();
    // Within window (floor below the row's created_at): hits n-1.
    let hit = notifications::find_unread_for_dedup_key(
        pool,
        Some("ws-1"),
        Some("wa-1"),
        "check_run_failed",
        "wa-1",
        1700000000000,
    )
    .await
    .unwrap();
    assert_eq!(
        hit.map(|r| r.id),
        Some("n-1".into()),
        "de-dup hits the unread same-key row"
    );

    // Floor above the row's created_at: no hit (outside window).
    let miss = notifications::find_unread_for_dedup_key(
        pool,
        Some("ws-1"),
        Some("wa-1"),
        "check_run_failed",
        "wa-1",
        1700000099000,
    )
    .await
    .unwrap();
    assert!(
        miss.is_none(),
        "rows older than the window floor are not de-dup candidates"
    );

    // update_body_and_at refreshes in place.
    {
        let mut w = persist.writer().await;
        assert_eq!(
            notifications::update_body_and_at(&mut w, "n-1", "fresh body", 1700000050000)
                .await
                .unwrap(),
            1
        );
    }
    let updated = notifications::get(pool, "n-1").await.unwrap().unwrap();
    assert_eq!(updated.body, "fresh body");
    assert_eq!(updated.created_at, 1700000050000);
}

#[tokio::test(flavor = "multi_thread")]
async fn migration_0018_widens_push_platform_and_adds_dnd_until() {
    let (_dir, persist) = fresh_db().await;
    let mut w = persist.writer().await;
    // 'expo' is now accepted (widened CHECK) + dnd_until is writable.
    sqlx::query(
        "INSERT INTO devices (id, name, public_key, paired_at, push_token, push_platform, dnd_until)
         VALUES ('d-expo','Phone',?,1,'ExponentPushToken[x]','expo',1700000099000)",
    )
    .bind(vec![0u8; 32])
    .execute(&mut *w)
    .await
    .expect("expo platform + dnd_until must be accepted post-0018");
    // apns/fcm still accepted.
    sqlx::query("INSERT INTO devices (id, name, public_key, paired_at, push_platform) VALUES ('d-apns','P',?,1,'apns')")
        .bind(vec![0u8; 32])
        .execute(&mut *w)
        .await
        .expect("apns still accepted");
    // a bogus platform is still rejected by the widened CHECK.
    let bad = sqlx::query("INSERT INTO devices (id, name, public_key, paired_at, push_platform) VALUES ('d-bad','P',?,1,'telegram')")
        .bind(vec![0u8; 32])
        .execute(&mut *w)
        .await;
    assert!(
        bad.is_err(),
        "push_platform CHECK must still reject unknown platforms"
    );

    let dnd: Option<i64> = sqlx::query_scalar("SELECT dnd_until FROM devices WHERE id='d-expo'")
        .fetch_one(persist.readers())
        .await
        .unwrap();
    assert_eq!(dnd, Some(1700000099000));
}
