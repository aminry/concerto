//! `schedule_runs` table CRUD (Task 38).
//!
//! Schema is locked by migration 0004:
//!
//! ```sql
//! CREATE TABLE schedule_runs (
//!     id              TEXT PRIMARY KEY,
//!     schedule_id     TEXT NOT NULL REFERENCES schedules(id) ON DELETE CASCADE,
//!     session_id      TEXT REFERENCES sessions(id),
//!     started_at      INTEGER NOT NULL,
//!     ended_at        INTEGER,
//!     terminal_state  TEXT  -- NULL | completed | failed | crashed
//! );
//! ```
//!
//! V0.1 does not track `tokens_in`/`tokens_out` from `design/09 §4.3` —
//! token bookkeeping is V1.0 (budget guardrails).

use concerto_error::{Error, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::api::{NewScheduleRun, ScheduleId, ScheduleRun, ScheduleRunId, SessionId};

/// Insert a new `schedule_runs` row. The row is inserted with
/// `ended_at = NULL` and `terminal_state = NULL`; the lifecycle update
/// goes through [`update_terminal`].
pub async fn insert(conn: &mut SqliteConnection, r: NewScheduleRun) -> Result<ScheduleRunId> {
    let id = r.id.clone();
    sqlx::query(
        "INSERT INTO schedule_runs (
            id, schedule_id, session_id, started_at, ended_at, terminal_state
         ) VALUES (?, ?, ?, ?, NULL, NULL)",
    )
    .bind(&id.0)
    .bind(&r.schedule_id.0)
    .bind(r.session_id.as_ref().map(|s| s.0.clone()))
    .bind(r.started_at)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(id)
}

/// Patch the `session_id` for a run after the supervisor returns the
/// freshly-allocated id. Splitting `insert` + `update_session` keeps the
/// run row in scope during the suppression check before the session
/// exists.
pub async fn update_session(
    conn: &mut SqliteConnection,
    id: &ScheduleRunId,
    session_id: &SessionId,
) -> Result<()> {
    sqlx::query("UPDATE schedule_runs SET session_id = ? WHERE id = ?")
        .bind(&session_id.0)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Mark the run terminal: set `ended_at` and one of
/// `'completed' | 'failed' | 'crashed'` on `terminal_state`. Called
/// when the session emits `TurnComplete` (completed), `Exited` with
/// nonzero exit/signal (crashed), or when a pre-handshake spawn error
/// trips the watch task (failed).
pub async fn update_terminal(
    conn: &mut SqliteConnection,
    id: &ScheduleRunId,
    ended_at: i64,
    terminal_state: &str,
) -> Result<()> {
    sqlx::query("UPDATE schedule_runs SET ended_at = ?, terminal_state = ? WHERE id = ?")
        .bind(ended_at)
        .bind(terminal_state)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Return the inflight run id for a schedule (rows where
/// `ended_at IS NULL`), if any. Used by the fire loop's inflight
/// suppression check.
pub async fn current_inflight(
    pool: &SqlitePool,
    schedule_id: &ScheduleId,
) -> Result<Option<ScheduleRunId>> {
    let row = sqlx::query(
        "SELECT id FROM schedule_runs
         WHERE schedule_id = ? AND ended_at IS NULL
         ORDER BY started_at DESC LIMIT 1",
    )
    .bind(&schedule_id.0)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(|r| ScheduleRunId(r.get::<String, _>("id"))))
}

/// List the run history for a schedule, newest first. Used by
/// `Schedules.GetScheduleHistory`.
pub async fn list_by_schedule(
    pool: &SqlitePool,
    schedule_id: &ScheduleId,
) -> Result<Vec<ScheduleRun>> {
    let rows = sqlx::query(
        "SELECT id, schedule_id, session_id, started_at, ended_at, terminal_state
         FROM schedule_runs WHERE schedule_id = ?
         ORDER BY started_at DESC",
    )
    .bind(&schedule_id.0)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_schedule_run).collect())
}

fn row_to_schedule_run(row: sqlx::sqlite::SqliteRow) -> ScheduleRun {
    ScheduleRun {
        id: ScheduleRunId(row.get::<String, _>("id")),
        schedule_id: ScheduleId(row.get::<String, _>("schedule_id")),
        session_id: row.get::<Option<String>, _>("session_id").map(SessionId),
        started_at: row.get::<i64, _>("started_at"),
        ended_at: row.get::<Option<i64>, _>("ended_at"),
        terminal_state: row.get::<Option<String>, _>("terminal_state"),
    }
}
