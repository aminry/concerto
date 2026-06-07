//! Task 308: the shared per-workarea `EditMutexRegistry` + multi-session
//! cardinality.
//!
//! In-process Tier-1 tests (no agent host needed — the registry is pure
//! in-memory concurrency state; the cardinality check seeds the
//! `sessions` table directly over a tempdir DB). Covers, per the task's
//! `Verification` + `design/03 §10` ("assert per-workarea edit mutex
//! serializes writes"):
//!
//! - two sessions on one workarea **serialize**: A holds the lock, B's
//!   write blocks then errors with `workarea.edit_mutex.blocked` naming A;
//! - a **concurrent read** (acquires nothing) proceeds while A holds the
//!   write lock — reads don't block;
//! - the guard **releases on drop** so a later write on B succeeds;
//! - the 10s timeout is **configurable down** for the test (short-timeout
//!   injection via the `timeout` argument);
//! - **two sessions both live on one workarea** (`list_live_ids_by_workarea`
//!   returns 2) — no server-side cardinality cap.

#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use concerto_core::workspace_manager::{
    is_write_class, EditMutexRegistry, EDIT_MUTEX_BLOCKED_WIRE_CODE,
};
use concerto_persist::{Persistence, PersistenceConfig, SessionId, WorkareaId};
use tempfile::TempDir;

fn wa(id: &str) -> WorkareaId {
    WorkareaId(id.to_string())
}
fn sid(id: &str) -> SessionId {
    SessionId(id.to_string())
}

/// Two sessions on the same workarea serialize their writes: A acquires
/// and holds the lock; B's write blocks for the (short) timeout, then
/// fails fast with the `workarea.edit_mutex.blocked` error naming A. The
/// blocked path does NOT queue — it returns an error.
#[tokio::test(flavor = "multi_thread")]
async fn two_sessions_serialize_b_blocks_then_errors_naming_a() {
    let reg = EditMutexRegistry::new();
    let w = wa("wa-shared");

    // Session A acquires the workarea edit lock and holds it.
    let guard_a = reg
        .acquire(&w, &sid("session-A"), Duration::from_secs(5))
        .await
        .expect("A acquires");
    assert_eq!(reg.holder(&w).await, Some(sid("session-A")));

    // Session B's write blocks then errors (short injected timeout).
    let blocked = reg
        .acquire(&w, &sid("session-B"), Duration::from_millis(80))
        .await
        .expect_err("B must be blocked while A holds the lock");
    assert_eq!(
        blocked.holder,
        Some(sid("session-A")),
        "blocked error must name the holder (A)"
    );
    let msg = blocked.to_string();
    assert!(
        msg.contains(EDIT_MUTEX_BLOCKED_WIRE_CODE),
        "error carries the typed wire-code: {msg}"
    );
    assert!(
        msg.contains("session-A"),
        "error names the holding session: {msg}"
    );

    // Holding A must not have changed.
    assert_eq!(reg.holder(&w).await, Some(sid("session-A")));
    drop(guard_a);
}

/// A read (which acquires NOTHING) proceeds concurrently while A holds
/// the write lock. We model "read" as simply not calling `acquire`; the
/// assertion is that B's *read* path can run to completion with no delay
/// while A's write guard is held. We additionally prove a write on a
/// *different* workarea is not serialized against A's lock.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_read_does_not_block_during_write() {
    let reg = Arc::new(EditMutexRegistry::new());
    let w = wa("wa-rw");

    // A holds the write lock for the whole test.
    let guard_a = reg
        .acquire(&w, &sid("A"), Duration::from_secs(5))
        .await
        .expect("A acquires");

    // A "read" by B touches no lock — it returns immediately even though
    // A holds the write lock. Model the read as a holder() query (a
    // read-class diagnostic) bounded by a tight timeout: it must resolve
    // far inside the budget because reads never await the inner lock.
    let read_result = tokio::time::timeout(Duration::from_millis(50), reg.holder(&w)).await;
    assert!(
        read_result.is_ok(),
        "a read (holder query) must not block on the held write lock"
    );
    assert_eq!(read_result.unwrap(), Some(sid("A")));

    // A write on a *different* workarea is also not serialized.
    let g2 = reg
        .acquire(&wa("wa-other"), &sid("B"), Duration::from_millis(50))
        .await
        .expect("a write on a different workarea is independent");
    drop(g2);
    drop(guard_a);
}

/// The guard releases on drop, so a later write on B succeeds (and the
/// holder flips A → B → unheld).
#[tokio::test(flavor = "multi_thread")]
async fn guard_release_on_drop_lets_later_write_succeed() {
    let reg = EditMutexRegistry::new();
    let w = wa("wa-release");

    let guard_a = reg
        .acquire(&w, &sid("A"), Duration::from_secs(5))
        .await
        .expect("A acquires");
    assert_eq!(reg.holder(&w).await, Some(sid("A")));

    // Drop A's guard — releases the lock + clears the holder.
    drop(guard_a);

    // B now acquires without blocking; the holder is B.
    let guard_b = reg
        .acquire(&w, &sid("B"), Duration::from_secs(5))
        .await
        .expect("B acquires after A releases");
    assert_eq!(reg.holder(&w).await, Some(sid("B")));

    drop(guard_b);
    // Fully released → no holder.
    assert_eq!(reg.holder(&w).await, None);
}

/// The acquisition timeout is configurable down for the test: a 1ms
/// timeout against a held lock errors essentially immediately (short
/// timeout injection), proving the `timeout` argument is honored rather
/// than a hardcoded 10s.
#[tokio::test(flavor = "multi_thread")]
async fn short_timeout_injection_fails_fast() {
    let reg = EditMutexRegistry::new();
    let w = wa("wa-timeout");
    let _guard_a = reg
        .acquire(&w, &sid("A"), Duration::from_secs(5))
        .await
        .expect("A acquires");

    let start = std::time::Instant::now();
    let blocked = reg
        .acquire(&w, &sid("B"), Duration::from_millis(1))
        .await
        .expect_err("B blocks");
    let elapsed = start.elapsed();
    assert_eq!(blocked.holder, Some(sid("A")));
    assert!(
        elapsed < Duration::from_secs(1),
        "a 1ms timeout must fail fast, took {elapsed:?}"
    );
}

/// The write-class set is exactly the frozen tools; reads + shell are
/// excluded so they acquire nothing.
#[test]
fn write_class_set_is_frozen() {
    for t in ["Write", "Edit", "MultiEdit", "NotebookEdit"] {
        assert!(is_write_class(t), "{t} must be write-class");
    }
    for t in ["Read", "Grep", "Glob", "Bash", "TodoWrite"] {
        assert!(!is_write_class(t), "{t} must NOT be write-class (no lock)");
    }
}

// ---------------------------------------------------------------------------
// Multi-session cardinality: two sessions both live on one workarea.
// ---------------------------------------------------------------------------

async fn open_persistence() -> (TempDir, Arc<Persistence>) {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("concerto.db");
    let persistence = Arc::new(
        Persistence::open(PersistenceConfig {
            db_path,
            max_readers: 2,
        })
        .await
        .expect("open"),
    );
    (tmp, persistence)
}

async fn seed_parents(persistence: &Persistence) {
    let mut w = persistence.writer().await;
    sqlx::query("INSERT INTO projects (id, name, created_at) VALUES ('p', 'p', 0)")
        .execute(&mut *w)
        .await
        .expect("project");
    sqlx::query(
        "INSERT INTO workspaces (id, project_id, name, slug, created_at) \
         VALUES ('ws', 'p', 'ws', 'ws', 0)",
    )
    .execute(&mut *w)
    .await
    .expect("workspace");
    sqlx::query(
        "INSERT INTO workareas (id, workspace_id, composer_name, branch_name, worktree_root, status, created_at) \
         VALUES ('wa', 'ws', 'bach', 'concerto/bach', '/tmp/wt/wa', 'active', 0)",
    )
    .execute(&mut *w)
    .await
    .expect("workarea");
}

/// Insert a live (`ended_at IS NULL`) session row on workarea `wa`.
async fn seed_live_session(persistence: &Persistence, session_id: &str) {
    let mut w = persistence.writer().await;
    // chats(id) is a NOT NULL FK on sessions; one maestro-kind chat row
    // satisfies the FK without the circular session↔chat reference.
    sqlx::query("INSERT OR IGNORE INTO chats (id, kind, created_at) VALUES (?, 'maestro', 0)")
        .bind(session_id)
        .execute(&mut *w)
        .await
        .expect("chat");
    sqlx::query(
        "INSERT INTO sessions (id, workarea_id, chat_id, agent_kind, status, started_at) \
         VALUES (?, 'wa', ?, 'claude', 'running', 0)",
    )
    .bind(session_id)
    .bind(session_id)
    .execute(&mut *w)
    .await
    .expect("session");
}

/// Two sessions can coexist on one workarea: `sessions.workarea_id` has
/// no cardinality guard, so inserting a second session on the same
/// workarea succeeds and `list_live_ids_by_workarea` returns both. This
/// is the union-of-sessions set 307's `finished` rule consumes and the
/// set the mutex's holder bookkeeping names — there is no server-side
/// session cap (`design/03 R-7`).
#[tokio::test(flavor = "multi_thread")]
async fn two_sessions_live_on_one_workarea() {
    let (_tmp, persistence) = open_persistence().await;
    seed_parents(&persistence).await;

    seed_live_session(&persistence, "sess-claude").await;
    seed_live_session(&persistence, "sess-codex").await;

    let live =
        concerto_persist::sessions::list_live_ids_by_workarea(persistence.readers(), &wa("wa"))
            .await
            .expect("list live");
    assert_eq!(
        live.len(),
        2,
        "both sessions must be live on the one workarea (no cardinality cap): {live:?}"
    );
    assert!(live.contains(&sid("sess-claude")));
    assert!(live.contains(&sid("sess-codex")));
}
