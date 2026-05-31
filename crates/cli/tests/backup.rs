//! Integration test for `concerto backup` (Task 111).
//!
//! This test is **fully cross-platform** — it builds and runs on the Windows
//! CI lane. It does NOT use `concerto-test-harness` (which is Unix-only: it
//! binds a UDS) and it does NOT need a running Core. Instead it:
//!
//!   1. Seeds a real, migrated SQLite DB by calling `concerto-persist`
//!      (`Persistence::open` on a temp path creates + migrates the DB and runs
//!      `PRAGMA quick_check`), exactly as the Core does.
//!   2. Plants a small audit JSONL with records straddling a date range.
//!   3. Runs the shipped `concerto` binary's `backup` subcommand against that
//!      scratch data dir (pointed at via `$CONCERTO_DATA_DIR` on the child
//!      process — no process-global env mutation, so libtest's parallel runner
//!      can't race it).
//!   4. Asserts the snapshot opens + `PRAGMA quick_check`s ok, the manifest is
//!      correct, and the audit range was filtered to exactly the in-range
//!      records.

use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection};

/// Seed a migrated SQLite DB at `<data_dir>/concerto.db` via `concerto-persist`
/// (the same open/migrate path the Core uses). Returns nothing — the file is
/// left on disk for the backup to snapshot.
async fn seed_db(data_dir: &Path) {
    let db_path = data_dir.join("concerto.db");
    let cfg = concerto_persist::PersistenceConfig {
        db_path: db_path.clone(),
        max_readers: 2,
    };
    let persist = concerto_persist::Persistence::open(cfg)
        .await
        .expect("open + migrate the seed DB");
    // Cleanly close so the WAL is checkpointed and the file is a complete DB.
    persist.shutdown().await.expect("shutdown seed persistence");
    assert!(db_path.exists(), "seed DB file should exist after open");
}

/// Plant a small audit JSONL in `<data_dir>/audit/` with three records on
/// different days, matching the Core's `at` format (`YYYY-MM-DDTHH:MM:SS.mmmZ`).
fn seed_audit(data_dir: &Path) {
    let audit_dir = data_dir.join("audit");
    std::fs::create_dir_all(&audit_dir).expect("create audit dir");
    // Two daily files (rotation), three records total.
    std::fs::write(
        audit_dir.join("audit-2026-05-29.jsonl"),
        "{\"at\":\"2026-05-29T10:00:00.000Z\",\"kind\":\"before_range\"}\n",
    )
    .expect("write day-1 audit");
    std::fs::write(
        audit_dir.join("audit-2026-05-30.jsonl"),
        concat!(
            "{\"at\":\"2026-05-30T09:00:00.000Z\",\"kind\":\"in_range\"}\n",
            "{\"at\":\"2026-05-31T23:59:59.000Z\",\"kind\":\"after_range\"}\n",
        ),
    )
    .expect("write day-2 audit");
}

/// Run the built `concerto` binary's `backup` subcommand with the given args,
/// pointing `$CONCERTO_DATA_DIR` at `data_dir`. Returns the captured stdout.
fn run_backup(data_dir: &Path, args: &[&str]) -> Vec<u8> {
    let mut cmd = Command::cargo_bin("concerto").expect("locate the built `concerto` binary");
    cmd.env("CONCERTO_DATA_DIR", data_dir)
        // Defend against a developer-set CONCERTO_DB_PATH leaking in and
        // redirecting the snapshot away from the seeded file.
        .env_remove("CONCERTO_DB_PATH")
        .env_remove("CONCERTO_HOME")
        .arg("backup");
    cmd.args(args);
    cmd.assert().success().get_output().stdout.clone()
}

/// Re-open a SQLite DB read-only and assert `PRAGMA quick_check` returns "ok".
async fn assert_quick_check_ok(db_path: &Path) {
    let mut conn = SqliteConnectOptions::new()
        .filename(db_path)
        .read_only(true)
        .create_if_missing(false)
        .connect()
        .await
        .expect("open the snapshot read-only");
    let result: String = sqlx::query_scalar("PRAGMA quick_check")
        .fetch_one(&mut conn)
        .await
        .expect("run PRAGMA quick_check on the snapshot");
    conn.close().await.expect("close snapshot connection");
    assert_eq!(result, "ok", "snapshot should pass PRAGMA quick_check");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backup_snapshots_db_and_filters_audit_range() {
    let scratch = tempfile::tempdir().expect("scratch tempdir");
    let data_dir = scratch.path().join("concerto");
    std::fs::create_dir_all(&data_dir).expect("create scratch data dir");

    seed_db(&data_dir).await;
    seed_audit(&data_dir);

    let out_dir = scratch.path().join("backup-out");

    // Run with an audit range that includes ONLY the 2026-05-30 record.
    let stdout = tokio::task::spawn_blocking({
        let data_dir = data_dir.clone();
        let out_dir = out_dir.clone();
        move || {
            run_backup(
                &data_dir,
                &[
                    "--out",
                    out_dir.to_str().unwrap(),
                    "--audit-from",
                    "2026-05-30",
                    "--audit-to",
                    "2026-05-30T23:59:59.999Z",
                    "--json",
                ],
            )
        }
    })
    .await
    .expect("join blocking backup command");

    // The snapshot exists and is a valid SQLite DB.
    let snapshot = out_dir.join("concerto.db");
    assert!(snapshot.exists(), "snapshot concerto.db should exist");
    assert_quick_check_ok(&snapshot).await;

    // The manifest exists and is correct.
    let manifest_path = out_dir.join("manifest.json");
    assert!(manifest_path.exists(), "manifest.json should exist");
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).expect("read manifest.json"))
            .expect("manifest.json parses");

    assert_eq!(manifest["manifest_version"], 1);
    assert!(
        manifest["created_at"]
            .as_str()
            .is_some_and(|s| s.ends_with('Z') && s.contains('T')),
        "created_at should be UTC ISO-8601; got {:?}",
        manifest["created_at"]
    );
    assert_eq!(manifest["included"]["db_snapshot"], "concerto.db");
    // No --with-worktrees, so no tar.
    assert!(manifest["included"]["worktrees_tar"].is_null());
    assert_eq!(manifest["included"]["audit_jsonl"], "audit.jsonl");
    assert_eq!(manifest["included"]["audit_from"], "2026-05-30");
    assert_eq!(
        manifest["included"]["audit_records"], 1,
        "exactly one audit record falls in [2026-05-30, 2026-05-30T23:59:59.999Z]"
    );

    // The exported audit.jsonl carries exactly the in-range record.
    let audit_out = out_dir.join("audit.jsonl");
    let audit_body = std::fs::read_to_string(&audit_out).expect("read audit.jsonl");
    let lines: Vec<&str> = audit_body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    assert_eq!(lines.len(), 1, "exactly one record exported; got {lines:?}");
    assert!(
        lines[0].contains("in_range"),
        "exported record should be the in-range one; got {}",
        lines[0]
    );
    assert!(
        !audit_body.contains("before_range") && !audit_body.contains("after_range"),
        "out-of-range records must not be exported"
    );

    // The --json stdout is the manifest.
    let stdout_json: Value = serde_json::from_slice(&stdout).expect("--json stdout parses");
    assert_eq!(stdout_json["included"]["audit_records"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backup_with_worktrees_produces_tar() {
    let scratch = tempfile::tempdir().expect("scratch tempdir");
    let data_dir = scratch.path().join("concerto");
    std::fs::create_dir_all(&data_dir).expect("create scratch data dir");

    seed_db(&data_dir).await;

    // Plant a worktree tree with a file to archive.
    let wt = data_dir.join("workspaces").join("demo").join("bach");
    std::fs::create_dir_all(&wt).expect("create worktree dir");
    std::fs::write(wt.join("hello.txt"), b"hello worktree").expect("write worktree file");

    let out_dir = scratch.path().join("backup-out");

    tokio::task::spawn_blocking({
        let data_dir = data_dir.clone();
        let out_dir = out_dir.clone();
        move || {
            run_backup(
                &data_dir,
                &["--out", out_dir.to_str().unwrap(), "--with-worktrees"],
            )
        }
    })
    .await
    .expect("join blocking backup command");

    // Snapshot still valid.
    assert_quick_check_ok(&out_dir.join("concerto.db")).await;

    // The tar exists, is non-empty, and contains the planted file path.
    let tar_path = out_dir.join("worktrees.tar");
    assert!(tar_path.exists(), "worktrees.tar should exist");
    let tar_bytes = std::fs::read(&tar_path).expect("read worktrees.tar");
    assert!(!tar_bytes.is_empty(), "worktrees.tar should not be empty");

    let mut archive = tar::Archive::new(std::io::Cursor::new(tar_bytes));
    let mut found_hello = false;
    for entry in archive.entries().expect("read tar entries") {
        let entry = entry.expect("tar entry");
        let path = entry.path().expect("tar entry path");
        if path
            .to_string_lossy()
            .replace('\\', "/")
            .ends_with("bach/hello.txt")
        {
            found_hello = true;
        }
    }
    assert!(found_hello, "tar should contain the planted worktree file");

    // No audit range requested → no audit.jsonl.
    assert!(
        !out_dir.join("audit.jsonl").exists(),
        "no audit range was requested, so audit.jsonl must be absent"
    );

    // Manifest records the tar and no audit.
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(out_dir.join("manifest.json")).expect("read manifest"),
    )
    .expect("parse manifest");
    assert_eq!(manifest["included"]["worktrees_tar"], "worktrees.tar");
    assert!(manifest["included"]["audit_jsonl"].is_null());
    assert_eq!(manifest["included"]["audit_records"], 0);
}
