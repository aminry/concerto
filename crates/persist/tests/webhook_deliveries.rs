//! Tests for the Task 315 `webhook_deliveries` accessor (migration 0013): the
//! `insert_delivery_if_absent` idempotency contract (first-insert ⇒ process,
//! replay ⇒ drop), restart-survival (the row persists across a reopen of the same
//! DB file), and the 1h-TTL `prune_expired` sweep.

use concerto_persist::{webhook_deliveries, Persistence, PersistenceConfig};

async fn fresh_db(dir: &std::path::Path) -> Persistence {
    Persistence::open(PersistenceConfig {
        db_path: dir.join("test.db"),
        max_readers: 2,
    })
    .await
    .expect("open")
}

#[tokio::test]
async fn first_insert_processes_replay_drops() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let persist = fresh_db(tmp.path()).await;
    let mut w = persist.writer().await;

    // First time seen ⇒ newly inserted ⇒ process.
    assert!(
        webhook_deliveries::insert_delivery_if_absent(&mut w, "delivery-1", "repo-1", 1_000)
            .await
            .unwrap()
    );
    // Replay (same delivery_id) ⇒ not inserted ⇒ drop.
    assert!(
        !webhook_deliveries::insert_delivery_if_absent(&mut w, "delivery-1", "repo-1", 2_000)
            .await
            .unwrap()
    );
    // A different id is independent ⇒ process.
    assert!(
        webhook_deliveries::insert_delivery_if_absent(&mut w, "delivery-2", "repo-1", 3_000)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn dedup_survives_restart() {
    let tmp = tempfile::tempdir().expect("tempdir");
    {
        let persist = fresh_db(tmp.path()).await;
        let mut w = persist.writer().await;
        assert!(webhook_deliveries::insert_delivery_if_absent(
            &mut w,
            "delivery-1",
            "repo-1",
            1_000
        )
        .await
        .unwrap());
    }
    // Reopen the SAME db file (a Core restart). The prior delivery must still be
    // deduped — the table is persisted, not in-memory.
    let persist = fresh_db(tmp.path()).await;
    let mut w = persist.writer().await;
    assert!(
        !webhook_deliveries::insert_delivery_if_absent(&mut w, "delivery-1", "repo-1", 5_000)
            .await
            .unwrap(),
        "a redelivery after a restart is still a replay"
    );
}

#[tokio::test]
async fn prune_drops_only_expired() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let persist = fresh_db(tmp.path()).await;
    let mut w = persist.writer().await;

    let ttl = webhook_deliveries::WEBHOOK_DELIVERY_TTL_MS;
    webhook_deliveries::insert_delivery_if_absent(&mut w, "old", "repo-1", 0)
        .await
        .unwrap();
    webhook_deliveries::insert_delivery_if_absent(&mut w, "fresh", "repo-1", ttl)
        .await
        .unwrap();

    // now = TTL + 1: `old` (received_at 0) is older than the window; `fresh`
    // (received_at == TTL) is at the boundary and survives.
    let pruned = webhook_deliveries::prune_expired(&mut w, ttl + 1)
        .await
        .unwrap();
    assert_eq!(pruned, 1);

    // `old` is gone ⇒ re-inserts as new; `fresh` is still a replay.
    assert!(
        webhook_deliveries::insert_delivery_if_absent(&mut w, "old", "repo-1", 99)
            .await
            .unwrap()
    );
    assert!(
        !webhook_deliveries::insert_delivery_if_absent(&mut w, "fresh", "repo-1", 99)
            .await
            .unwrap()
    );
}
