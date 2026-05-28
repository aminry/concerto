//! Integration tests for the Persistence migration runner.
//!
//! These tests treat `Persistence::open` as the boundary: they construct
//! `PersistenceConfig` against a tempdir, then assert the side effects the
//! design doc promises (WAL, foreign keys, integrity check, round-trip
//! open/close, malformed-file refusal).

use std::path::PathBuf;

use concerto_persist::{Persistence, PersistenceConfig};

fn tmp_db() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("test.db");
    (dir, path)
}

#[tokio::test]
async fn open_then_shutdown_round_trip() {
    let (_dir, db_path) = tmp_db();
    let persist = Persistence::open(PersistenceConfig {
        db_path: db_path.clone(),
        max_readers: 2,
    })
    .await
    .expect("open");

    assert!(db_path.exists(), "db file created");

    persist.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn pragmas_match_design_doc() {
    let (_dir, db_path) = tmp_db();
    let persist = Persistence::open(PersistenceConfig {
        db_path,
        max_readers: 2,
    })
    .await
    .expect("open");

    let journal_mode = persist.journal_mode().await.expect("journal_mode");
    assert_eq!(
        journal_mode.to_ascii_lowercase(),
        "wal",
        "journal_mode = WAL is mandatory per design/09 §3.3"
    );

    let foreign_keys = persist.foreign_keys().await.expect("foreign_keys");
    assert!(
        foreign_keys,
        "foreign_keys = ON is mandatory per design/09 §3.3"
    );

    persist.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn creates_parent_directory_if_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Two levels of nesting that don't exist yet — open() must mkdir -p.
    let db_path = dir.path().join("nested").join("more").join("test.db");
    let persist = Persistence::open(PersistenceConfig {
        db_path: db_path.clone(),
        max_readers: 1,
    })
    .await
    .expect("open should create parent dirs");
    assert!(db_path.exists());
    persist.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn reopen_after_shutdown_preserves_db_file() {
    let (_dir, db_path) = tmp_db();

    let p1 = Persistence::open(PersistenceConfig {
        db_path: db_path.clone(),
        max_readers: 1,
    })
    .await
    .expect("first open");
    p1.shutdown().await.expect("shutdown");

    // Reopening the same file should succeed — verifies WAL doesn't leave
    // the DB in a state the next open chokes on.
    let p2 = Persistence::open(PersistenceConfig {
        db_path,
        max_readers: 1,
    })
    .await
    .expect("second open");
    p2.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn writer_guard_serializes_access() {
    let (_dir, db_path) = tmp_db();
    let persist = Persistence::open(PersistenceConfig {
        db_path,
        max_readers: 2,
    })
    .await
    .expect("open");

    // Hold the writer; verify a second writer().await actually blocks until
    // the first guard is dropped.
    let guard = persist.writer().await;

    let racer = async {
        // Don't await on `persist.writer()` directly inside select — the
        // guard would deadlock the outer scope. Use a short timeout to
        // observe blocking.
        let r = tokio::time::timeout(std::time::Duration::from_millis(50), persist.writer()).await;
        assert!(
            r.is_err(),
            "second writer should not acquire while first holds"
        );
    };
    racer.await;

    drop(guard);

    // After dropping, the writer should be acquirable again.
    let _next = tokio::time::timeout(std::time::Duration::from_millis(500), persist.writer())
        .await
        .expect("writer acquires after prior guard drops");

    // `_next` drops here — keep `persist` reachable until then.
    drop(_next);
    persist.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn refuses_to_open_corrupt_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("corrupt.db");

    // Write garbage bytes that don't form a valid SQLite header.
    tokio::fs::write(&db_path, b"not a real sqlite database, just bytes")
        .await
        .expect("write garbage");

    let result = Persistence::open(PersistenceConfig {
        db_path,
        max_readers: 1,
    })
    .await;

    assert!(
        result.is_err(),
        "open on a malformed file must fail (either at connect, migrate, or quick_check)"
    );
}
