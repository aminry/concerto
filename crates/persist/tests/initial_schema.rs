//! Integration tests for the 0001_initial_schema migration.
//!
//! These tests open a fresh tempdir DB via `Persistence::open` (which runs
//! `sqlx::migrate!`), then assert the schema the migration was supposed to
//! produce: every named table, every named index, foreign-key enforcement,
//! and round-trip insert/read on each table.
//!
//! Anything the design doc (`design/09_Persistence.md §4`) names is asserted
//! here. If a future migration drops or renames one of these, the test
//! breaks loudly — which is the point.

use std::collections::HashSet;
use std::path::PathBuf;

use concerto_persist::{Persistence, PersistenceConfig};
use sqlx::Row;

fn tmp_db() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("test.db");
    (dir, path)
}

async fn fresh_db() -> (tempfile::TempDir, Persistence) {
    let (dir, db_path) = tmp_db();
    let persist = Persistence::open(PersistenceConfig {
        db_path,
        max_readers: 2,
    })
    .await
    .expect("open");
    (dir, persist)
}

/// Every table the design doc names in §4.1, §4.2, §4.4.
/// NOTE: `projects` is intentionally absent — it was removed in the
/// Project→Workspace collapse (2026-06-08).
const EXPECTED_TABLES: &[&str] = &[
    "repositories",
    "workspaces",
    "workspace_repos",
    "workareas",
    "workarea_repos",
    "chats",
    "chat_messages",
    "sessions",
    "checkpoints",
    "tool_approvals",
    "devices",
];

/// Every index the design doc names.
const EXPECTED_INDEXES: &[&str] = &[
    "idx_workspace_repos_position",
    "idx_workareas_status",
    "idx_workareas_workspace",
    "idx_chat_messages_chat",
    "idx_sessions_workarea",
    "idx_sessions_status",
    "idx_sessions_yolo",
    "idx_checkpoints_workarea",
    "idx_devices_active",
];

#[tokio::test]
async fn every_expected_table_exists() {
    let (_dir, persist) = fresh_db().await;
    let mut guard = persist.writer().await;

    let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type='table'")
        .fetch_all(&mut *guard)
        .await
        .expect("query sqlite_master tables");

    let found: HashSet<String> = rows.into_iter().map(|r| r.get::<String, _>(0)).collect();

    for table in EXPECTED_TABLES {
        assert!(
            found.contains(*table),
            "missing table `{table}` (found: {found:?})"
        );
    }

    // `_sqlx_migrations` is sqlx-internal but worth asserting — its presence
    // proves the migration ran rather than the tables coming from somewhere else.
    assert!(
        found.contains("_sqlx_migrations"),
        "expected sqlx migration tracking table"
    );
}

#[tokio::test]
async fn every_expected_index_exists() {
    let (_dir, persist) = fresh_db().await;
    let mut guard = persist.writer().await;

    let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type='index'")
        .fetch_all(&mut *guard)
        .await
        .expect("query sqlite_master indexes");

    let found: HashSet<String> = rows.into_iter().map(|r| r.get::<String, _>(0)).collect();

    for index in EXPECTED_INDEXES {
        assert!(
            found.contains(*index),
            "missing index `{index}` (found: {found:?})"
        );
    }
}

/// Insert a representative row into every table and read it back. The point
/// is to catch CHECK constraints, NOT NULL violations, and column-order /
/// type drift between the migration and what callers actually pass in.
#[tokio::test]
async fn insert_and_read_back_every_table() {
    let (_dir, persist) = fresh_db().await;
    let mut w = persist.writer().await;

    // ----- repositories -----------------------------------------------------
    sqlx::query(
        "INSERT INTO repositories \
         (id, name, url, local_path, clone_strategy, default_branch, \
          cone_defaults_json, fs_monitor_pid, last_fetch_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("repo-1")
    .bind("marketplace-api")
    .bind("git@github.com:example/marketplace.git")
    .bind("/tmp/concerto/repos/repo-1")
    .bind("full")
    .bind("main")
    .bind("[]")
    .bind(Option::<i64>::None)
    .bind(Option::<i64>::None)
    .execute(&mut *w)
    .await
    .expect("insert repositories");

    // ----- workspaces -------------------------------------------------------
    sqlx::query(
        "INSERT INTO workspaces \
         (id, name, slug, icon, description, permission_mode, \
          bypass_destructive_guard, settings_json, created_at, archived_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("ws-1")
    .bind("Idempotency keys for payments")
    .bind("idempotency-keys")
    .bind(Option::<String>::None)
    .bind(Option::<String>::None)
    .bind(Option::<String>::None) // NULL = inherit from workspace per design/03 §3.2
    .bind(Option::<i64>::None)
    .bind("{}")
    .bind(1_700_000_001_000_i64)
    .bind(Option::<i64>::None)
    .execute(&mut *w)
    .await
    .expect("insert workspaces");

    // ----- workspace_repos --------------------------------------------------
    sqlx::query("INSERT INTO workspace_repos (workspace_id, repository_id) VALUES (?, ?)")
        .bind("ws-1")
        .bind("repo-1")
        .execute(&mut *w)
        .await
        .expect("insert workspace_repos");

    // ----- workareas --------------------------------------------------------
    sqlx::query(
        "INSERT INTO workareas \
         (id, workspace_id, composer_name, branch_name, worktree_root, status, \
          permission_mode, bypass_destructive_guard, created_at, archived_at, \
          last_activity_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("wa-1")
    .bind("ws-1")
    .bind("bach")
    .bind("amin/idempotency-keys")
    .bind("/tmp/concerto/workspaces/idempotency-keys/bach")
    .bind("created")
    .bind(Option::<String>::None)
    .bind(Option::<i64>::None)
    .bind(1_700_000_002_000_i64)
    .bind(Option::<i64>::None)
    .bind(Option::<i64>::None)
    .execute(&mut *w)
    .await
    .expect("insert workareas");

    // ----- workarea_repos ---------------------------------------------------
    sqlx::query(
        "INSERT INTO workarea_repos \
         (workarea_id, repository_id, worktree_path, branch_override, sparse_cones_json) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind("wa-1")
    .bind("repo-1")
    .bind("/tmp/concerto/workspaces/idempotency-keys/bach/marketplace-api")
    .bind(Option::<String>::None)
    .bind("[]")
    .execute(&mut *w)
    .await
    .expect("insert workarea_repos");

    // ----- chats + sessions: insert chat with NULL session_id first
    // (kind='maestro' is the carve-out), so we don't hit the
    // chats.session_id → sessions(id) FK before sessions exists.
    sqlx::query("INSERT INTO chats (id, session_id, kind, created_at) VALUES (?, ?, ?, ?)")
        .bind("chat-maestro")
        .bind(Option::<String>::None)
        .bind("maestro")
        .bind(1_700_000_003_000_i64)
        .execute(&mut *w)
        .await
        .expect("insert maestro chat");

    // Now insert a session-kind chat (still allowed before sessions row
    // exists, because session_id is NULL here — we'll wire a real session
    // chat below by inserting the session first).
    sqlx::query(
        "INSERT INTO sessions \
         (id, workarea_id, chat_id, agent_kind, agent_version, model, mode, \
          host_pid, host_socket, pty_cookie, external_session_id, \
          permission_mode, bypass_destructive_guard, started_at, ended_at, \
          last_heartbeat, status) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("sess-1")
    .bind("wa-1")
    .bind("chat-maestro") // any existing chat is fine for the FK
    .bind("claude")
    .bind(Option::<String>::None)
    .bind(Option::<String>::None)
    .bind(Option::<String>::None)
    .bind(Option::<i64>::None)
    .bind(Option::<String>::None)
    .bind(Option::<Vec<u8>>::None)
    .bind(Option::<String>::None)
    .bind("normal")
    .bind(0_i64)
    .bind(1_700_000_004_000_i64)
    .bind(Option::<i64>::None)
    .bind(Option::<i64>::None)
    .bind("starting")
    .execute(&mut *w)
    .await
    .expect("insert sessions");

    // Now a session-kind chat that references the session.
    sqlx::query("INSERT INTO chats (id, session_id, kind, created_at) VALUES (?, ?, ?, ?)")
        .bind("chat-sess-1")
        .bind("sess-1")
        .bind("session")
        .bind(1_700_000_005_000_i64)
        .execute(&mut *w)
        .await
        .expect("insert session chat");

    // ----- chat_messages ----------------------------------------------------
    sqlx::query(
        "INSERT INTO chat_messages \
         (id, chat_id, role, content_json, created_at, parent_id, superseded_by) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("msg-1")
    .bind("chat-sess-1")
    .bind("user")
    .bind("{\"text\":\"hello\"}")
    .bind(1_700_000_006_000_i64)
    .bind(Option::<String>::None)
    .bind(Option::<String>::None)
    .execute(&mut *w)
    .await
    .expect("insert chat_messages");

    // ----- checkpoints ------------------------------------------------------
    sqlx::query(
        "INSERT INTO checkpoints \
         (id, workarea_id, repository_id, chat_message_id, git_ref, created_at, \
          diff_stats_json) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("ck-1")
    .bind("wa-1")
    .bind("repo-1")
    .bind("msg-1")
    .bind("refs/concerto/checkpoints/wa-1/repo-1/0")
    .bind(1_700_000_007_000_i64)
    .bind(Option::<String>::None)
    .execute(&mut *w)
    .await
    .expect("insert checkpoints");

    // ----- devices ---------------------------------------------------------
    sqlx::query(
        "INSERT INTO devices \
         (id, name, public_key, paired_at, last_seen_at, revoked_at, push_token, \
          push_platform) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("dev-1")
    .bind("Amin's iPhone")
    .bind(vec![0u8; 32])
    .bind(1_700_000_008_000_i64)
    .bind(Option::<i64>::None)
    .bind(Option::<i64>::None)
    .bind(Option::<String>::None)
    .bind(Option::<String>::None)
    .execute(&mut *w)
    .await
    .expect("insert devices");

    // ----- tool_approvals ---------------------------------------------------
    sqlx::query(
        "INSERT INTO tool_approvals \
         (id, session_id, tool_name, payload_json, requested_at, decided_at, \
          decided_by_device_id, decision) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("ta-1")
    .bind("sess-1")
    .bind("Bash")
    .bind("{\"cmd\":\"ls\"}")
    .bind(1_700_000_009_000_i64)
    .bind(Option::<i64>::None)
    .bind(Option::<String>::None)
    .bind(Option::<String>::None)
    .execute(&mut *w)
    .await
    .expect("insert tool_approvals");

    // ----- read back each ---------------------------------------------------
    // One representative `SELECT` per table catches column-name drift.
    let counts: Vec<(&str, &str)> = vec![
        ("repositories", "repo-1"),
        ("workspaces", "ws-1"),
        ("workareas", "wa-1"),
        ("sessions", "sess-1"),
        ("chats", "chat-sess-1"),
        ("chat_messages", "msg-1"),
        ("checkpoints", "ck-1"),
        ("devices", "dev-1"),
        ("tool_approvals", "ta-1"),
    ];
    for (table, id) in counts {
        let sql = format!("SELECT id FROM {table} WHERE id = ?");
        let row = sqlx::query(&sql)
            .bind(id)
            .fetch_one(&mut *w)
            .await
            .unwrap_or_else(|e| panic!("read back {table}/{id}: {e}"));
        assert_eq!(row.get::<String, _>(0), id, "{table} id round-trip");
    }

    // ----- read back composite-PK tables ------------------------------------
    let n_wr: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workspace_repos WHERE workspace_id = ? AND repository_id = ?",
    )
    .bind("ws-1")
    .bind("repo-1")
    .fetch_one(&mut *w)
    .await
    .expect("read workspace_repos");
    assert_eq!(n_wr, 1);

    let n_war: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM workarea_repos WHERE workarea_id = ? AND repository_id = ?",
    )
    .bind("wa-1")
    .bind("repo-1")
    .fetch_one(&mut *w)
    .await
    .expect("read workarea_repos");
    assert_eq!(n_war, 1);
}

/// Foreign-key enforcement must fire on `workareas.workspace_id`. This
/// catches the foot-gun where someone forgets `PRAGMA foreign_keys = ON`
/// — the constraint is declared in the migration but SQLite ignores it
/// unless the connection enabled the pragma. Task 08 enables it; this
/// test verifies it actually works end-to-end.
#[tokio::test]
async fn foreign_key_violation_is_rejected() {
    let (_dir, persist) = fresh_db().await;
    let mut w = persist.writer().await;

    let result = sqlx::query(
        "INSERT INTO workareas \
         (id, workspace_id, composer_name, branch_name, worktree_root, status, \
          permission_mode, bypass_destructive_guard, created_at, archived_at, \
          last_activity_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("wa-orphan")
    .bind("ws-does-not-exist") // <-- the FK violation
    .bind("bach")
    .bind("amin/test")
    .bind("/tmp/whatever")
    .bind("created")
    .bind(Option::<String>::None)
    .bind(Option::<i64>::None)
    .bind(1_700_000_000_000_i64)
    .bind(Option::<i64>::None)
    .bind(Option::<i64>::None)
    .execute(&mut *w)
    .await;

    assert!(
        result.is_err(),
        "FK violation on workareas.workspace_id must be rejected; got Ok"
    );
    let err = result.unwrap_err().to_string().to_lowercase();
    assert!(
        err.contains("foreign key") || err.contains("constraint"),
        "expected foreign-key error, got: {err}"
    );
}

/// CHECK constraint enforcement on `workareas.status`. The proto field is
/// `string` (any value over the wire), but the DB enforces the allowed set
/// so callers can't persist a typo.
#[tokio::test]
async fn invalid_workarea_status_is_rejected() {
    let (_dir, persist) = fresh_db().await;
    let mut w = persist.writer().await;

    // Set up parent workspace row first (no project needed after the collapse).
    sqlx::query("INSERT INTO workspaces (id, name, slug, created_at) VALUES (?, ?, ?, ?)")
        .bind("w")
        .bind("w")
        .bind("w")
        .bind(0_i64)
        .execute(&mut *w)
        .await
        .expect("workspace");

    let result = sqlx::query(
        "INSERT INTO workareas \
         (id, workspace_id, composer_name, branch_name, worktree_root, status, \
          created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("wa-bad")
    .bind("w")
    .bind("c")
    .bind("b")
    .bind("/tmp/x")
    .bind("not-a-real-status") // <-- the CHECK violation
    .bind(0_i64)
    .execute(&mut *w)
    .await;

    assert!(
        result.is_err(),
        "invalid workarea.status must be rejected; got Ok"
    );
}

/// Running migrations a second time on the same DB is a no-op (idempotent).
/// This is the design/09 §6.2 promise: forward-only and re-applying is safe.
#[tokio::test]
async fn migrations_are_idempotent_across_reopens() {
    let (_dir, db_path) = tmp_db();

    let p1 = Persistence::open(PersistenceConfig {
        db_path: db_path.clone(),
        max_readers: 1,
    })
    .await
    .expect("first open");
    p1.shutdown().await.expect("shutdown");

    // Second open on the same file: migrations table records 0001 as already
    // applied; the runner does nothing. If sqlx::migrate were not idempotent
    // it would either fail (constraint violation on CREATE TABLE) or attempt
    // to re-apply.
    let p2 = Persistence::open(PersistenceConfig {
        db_path,
        max_readers: 1,
    })
    .await
    .expect("second open is no-op");

    // Sanity: the schema is still intact after the reopen.
    let mut w = p2.writer().await;
    let n_tables: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?")
            .bind("workspaces")
            .fetch_one(&mut *w)
            .await
            .expect("count workspaces table");
    assert_eq!(n_tables, 1);
    drop(w);

    p2.shutdown().await.expect("shutdown");
}

/// After the Project→Workspace collapse (2026-06-08), the `projects` table
/// must not exist. Any code that still creates it is a regression.
#[tokio::test]
async fn schema_has_no_projects_table() {
    let (_dir, persist) = fresh_db().await;
    let mut w = persist.writer().await;
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='projects'",
    )
    .fetch_one(&mut *w)
    .await
    .unwrap();
    assert_eq!(n, 0, "projects table must be gone after the collapse");
}

/// `workspace_repos` must carry both folded-in columns: `sparse_cones_json`
/// (from the Project→Workspace collapse, 2026-06-08) and `position` (folded
/// in from migration 0009 by the same collapse). Both columns are
/// load-bearing: `position` backs the `list_repos` ordering query and Task
/// 309's reference-repo lookup; `sparse_cones_json` backs per-(workspace, repo)
/// sparse cone configuration.
#[tokio::test]
async fn workspace_repos_has_folded_columns() {
    let (_dir, persist) = fresh_db().await;
    let mut w = persist.writer().await;
    let cols: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('workspace_repos')")
            .fetch_all(&mut *w)
            .await
            .unwrap();
    assert!(cols.iter().any(|c| c == "sparse_cones_json"));
    assert!(cols.iter().any(|c| c == "position"));
}
