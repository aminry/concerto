//! Integration tests for the Task 110 startup hardening guards.
//!
//! These tests treat `Persistence::open` as the boundary and assert the two
//! boot guards `design/09` calls for:
//!
//! - a corrupt DB fails `PRAGMA quick_check` on open → [`Error::DatabaseCorrupt`]
//!   (before the migrator touches the file);
//! - a DB stamped with a schema version higher than the binary's max-known
//!   migration triggers the downgrade refusal → [`Error::SchemaDowngrade`];
//! - a normal/fresh DB boots and migrates as before.
//!
//! They reuse the existing persist test style (a tempdir + `PersistenceConfig`).

use std::path::PathBuf;

use concerto_error::Error;
use concerto_persist::{Persistence, PersistenceConfig};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection};

fn tmp_db() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("test.db");
    (dir, path)
}

/// A normal/fresh DB boots and migrates as before — the guards are no-ops on
/// the happy path.
#[tokio::test]
async fn fresh_db_boots_and_migrates() {
    let (_dir, db_path) = tmp_db();
    let persist = Persistence::open(PersistenceConfig {
        db_path: db_path.clone(),
        max_readers: 2,
    })
    .await
    .expect("fresh DB opens cleanly");

    assert!(db_path.exists(), "db file created");

    // Sanity: the migrator ran (the initial schema is present).
    let mut w = persist.writer().await;
    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?")
            .bind("projects")
            .fetch_one(&mut *w)
            .await
            .expect("count projects table");
    assert_eq!(n, 1, "migrations applied on a fresh DB");
    drop(w);

    persist.shutdown().await.expect("shutdown");
}

/// A corrupt DB file fails `PRAGMA quick_check` on open and surfaces the
/// frozen [`Error::DatabaseCorrupt`] variant — before the migrator runs.
#[tokio::test]
async fn corrupt_db_fails_with_database_corrupt() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("corrupt.db");

    // Build a file with a VALID SQLite header (so `connect()` succeeds) but a
    // corrupted page body, so the failure lands on `quick_check` rather than
    // at connect time. Start from a real, migrated DB, then clobber the
    // middle of the file with garbage.
    {
        let opts = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true);
        let mut conn = opts.connect().await.expect("seed connect");
        // A couple of pages of real content so there's a body to corrupt.
        sqlx::query("CREATE TABLE t (id INTEGER PRIMARY KEY, blob TEXT)")
            .execute(&mut conn)
            .await
            .expect("create table");
        for i in 0..256 {
            sqlx::query("INSERT INTO t (id, blob) VALUES (?, ?)")
                .bind(i)
                .bind("x".repeat(200))
                .execute(&mut conn)
                .await
                .expect("insert");
        }
        conn.close().await.expect("close seed conn");
    }

    // Corrupt the file body while preserving the first page (the header +
    // schema root live in page 1). Overwrite a span well past the header.
    let mut bytes = std::fs::read(&db_path).expect("read db");
    assert!(bytes.len() > 8192, "seeded db should be multiple pages");
    for b in bytes.iter_mut().skip(4096).take(4096) {
        *b = 0xFF;
    }
    std::fs::write(&db_path, &bytes).expect("write corrupted db");

    let result = Persistence::open(PersistenceConfig {
        db_path: db_path.clone(),
        max_readers: 1,
    })
    .await;

    match result {
        Err(Error::DatabaseCorrupt(msg)) => {
            assert!(
                msg.contains(&db_path.display().to_string()),
                "error message names the DB path; got: {msg}"
            );
        }
        Err(other) => panic!("expected Error::DatabaseCorrupt, got: {other:?}"),
        Ok(_) => panic!("expected open to fail on a corrupt DB"),
    }
}

/// A DB stamped with a schema version higher than the binary's max-known
/// migration triggers the downgrade refusal — [`Error::SchemaDowngrade`],
/// naming both versions.
#[tokio::test]
async fn future_schema_version_refuses_downgrade() {
    let (_dir, db_path) = tmp_db();

    // First open: migrate the DB up to the binary's current max, creating the
    // `_sqlx_migrations` table.
    let persist = Persistence::open(PersistenceConfig {
        db_path: db_path.clone(),
        max_readers: 1,
    })
    .await
    .expect("first open migrates");
    persist.shutdown().await.expect("shutdown");

    // Stamp a future migration version directly into `_sqlx_migrations`,
    // simulating a DB written by a newer Core. We pick a version far above any
    // plausible binary_max so the test never collides with new migrations.
    let future_version: i64 = 999_999;
    {
        let opts = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(false);
        let mut conn = opts.connect().await.expect("reconnect to stamp");
        // Mirror sqlx's `_sqlx_migrations` columns (version, description,
        // installed_on, success, checksum, execution_time).
        sqlx::query(
            "INSERT INTO _sqlx_migrations \
             (version, description, installed_on, success, checksum, execution_time) \
             VALUES (?, ?, CURRENT_TIMESTAMP, 1, ?, 0)",
        )
        .bind(future_version)
        .bind("future migration from a newer Core")
        .bind(vec![0u8; 32])
        .execute(&mut conn)
        .await
        .expect("stamp future migration");
        conn.close().await.expect("close stamp conn");
    }

    let result = Persistence::open(PersistenceConfig {
        db_path,
        max_readers: 1,
    })
    .await;

    match result {
        Err(Error::SchemaDowngrade(msg)) => {
            assert!(
                msg.contains(&future_version.to_string()),
                "error names the DB schema version; got: {msg}"
            );
        }
        Err(other) => panic!("expected Error::SchemaDowngrade, got: {other:?}"),
        Ok(_) => panic!("expected open to refuse a future schema version"),
    }
}
