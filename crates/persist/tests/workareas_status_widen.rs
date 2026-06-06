//! Tests for migration 0010 — the recreate-table widening of the
//! `workareas.status` CHECK to add `finished` + `partial` (Task 307).
//!
//! Two angles:
//! 1. Against a fully-migrated DB (`Persistence::open` runs every
//!    migration): the widened CHECK accepts `finished` + `partial` and
//!    still rejects garbage; the FK to `workspaces`, the
//!    `UNIQUE(workspace_id, composer_name)` constraint, and both indexes
//!    (`idx_workareas_status`, `idx_workareas_workspace`) survive the
//!    recreate.
//! 2. A standalone recreate against a manually-built *pre-0010* table,
//!    seeded with a row + a child `workarea_repos` row, proving the
//!    recreate-table SQL preserves every column's data and the child FK
//!    reference (the on-upgrade data-preservation path migration 0010
//!    runs in production).

use concerto_persist::{
    workareas, NewProject, NewRepository, NewWorkarea, NewWorkspace, Persistence,
    PersistenceConfig, ProjectId, RepositoryId, WorkareaId, WorkspaceId,
};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection, Row};

async fn fresh_db() -> (tempfile::TempDir, Persistence) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let persist = Persistence::open(PersistenceConfig {
        db_path,
        max_readers: 2,
    })
    .await
    .expect("open");
    (dir, persist)
}

/// Seed project + repo + workspace, then a workarea, returning its id.
async fn seed_workarea(persist: &Persistence, status: &str) -> (WorkspaceId, WorkareaId) {
    let project_id = ProjectId("p1".to_string());
    let ws_id = WorkspaceId("ws1".to_string());
    let repo_id = RepositoryId("r1".to_string());
    let wa_id = WorkareaId(format!("wa-{status}"));

    let mut w = persist.writer().await;
    concerto_persist::projects::insert(
        &mut w,
        NewProject {
            id: project_id.clone(),
            name: "T".to_string(),
            icon: None,
            created_at: 1,
        },
    )
    .await
    .expect("project");
    // Idempotent-ish: ignore if the workspace/repo already exist (seed is
    // called once per test, but keep it robust).
    let _ = concerto_persist::repositories::insert(
        &mut w,
        NewRepository {
            id: repo_id.clone(),
            project_id: project_id.0.clone(),
            name: "r".to_string(),
            url: "file:///tmp/r.git".to_string(),
            local_path: "/tmp/r".to_string(),
            clone_strategy: "full".to_string(),
            default_branch: "main".to_string(),
        },
    )
    .await;
    concerto_persist::workspaces::insert(
        &mut w,
        NewWorkspace {
            id: ws_id.clone(),
            project_id: project_id.0.clone(),
            name: "W".to_string(),
            slug: "w".to_string(),
            description: None,
            permission_mode: None,
            created_at: 2,
        },
    )
    .await
    .expect("workspace");

    workareas::insert(
        &mut w,
        NewWorkarea {
            id: wa_id.clone(),
            workspace_id: ws_id.0.clone(),
            composer_name: format!("c-{status}"),
            branch_name: "concerto/c".to_string(),
            worktree_root: "/tmp/wt".to_string(),
            status: "created".to_string(),
            permission_mode: None,
            created_at: 3,
        },
    )
    .await
    .expect("insert workarea");
    drop(w);
    (ws_id, wa_id)
}

#[tokio::test]
async fn finished_and_partial_round_trip_through_widened_check() {
    let (_dir, persist) = fresh_db().await;
    let (_ws, wa) = seed_workarea(&persist, "rt").await;

    // The 0001 CHECK omitted both; the 0010 widen must accept them.
    for status in ["finished", "partial"] {
        let mut w = persist.writer().await;
        workareas::update_status(&mut w, &wa, status)
            .await
            .unwrap_or_else(|e| panic!("update_status({status}) must succeed post-0010: {e}"));
        drop(w);
        let row = workareas::get(persist.readers(), &wa)
            .await
            .expect("get")
            .expect("row");
        assert_eq!(row.status, status, "{status} must round-trip");
    }
}

#[tokio::test]
async fn widened_check_still_rejects_garbage() {
    let (_dir, persist) = fresh_db().await;
    let (_ws, wa) = seed_workarea(&persist, "gar").await;
    let mut w = persist.writer().await;
    let err = workareas::update_status(&mut w, &wa, "bogus_status").await;
    assert!(
        err.is_err(),
        "an out-of-set status must still violate the CHECK"
    );
}

#[tokio::test]
async fn fk_unique_and_indexes_survive_recreate() {
    let (_dir, persist) = fresh_db().await;
    let pool = persist.readers();

    // Both indexes still present on the recreated table.
    let idx: Vec<String> = sqlx::query(
        "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='workareas' ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .expect("idx query")
    .into_iter()
    .map(|r| r.get::<String, _>("name"))
    .collect();
    assert!(
        idx.contains(&"idx_workareas_status".to_string()),
        "idx_workareas_status must survive the recreate; got {idx:?}"
    );
    assert!(
        idx.contains(&"idx_workareas_workspace".to_string()),
        "idx_workareas_workspace must survive the recreate; got {idx:?}"
    );

    // FK to workspaces enforced: a workarea referencing a missing
    // workspace must fail (foreign_keys = ON on the writer).
    let mut w = persist.writer().await;
    let bad = workareas::insert(
        &mut w,
        NewWorkarea {
            id: WorkareaId("orphan".to_string()),
            workspace_id: "no-such-workspace".to_string(),
            composer_name: "x".to_string(),
            branch_name: "b".to_string(),
            worktree_root: "/tmp".to_string(),
            status: "active".to_string(),
            permission_mode: None,
            created_at: 1,
        },
    )
    .await;
    assert!(
        bad.is_err(),
        "FK to workspaces(id) must survive the recreate"
    );
    drop(w);

    // UNIQUE(workspace_id, composer_name) enforced.
    let (_ws, _wa) = seed_workarea(&persist, "uniq").await;
    let mut w = persist.writer().await;
    let dup = workareas::insert(
        &mut w,
        NewWorkarea {
            id: WorkareaId("dup".to_string()),
            workspace_id: "ws1".to_string(),
            composer_name: "c-uniq".to_string(), // same composer as seed
            branch_name: "b".to_string(),
            worktree_root: "/tmp".to_string(),
            status: "active".to_string(),
            created_at: 1,
            permission_mode: None,
        },
    )
    .await;
    assert!(
        dup.is_err(),
        "UNIQUE(workspace_id, composer_name) must survive the recreate"
    );
}

/// The on-upgrade path: build the *pre-0010* `workareas` table by hand
/// (FKs ON, with a child `workarea_repos` row), seed a fully-populated
/// row, run the exact 0010 migration SQL (the in-place `writable_schema`
/// CHECK rewrite), and assert every column's data AND the child FK row
/// survived — i.e. the widen did NOT cascade-delete children the way a
/// naive recreate-table-with-DROP would.
#[tokio::test]
async fn migration_0010_preserves_seeded_rows_and_child_fk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("pre0010.db");
    let mut conn = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        // Mirror production: foreign keys ON. The migration must suppress
        // the DROP-cascade itself (via `PRAGMA foreign_keys=OFF`), proving
        // the child rows survive even with enforcement on at connect time.
        .foreign_keys(true)
        .connect()
        .await
        .expect("connect");

    // Minimal parent + the PRE-0010 workareas table (0001 CHECK + 0002's
    // settings_json column, no `finished`/`partial`) + a child table.
    sqlx::raw_sql(
        "CREATE TABLE workspaces (id TEXT PRIMARY KEY);
         CREATE TABLE workareas (
             id TEXT PRIMARY KEY,
             workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
             composer_name TEXT NOT NULL,
             branch_name TEXT NOT NULL,
             worktree_root TEXT NOT NULL,
             status TEXT NOT NULL CHECK (status IN
                 ('created','active','running','awaiting','paused','archived','crashed')),
             permission_mode TEXT,
             bypass_destructive_guard INTEGER,
             created_at INTEGER NOT NULL,
             archived_at INTEGER,
             last_activity_at INTEGER,
             settings_json TEXT NOT NULL DEFAULT '{}',
             UNIQUE(workspace_id, composer_name)
         );
         CREATE INDEX idx_workareas_status ON workareas(status);
         CREATE INDEX idx_workareas_workspace ON workareas(workspace_id);
         CREATE TABLE workarea_repos (
             workarea_id TEXT NOT NULL REFERENCES workareas(id) ON DELETE CASCADE,
             repository_id TEXT NOT NULL,
             PRIMARY KEY (workarea_id, repository_id)
         );
         INSERT INTO workspaces (id) VALUES ('ws');
         INSERT INTO workareas
             (id, workspace_id, composer_name, branch_name, worktree_root, status,
              permission_mode, bypass_destructive_guard, created_at, archived_at,
              last_activity_at, settings_json)
         VALUES
             ('wa', 'ws', 'bach', 'concerto/bach', '/tmp/wt', 'running',
              'auto', 1, 100, NULL, 200, '{\"k\":1}');
         INSERT INTO workarea_repos (workarea_id, repository_id) VALUES ('wa', 'repo1');",
    )
    .execute(&mut conn)
    .await
    .expect("seed pre-0010");

    // Run the EXACT migration-0010 body.
    let sql = include_str!("../migrations/0010_workareas_status_finished_partial.sql");
    sqlx::raw_sql(sql)
        .execute(&mut conn)
        .await
        .expect("run 0010 recreate");

    // Every column of the seeded row survived intact.
    let row = sqlx::query(
        "SELECT id, workspace_id, composer_name, branch_name, worktree_root, status,
                permission_mode, bypass_destructive_guard, created_at, archived_at,
                last_activity_at, settings_json
         FROM workareas WHERE id = 'wa'",
    )
    .fetch_one(&mut conn)
    .await
    .expect("seeded row survives");
    assert_eq!(row.get::<String, _>("composer_name"), "bach");
    assert_eq!(row.get::<String, _>("branch_name"), "concerto/bach");
    assert_eq!(row.get::<String, _>("worktree_root"), "/tmp/wt");
    assert_eq!(row.get::<String, _>("status"), "running");
    assert_eq!(row.get::<String, _>("permission_mode"), "auto");
    assert_eq!(row.get::<i64, _>("bypass_destructive_guard"), 1);
    assert_eq!(row.get::<i64, _>("created_at"), 100);
    assert_eq!(row.get::<i64, _>("last_activity_at"), 200);
    assert_eq!(row.get::<String, _>("settings_json"), "{\"k\":1}");

    // The child workarea_repos row still references the (renamed) table.
    let (child,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM workarea_repos WHERE workarea_id = 'wa'")
            .fetch_one(&mut conn)
            .await
            .expect("child count");
    assert_eq!(
        child, 1,
        "child workarea_repos row must survive the recreate"
    );

    // The widened CHECK now accepts the two new values.
    for status in ["finished", "partial"] {
        sqlx::query("UPDATE workareas SET status = ? WHERE id = 'wa'")
            .bind(status)
            .execute(&mut conn)
            .await
            .unwrap_or_else(|e| panic!("post-recreate update to {status} must succeed: {e}"));
    }

    conn.close().await.expect("close");
}
