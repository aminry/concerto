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

use concerto_error::{Error, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};

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
            started_at, status
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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

/// Fetch one session by id (read-only).
pub async fn get(pool: &SqlitePool, id: &SessionId) -> Result<Option<Session>> {
    let row = sqlx::query(
        "SELECT id, workarea_id, chat_id, agent_kind, agent_version, model, mode,
                host_pid, host_socket, pty_cookie, external_session_id,
                permission_mode, bypass_destructive_guard,
                started_at, ended_at, last_heartbeat, status
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
                started_at, ended_at, last_heartbeat, status
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
    }
}
