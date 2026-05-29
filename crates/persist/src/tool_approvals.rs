//! `tool_approvals` table CRUD (Task 33).
//!
//! Schema is locked by migration 0001 (Task 09):
//!
//! ```sql
//! CREATE TABLE tool_approvals (
//!     id                      TEXT PRIMARY KEY,
//!     session_id              TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
//!     tool_name               TEXT NOT NULL,
//!     payload_json            TEXT NOT NULL,
//!     requested_at            INTEGER NOT NULL,
//!     decided_at              INTEGER,
//!     decided_by_device_id    TEXT REFERENCES devices(id),
//!     decision                TEXT CHECK (decision IS NULL OR decision IN (
//!         'approve','approve_once','deny','auto_strict','auto_normal','auto_auto','auto_yolo'
//!     ))
//! );
//! ```
//!
//! Surfaces a small CRUD: [`insert`] writes a pending row (or an
//! already-decided auto row), [`update_decision`] applies the
//! first-write-wins decision flip, [`get`] / [`list_by_session`] are the
//! readers.

use concerto_error::{Error, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::api::SessionId;

/// Insert-time shape for a `tool_approvals` row. The PK is allocated by
/// the caller (UUIDv7 string). When the supervisor's
/// [`PermissionResolver`] auto-decides the call (auto / normal-safe /
/// yolo-* paths), it passes the matching `decision` + `decided_at` in
/// the same insert; manual approvals leave both `None`.
#[derive(Debug, Clone)]
pub struct NewToolApproval {
    pub id: String,
    pub session_id: SessionId,
    pub tool_name: String,
    pub payload_json: String,
    pub requested_at: i64,
    pub decision: Option<String>,
    pub decided_at: Option<i64>,
    pub decided_by_device_id: Option<String>,
    /// Task 43: true iff the destructive-command intercept fired. Persisted as
    /// the `tool_approvals.urgent` integer column (migration 0007) and
    /// surfaced on the `AwaitingApproval` event so clients render the
    /// red-urgent prompt styling.
    pub urgent: bool,
}

/// Row-shaped projection of a `tool_approvals` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolApproval {
    pub id: String,
    pub session_id: SessionId,
    pub tool_name: String,
    pub payload_json: String,
    pub requested_at: i64,
    pub decided_at: Option<i64>,
    pub decided_by_device_id: Option<String>,
    pub decision: Option<String>,
    /// Task 43: destructive-command intercept fired for this row.
    pub urgent: bool,
}

/// Insert a new `tool_approvals` row.
///
/// Returns the row id on success. Caller-supplied UUIDv7 keeps the
/// chronological ordering — the supervisor's auto path passes the same
/// id it uses for the in-memory `pending_approvals` map key when it
/// branches on `MustAsk`, so a later `Sessions.ResolveApproval` lookup
/// is O(1).
pub async fn insert(conn: &mut SqliteConnection, row: NewToolApproval) -> Result<String> {
    let id = row.id.clone();
    sqlx::query(
        "INSERT INTO tool_approvals (
            id, session_id, tool_name, payload_json, requested_at,
            decided_at, decided_by_device_id, decision, urgent
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&row.id)
    .bind(&row.session_id.0)
    .bind(&row.tool_name)
    .bind(&row.payload_json)
    .bind(row.requested_at)
    .bind(row.decided_at)
    .bind(&row.decided_by_device_id)
    .bind(&row.decision)
    .bind(i64::from(row.urgent))
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(id)
}

/// Patch `decision` + `decided_at` + `decided_by_device_id` on an
/// existing `tool_approvals` row.
///
/// First-write-wins: the `UPDATE … WHERE id = ? AND decision IS NULL`
/// guard means a second call against an already-decided row is a no-op.
/// Callers MUST check the returned row count and surface
/// `FAILED_PRECONDITION` when zero rows changed.
pub async fn update_decision(
    conn: &mut SqliteConnection,
    id: &str,
    decision: &str,
    decided_at: i64,
    decided_by_device_id: Option<&str>,
) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE tool_approvals
           SET decision = ?, decided_at = ?, decided_by_device_id = ?
         WHERE id = ? AND decision IS NULL",
    )
    .bind(decision)
    .bind(decided_at)
    .bind(decided_by_device_id)
    .bind(id)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(res.rows_affected())
}

/// Fetch one row by id.
pub async fn get(pool: &SqlitePool, id: &str) -> Result<Option<ToolApproval>> {
    let row = sqlx::query(
        "SELECT id, session_id, tool_name, payload_json, requested_at,
                decided_at, decided_by_device_id, decision, urgent
           FROM tool_approvals WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(row_to_approval))
}

/// List all approvals for a session, oldest first (matching
/// `requested_at` order). Used by the desktop UI to render the pending
/// + historical approval log.
pub async fn list_by_session(
    pool: &SqlitePool,
    session_id: &SessionId,
) -> Result<Vec<ToolApproval>> {
    let rows = sqlx::query(
        "SELECT id, session_id, tool_name, payload_json, requested_at,
                decided_at, decided_by_device_id, decision, urgent
           FROM tool_approvals WHERE session_id = ?
          ORDER BY requested_at ASC",
    )
    .bind(&session_id.0)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_approval).collect())
}

fn row_to_approval(row: sqlx::sqlite::SqliteRow) -> ToolApproval {
    ToolApproval {
        id: row.get::<String, _>("id"),
        session_id: SessionId(row.get::<String, _>("session_id")),
        tool_name: row.get::<String, _>("tool_name"),
        payload_json: row.get::<String, _>("payload_json"),
        requested_at: row.get::<i64, _>("requested_at"),
        decided_at: row.get::<Option<i64>, _>("decided_at"),
        decided_by_device_id: row.get::<Option<String>, _>("decided_by_device_id"),
        decision: row.get::<Option<String>, _>("decision"),
        urgent: row.get::<i64, _>("urgent") != 0,
    }
}
