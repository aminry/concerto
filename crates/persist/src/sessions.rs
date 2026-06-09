//! `sessions` table CRUD (Task 22).
//!
//! Schema is locked by migration 0001 (Task 09):
//!
//! ```sql
//! CREATE TABLE sessions (
//!     id                          TEXT PRIMARY KEY,
//!     workarea_id                 TEXT NOT NULL REFERENCES workareas(id) ON DELETE CASCADE,
//!     chat_id                     TEXT NOT NULL REFERENCES chats(id),
//!     agent_kind                  TEXT NOT NULL CHECK (agent_kind IN ('claude','codex','gemini','maestro')),
//!     agent_version               TEXT,
//!     model                       TEXT,
//!     mode                        TEXT,
//!     host_pid                    INTEGER,
//!     host_socket                 TEXT,
//!     pty_cookie                  BLOB,
//!     external_session_id         TEXT,
//!     permission_mode             TEXT NOT NULL DEFAULT 'normal',
//!     bypass_destructive_guard    INTEGER NOT NULL DEFAULT 0,
//!     started_at                  INTEGER NOT NULL,
//!     ended_at                    INTEGER,
//!     last_heartbeat              INTEGER,
//!     status                      TEXT NOT NULL CHECK (status IN (
//!         'starting','running','awaiting','finished','crashed'
//!     ))
//! );
//! ```
//!
//! ## V0.1 notes (Task 22)
//!
//! - `chat_id` is `NOT NULL` with an FK to `chats(id)`. Because every
//!   session needs a `chats` row before the `sessions` row can exist,
//!   [`insert`] takes a `chat_id` parameter — the caller is responsible
//!   for inserting (or reusing) a row in the same transaction.
//!   [`insert_chat`] is provided so the Agent Supervisor can do both in
//!   one transaction inside a single `Connection::begin()` block.
//! - The `agent_kind` CHECK constraint accepts `claude|codex|gemini|maestro`.
//!   The Agent Supervisor's V0.1 "echo" kind is purely an in-process spawn
//!   mode (no DB column value); for echo-mode integration tests the row is
//!   seeded with `agent_kind = "claude"` so the CHECK constraint is
//!   satisfied. The kind stored on disk is decoupled from the wrapped
//!   binary path used at spawn.
//! - Status values: `'starting'` → `'running'` → `'finished'` (or
//!   `'crashed'`). The Agent Supervisor walks this state machine in
//!   `start_session` (`starting → running` after `Hello/Ready`) and
//!   `stop_session` (`running → finished`).
//!
//! ## V0.1 + Task 36 column additions
//!
//! Migration 0003 adds `last_acked_seq INTEGER NOT NULL DEFAULT 0`, the
//! persisted watermark of bytes the Core has consumed from the
//! agent-host's bridge ring buffer. The bridge pump writes it
//! opportunistically (~every 5s) so a crash loses at most that window;
//! `adopt_orphans` reads it on boot when reconnecting to surviving
//! hosts (`HostFrame::Hello { last_seq }`).

use concerto_error::{Error, Result};
use sqlx::{Connection, Row, SqliteConnection, SqlitePool};

use crate::api::{NewChat, NewSession, Session, SessionId, WorkareaId};

/// Insert a new `chats` row.
///
/// The Agent Supervisor calls this inside the same `begin()` transaction
/// as [`insert`] (the session) so the `sessions.chat_id` FK is satisfied
/// atomically.
pub async fn insert_chat(conn: &mut SqliteConnection, chat: NewChat) -> Result<String> {
    sqlx::query("INSERT INTO chats (id, session_id, kind, created_at) VALUES (?, ?, ?, ?)")
        .bind(&chat.id)
        .bind(&chat.session_id)
        .bind(&chat.kind)
        .bind(chat.created_at)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(chat.id)
}

/// Insert a new `sessions` row.
///
/// `pty_cookie` is stored as raw bytes (BLOB). `host_pid` and
/// `host_socket` start as `NULL` and are filled in by [`update_host`]
/// once `Hello/Ready` succeeds.
pub async fn insert(conn: &mut SqliteConnection, s: NewSession) -> Result<SessionId> {
    let id = s.id.clone();
    sqlx::query(
        "INSERT INTO sessions (
            id, workarea_id, chat_id, agent_kind, agent_version, model, mode,
            host_pid, host_socket, pty_cookie, external_session_id,
            permission_mode, bypass_destructive_guard,
            started_at, status, last_acked_seq
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id.0)
    .bind(&s.workarea_id.0)
    .bind(&s.chat_id)
    .bind(&s.agent_kind)
    .bind(&s.agent_version)
    .bind(&s.model)
    .bind(&s.mode)
    .bind(s.host_pid)
    .bind(&s.host_socket)
    .bind(&s.pty_cookie)
    .bind(&s.external_session_id)
    .bind(&s.permission_mode)
    .bind(s.bypass_destructive_guard as i64)
    .bind(s.started_at)
    .bind(&s.status)
    .bind(s.last_acked_seq)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(id)
}

/// Patch `host_pid`, `host_socket`, and `status` after the agent host
/// completes its `Hello/Ready` handshake. Called by the Agent Supervisor
/// inside `start_session` once the bridge connection is up.
pub async fn update_host(
    conn: &mut SqliteConnection,
    id: &SessionId,
    host_pid: i64,
    host_socket: &str,
    status: &str,
) -> Result<()> {
    sqlx::query("UPDATE sessions SET host_pid = ?, host_socket = ?, status = ? WHERE id = ?")
        .bind(host_pid)
        .bind(host_socket)
        .bind(status)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Task 36: persist the highest `seq` the Core has consumed from the
/// agent-host bridge ring buffer. Called from the read-pump's ack
/// scheduler (~every 5 s) so a Core crash loses at most that window of
/// ack progress. Cheap single-row UPDATE.
pub async fn update_last_acked(
    conn: &mut SqliteConnection,
    id: &SessionId,
    seq: i64,
) -> Result<()> {
    sqlx::query("UPDATE sessions SET last_acked_seq = ? WHERE id = ?")
        .bind(seq)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Task 37: persist the agent CLI's own session identifier (Claude /
/// Codex resume token). The Claude parser pack will extract this from
/// the agent's first banner once the V0.1 parser surfaces it; for now
/// the cold-resume RPC reads whatever value the column carries.
///
/// Passing `None` clears the column — useful for tests that want to
/// exercise the `session.no_external_id` error path.
pub async fn set_external_session_id(
    conn: &mut SqliteConnection,
    id: &SessionId,
    external_session_id: Option<&str>,
) -> Result<()> {
    sqlx::query("UPDATE sessions SET external_session_id = ? WHERE id = ?")
        .bind(external_session_id)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Update only the `status` column on a `sessions` row.
pub async fn update_status(
    conn: &mut SqliteConnection,
    id: &SessionId,
    status: &str,
) -> Result<()> {
    sqlx::query("UPDATE sessions SET status = ? WHERE id = ?")
        .bind(status)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Mark the session ended: set `ended_at` and transition to `finished`.
/// Idempotent.
pub async fn mark_ended(conn: &mut SqliteConnection, id: &SessionId, ended_at: i64) -> Result<()> {
    sqlx::query("UPDATE sessions SET ended_at = ?, status = 'finished' WHERE id = ?")
        .bind(ended_at)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Overwrite `sessions.permission_mode` for `id`. `mode` is one of
/// `strict|normal|auto|yolo` — sessions never carry NULL here. Task 32
/// uses this for `Sessions.UpdateSessionPermissionMode`.
pub async fn set_permission_mode(
    conn: &mut SqliteConnection,
    id: &SessionId,
    mode: &str,
) -> Result<()> {
    sqlx::query("UPDATE sessions SET permission_mode = ? WHERE id = ?")
        .bind(mode)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Hard-delete a session and everything that hangs off it, in one
/// transaction. Supports the destructive `Sessions.DeleteSession` RPC.
///
/// The cascade has two flavours of dependent, handled here in the only
/// order that satisfies the FK graph:
///
/// 1. **`schedule_runs.session_id`** → `REFERENCES sessions(id)` with no
///    on-delete action (RESTRICT). Deleting the session while a run still
///    points at it would be rejected. The column is nullable and the run
///    history is worth keeping (it records that a /loop fired), so we
///    NULL the link rather than delete the run.
/// 2. **`checkpoints.chat_message_id`** → `REFERENCES chat_messages(id)`
///    (RESTRICT). The session's `chats` (and their `chat_messages`)
///    cascade-delete via `chats.session_id ON DELETE CASCADE`, but those
///    cascading message deletes would be blocked by any checkpoint still
///    referencing them. So we delete the session's checkpoints first.
///
/// After the two blockers are released, deleting the `sessions` row lets
/// the automatic cascades fire: `chats` (→ `chat_messages`) and
/// `tool_approvals` all carry `ON DELETE CASCADE`.
///
/// Deleting a non-existent id is not an error (it just affects 0 rows).
///
/// Relies on `PRAGMA foreign_keys = ON`, which the persistence layer sets
/// on every connection (see [`crate::api`]); the cascades and RESTRICTs
/// above are inert without it.
pub async fn delete(conn: &mut SqliteConnection, id: &SessionId) -> Result<()> {
    let mut tx = conn.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;

    // 1. Unlink (don't delete) schedule_runs — preserve /loop run history.
    sqlx::query("UPDATE schedule_runs SET session_id = NULL WHERE session_id = ?")
        .bind(&id.0)
        .execute(&mut *tx)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;

    // 2. Delete checkpoints anchored to this session's chat messages before
    //    the chats/chat_messages cascade would trip the RESTRICT FK.
    sqlx::query(
        "DELETE FROM checkpoints WHERE chat_message_id IN (
            SELECT cm.id FROM chat_messages cm
            JOIN chats c ON cm.chat_id = c.id
            WHERE c.session_id = ?
         )",
    )
    .bind(&id.0)
    .execute(&mut *tx)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;

    // 3. Delete the session; chats (→ chat_messages) and tool_approvals
    //    cascade automatically.
    sqlx::query("DELETE FROM sessions WHERE id = ?")
        .bind(&id.0)
        .execute(&mut *tx)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;

    tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Fetch one session by id (read-only).
pub async fn get(pool: &SqlitePool, id: &SessionId) -> Result<Option<Session>> {
    let row = sqlx::query(
        "SELECT id, workarea_id, chat_id, agent_kind, agent_version, model, mode,
                host_pid, host_socket, pty_cookie, external_session_id,
                permission_mode, bypass_destructive_guard,
                started_at, ended_at, last_heartbeat, status, last_acked_seq
         FROM sessions WHERE id = ?",
    )
    .bind(&id.0)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(row_to_session))
}

/// List `sessions.id` values for a workarea whose `ended_at` is NULL
/// (i.e. potentially live sessions). Task 31's archive cascade uses this
/// to decide which sessions to ask the Agent Supervisor to stop.
///
/// Reads the read-only pool — callers do not need to hold the writer
/// guard for this lookup.
pub async fn list_live_ids_by_workarea(
    pool: &SqlitePool,
    workarea_id: &WorkareaId,
) -> Result<Vec<SessionId>> {
    let rows = sqlx::query("SELECT id FROM sessions WHERE workarea_id = ? AND ended_at IS NULL")
        .bind(&workarea_id.0)
        .fetch_all(pool)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows
        .into_iter()
        .map(|r| SessionId(r.get::<String, _>("id")))
        .collect())
}

/// List sessions attached to a workarea (read-only), newest first.
pub async fn list_by_workarea(pool: &SqlitePool, workarea_id: &WorkareaId) -> Result<Vec<Session>> {
    let rows = sqlx::query(
        "SELECT id, workarea_id, chat_id, agent_kind, agent_version, model, mode,
                host_pid, host_socket, pty_cookie, external_session_id,
                permission_mode, bypass_destructive_guard,
                started_at, ended_at, last_heartbeat, status, last_acked_seq
         FROM sessions WHERE workarea_id = ? ORDER BY started_at DESC",
    )
    .bind(&workarea_id.0)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_session).collect())
}

fn row_to_session(row: sqlx::sqlite::SqliteRow) -> Session {
    Session {
        id: SessionId(row.get::<String, _>("id")),
        workarea_id: WorkareaId(row.get::<String, _>("workarea_id")),
        chat_id: row.get::<String, _>("chat_id"),
        agent_kind: row.get::<String, _>("agent_kind"),
        agent_version: row.get::<Option<String>, _>("agent_version"),
        model: row.get::<Option<String>, _>("model"),
        mode: row.get::<Option<String>, _>("mode"),
        host_pid: row.get::<Option<i64>, _>("host_pid"),
        host_socket: row.get::<Option<String>, _>("host_socket"),
        pty_cookie: row.get::<Option<Vec<u8>>, _>("pty_cookie"),
        external_session_id: row.get::<Option<String>, _>("external_session_id"),
        permission_mode: row.get::<String, _>("permission_mode"),
        bypass_destructive_guard: row.get::<i64, _>("bypass_destructive_guard") != 0,
        started_at: row.get::<i64, _>("started_at"),
        ended_at: row.get::<Option<i64>, _>("ended_at"),
        last_heartbeat: row.get::<Option<i64>, _>("last_heartbeat"),
        status: row.get::<String, _>("status"),
        last_acked_seq: row.get::<i64, _>("last_acked_seq"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Persistence, PersistenceConfig};

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

    /// Seed the full workspace → workarea → session chain plus
    /// every dependent that participates in the cascade (chat, chat_message,
    /// checkpoint, tool_approval, schedule + schedule_run), delete the
    /// session, and assert the cascade behaviour:
    ///   * session / chat / chat_message / checkpoint / tool_approval gone;
    ///   * schedule_run survives with `session_id` NULLed (history kept).
    #[tokio::test]
    async fn delete_removes_session_and_dependents() {
        let (_dir, persist) = fresh_db().await;
        let mut w = persist.writer().await;

        // ----- repository / workspace / workarea --------------------------
        sqlx::query(
            "INSERT INTO repositories \
             (id, name, url, local_path, clone_strategy, default_branch) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind("repo-1")
        .bind("r")
        .bind("git@example.com:r.git")
        .bind("/tmp/repo-1")
        .bind("full")
        .bind("main")
        .execute(&mut *w)
        .await
        .expect("repository");
        sqlx::query(
            "INSERT INTO workspaces (id, name, slug, created_at) \
             VALUES (?, ?, ?, ?)",
        )
        .bind("ws-1")
        .bind("w")
        .bind("w")
        .bind(0_i64)
        .execute(&mut *w)
        .await
        .expect("workspace");
        sqlx::query(
            "INSERT INTO workareas \
             (id, workspace_id, composer_name, branch_name, worktree_root, status, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("wa-1")
        .bind("ws-1")
        .bind("bach")
        .bind("b")
        .bind("/tmp/wa-1")
        .bind("created")
        .bind(0_i64)
        .execute(&mut *w)
        .await
        .expect("workarea");

        // ----- session (+ its session-kind chat) --------------------------
        // `chats.session_id` is NOT NULL only when kind='session', and the
        // sessions.chat_id FK needs a chat to exist first. Seed a maestro
        // chat (session_id NULL) to satisfy sessions.chat_id, insert the
        // session, then a real session-kind chat pointing back at it.
        sqlx::query("INSERT INTO chats (id, session_id, kind, created_at) VALUES (?, NULL, ?, ?)")
            .bind("chat-bootstrap")
            .bind("maestro")
            .bind(0_i64)
            .execute(&mut *w)
            .await
            .expect("bootstrap chat");

        insert(
            &mut w,
            NewSession {
                id: SessionId("sess-1".into()),
                workarea_id: WorkareaId("wa-1".into()),
                chat_id: "chat-bootstrap".into(),
                agent_kind: "claude".into(),
                agent_version: None,
                model: None,
                mode: None,
                host_pid: None,
                host_socket: None,
                pty_cookie: None,
                external_session_id: None,
                permission_mode: "normal".into(),
                bypass_destructive_guard: false,
                started_at: 0,
                status: "running".into(),
                last_acked_seq: 0,
            },
        )
        .await
        .expect("insert session");

        sqlx::query("INSERT INTO chats (id, session_id, kind, created_at) VALUES (?, ?, ?, ?)")
            .bind("chat-sess")
            .bind("sess-1")
            .bind("session")
            .bind(0_i64)
            .execute(&mut *w)
            .await
            .expect("session chat");

        // ----- chat_message + checkpoint (RESTRICT FK on chat_message) ----
        sqlx::query(
            "INSERT INTO chat_messages (id, chat_id, role, content_json, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind("msg-1")
        .bind("chat-sess")
        .bind("user")
        .bind("{}")
        .bind(0_i64)
        .execute(&mut *w)
        .await
        .expect("chat message");
        sqlx::query(
            "INSERT INTO checkpoints \
             (id, workarea_id, repository_id, chat_message_id, git_ref, created_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind("ck-1")
        .bind("wa-1")
        .bind("repo-1")
        .bind("msg-1")
        .bind("refs/x")
        .bind(0_i64)
        .execute(&mut *w)
        .await
        .expect("checkpoint");

        // ----- tool_approval (auto-cascade) -------------------------------
        sqlx::query(
            "INSERT INTO tool_approvals \
             (id, session_id, tool_name, payload_json, requested_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind("ta-1")
        .bind("sess-1")
        .bind("Bash")
        .bind("{}")
        .bind(0_i64)
        .execute(&mut *w)
        .await
        .expect("tool approval");

        // ----- schedule + schedule_run (RESTRICT, must be NULLed) ---------
        sqlx::query(
            "INSERT INTO schedules \
             (id, workarea_id, kind, interval_seconds, expires_at, prompt, created_at) \
             VALUES (?, ?, 'loop', ?, ?, ?, ?)",
        )
        .bind("sch-1")
        .bind("wa-1")
        .bind(60_i64)
        .bind(9_999_999_999_999_i64)
        .bind("do the thing")
        .bind(0_i64)
        .execute(&mut *w)
        .await
        .expect("schedule");
        sqlx::query(
            "INSERT INTO schedule_runs (id, schedule_id, session_id, started_at) \
             VALUES (?, ?, ?, ?)",
        )
        .bind("run-1")
        .bind("sch-1")
        .bind("sess-1")
        .bind(0_i64)
        .execute(&mut *w)
        .await
        .expect("schedule run");

        // ----- act --------------------------------------------------------
        delete(&mut w, &SessionId("sess-1".into()))
            .await
            .expect("delete session");

        // ----- assert: session gone everywhere ----------------------------
        let n_sess: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE id = ?")
            .bind("sess-1")
            .fetch_one(&mut *w)
            .await
            .expect("count sessions");
        assert_eq!(n_sess, 0, "session row must be deleted");

        // chats + chat_messages cascaded (the session-kind chat + its message)
        let n_chat: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chats WHERE id = ?")
            .bind("chat-sess")
            .fetch_one(&mut *w)
            .await
            .expect("count chats");
        assert_eq!(n_chat, 0, "session chat must cascade-delete");
        let n_msg: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_messages WHERE id = ?")
            .bind("msg-1")
            .fetch_one(&mut *w)
            .await
            .expect("count chat_messages");
        assert_eq!(n_msg, 0, "chat_message must cascade-delete");

        // checkpoint explicitly deleted (would otherwise block the cascade)
        let n_ck: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM checkpoints WHERE id = ?")
            .bind("ck-1")
            .fetch_one(&mut *w)
            .await
            .expect("count checkpoints");
        assert_eq!(n_ck, 0, "checkpoint must be deleted");

        // tool_approval cascaded
        let n_ta: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tool_approvals WHERE id = ?")
            .bind("ta-1")
            .fetch_one(&mut *w)
            .await
            .expect("count tool_approvals");
        assert_eq!(n_ta, 0, "tool_approval must cascade-delete");

        // schedule_run SURVIVES with session_id NULLed
        let n_run: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM schedule_runs WHERE id = ?")
            .bind("run-1")
            .fetch_one(&mut *w)
            .await
            .expect("count schedule_runs");
        assert_eq!(n_run, 1, "schedule_run must be preserved (history)");
        let run_session: Option<String> =
            sqlx::query_scalar("SELECT session_id FROM schedule_runs WHERE id = ?")
                .bind("run-1")
                .fetch_one(&mut *w)
                .await
                .expect("schedule_run session_id");
        assert_eq!(run_session, None, "schedule_run.session_id must be NULLed");

        // get() / list_by_workarea reflect the deletion (need the pool, not
        // the writer guard — drop the guard first to avoid deadlock).
        drop(w);
        let got = get(persist.readers(), &SessionId("sess-1".into()))
            .await
            .expect("get");
        assert!(got.is_none(), "get() must return None after delete");
        let listed = list_by_workarea(persist.readers(), &WorkareaId("wa-1".into()))
            .await
            .expect("list");
        assert!(
            listed.iter().all(|s| s.id.0 != "sess-1"),
            "deleted session must be absent from list_by_workarea"
        );
    }
}
