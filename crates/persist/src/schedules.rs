//! `schedules` table CRUD (Task 38).
//!
//! Schema is locked by migration 0004 (`tasks/38-scheduler-loop.md`):
//!
//! ```sql
//! CREATE TABLE schedules (
//!     id                  TEXT PRIMARY KEY,
//!     workarea_id         TEXT NOT NULL REFERENCES workareas(id) ON DELETE CASCADE,
//!     kind                TEXT NOT NULL CHECK (kind = 'loop'),
//!     interval_seconds    INTEGER NOT NULL,
//!     expires_at          INTEGER NOT NULL,
//!     last_run_at         INTEGER,
//!     paused              INTEGER NOT NULL DEFAULT 0,
//!     prompt              TEXT NOT NULL,
//!     agent_kind          TEXT NOT NULL DEFAULT 'claude',
//!     created_at          INTEGER NOT NULL
//! );
//! ```
//!
//! V0.1 only ships `kind = 'loop'`. The persistent scheduled-task
//! columns (`cron_expr`, `model`, `permission_mode`,
//! `bypass_destructive_guard`, `worktree_mode`, `failure_policy_json`,
//! `daily_budget_tokens`) from `design/09 §4.3` arrive with the V1.0
//! cron surface in a later migration.

use concerto_error::{Error, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::api::{NewSchedule, Schedule, ScheduleId, WorkareaId};

/// Insert a new `schedules` row.
pub async fn insert(conn: &mut SqliteConnection, s: NewSchedule) -> Result<ScheduleId> {
    let id = s.id.clone();
    sqlx::query(
        "INSERT INTO schedules (
            id, workarea_id, kind, interval_seconds, expires_at,
            last_run_at, paused, prompt, agent_kind, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id.0)
    .bind(&s.workarea_id.0)
    .bind(&s.kind)
    .bind(s.interval_seconds)
    .bind(s.expires_at)
    .bind(s.last_run_at)
    .bind(s.paused as i64)
    .bind(&s.prompt)
    .bind(&s.agent_kind)
    .bind(s.created_at)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(id)
}

/// Fetch one schedule by id (read-only).
pub async fn get(pool: &SqlitePool, id: &ScheduleId) -> Result<Option<Schedule>> {
    let row = sqlx::query(
        "SELECT id, workarea_id, kind, interval_seconds, expires_at,
                last_run_at, paused, prompt, agent_kind, created_at
         FROM schedules WHERE id = ?",
    )
    .bind(&id.0)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(row_to_schedule))
}

/// List every schedule for a workarea (read-only), oldest first.
///
/// Used by `Schedules.ListSchedules` so the UI can render the workarea's
/// active + paused + expired loops together. Pause / expiry filtering
/// happens in the caller because the UI wants to differentiate them.
pub async fn list_by_workarea(
    pool: &SqlitePool,
    workarea_id: &WorkareaId,
) -> Result<Vec<Schedule>> {
    let rows = sqlx::query(
        "SELECT id, workarea_id, kind, interval_seconds, expires_at,
                last_run_at, paused, prompt, agent_kind, created_at
         FROM schedules WHERE workarea_id = ? ORDER BY created_at ASC",
    )
    .bind(&workarea_id.0)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_schedule).collect())
}

/// List every schedule that the scheduler's fire loop should hold in
/// memory: `paused = 0 AND expires_at > now`. Called on Core boot to
/// rebuild the BTreeMap from disk and on `expiration_sweep` to figure
/// out which schedules to evict.
pub async fn list_active(pool: &SqlitePool, now_ms: i64) -> Result<Vec<Schedule>> {
    let rows = sqlx::query(
        "SELECT id, workarea_id, kind, interval_seconds, expires_at,
                last_run_at, paused, prompt, agent_kind, created_at
         FROM schedules WHERE paused = 0 AND expires_at > ?
         ORDER BY created_at ASC",
    )
    .bind(now_ms)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_schedule).collect())
}

/// Set `paused = 1` on the row. Idempotent — pausing an already-paused
/// row is a no-op at the SQL level.
pub async fn pause(conn: &mut SqliteConnection, id: &ScheduleId) -> Result<()> {
    sqlx::query("UPDATE schedules SET paused = 1 WHERE id = ?")
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Delete the row. `schedule_runs` rows cascade per the FK.
pub async fn delete(conn: &mut SqliteConnection, id: &ScheduleId) -> Result<()> {
    sqlx::query("DELETE FROM schedules WHERE id = ?")
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Patch `last_run_at` for a schedule. Called every time the fire loop
/// successfully kicks off a session for the schedule (suppressed fires
/// do not update the column — they are skipped on the way to the
/// supervisor).
pub async fn update_last_run(
    conn: &mut SqliteConnection,
    id: &ScheduleId,
    last_run_at: i64,
) -> Result<()> {
    sqlx::query("UPDATE schedules SET last_run_at = ? WHERE id = ?")
        .bind(last_run_at)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// `expiration_sweep` UPDATE — pauses every row whose `expires_at` is
/// in the past. Returns the count of rows modified so the caller can
/// log the sweep result.
pub async fn pause_expired(conn: &mut SqliteConnection, now_ms: i64) -> Result<u64> {
    let result =
        sqlx::query("UPDATE schedules SET paused = 1 WHERE paused = 0 AND expires_at <= ?")
            .bind(now_ms)
            .execute(conn)
            .await
            .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(result.rows_affected())
}

fn row_to_schedule(row: sqlx::sqlite::SqliteRow) -> Schedule {
    Schedule {
        id: ScheduleId(row.get::<String, _>("id")),
        workarea_id: WorkareaId(row.get::<String, _>("workarea_id")),
        kind: row.get::<String, _>("kind"),
        interval_seconds: row.get::<i64, _>("interval_seconds"),
        expires_at: row.get::<i64, _>("expires_at"),
        last_run_at: row.get::<Option<i64>, _>("last_run_at"),
        paused: row.get::<i64, _>("paused") != 0,
        prompt: row.get::<String, _>("prompt"),
        agent_kind: row.get::<String, _>("agent_kind"),
        created_at: row.get::<i64, _>("created_at"),
    }
}
