//! `notifications` + `notification_deliveries` table CRUD (Task 501).
//!
//! Schema is locked by migration 0017 (`design/14 §4`, PHASE5_PLANNING §4.1).
//! This module is the persistence root of sub-system 14: Task 502 adds the
//! de-dup query + retention/archive helpers, 504 the delivery fan-out reads,
//! and 507 the gRPC-facing reads. The `kind`/`subject_kind`/`severity` columns
//! store the snake_case string forms; `crates/core/src/notifications/model.rs`
//! maps them to/from the proto enums.
//!
//! Writes take `&mut SqliteConnection` (run under the persistence writer mutex);
//! reads take `&SqlitePool` (the query-only reader pool) — the convention every
//! other table module follows.

use concerto_error::{Error, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};

/// Insert-time shape for a `notifications` row. The PK is a caller-allocated
/// ULID (chronological-sortable). `kind`/`subject_kind`/`severity` are the
/// snake_case strings the CHECKs accept. The denormalized `action_*` columns
/// are always `None` at insert (they are set later via [`set_action_taken`]
/// after the underlying approval resolves).
#[derive(Debug, Clone)]
pub struct NewNotification {
    pub id: String,
    pub kind: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub workspace_id: Option<String>,
    pub workarea_id: Option<String>,
    pub session_id: Option<String>,
    pub title: String,
    pub body: String,
    /// Persisted top-3 `Chip` slate as JSON (suggestions.proto shape), or `None`.
    pub chips_json: Option<String>,
    /// `ToolApprovalContext` JSON for `tool_approval_needed` rows, or `None`.
    pub approval_json: Option<String>,
    pub severity: String,
    pub created_at: i64,
}

/// Row-shaped projection of a `notifications` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationRow {
    pub id: String,
    pub kind: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub workspace_id: Option<String>,
    pub workarea_id: Option<String>,
    pub session_id: Option<String>,
    pub title: String,
    pub body: String,
    pub chips_json: Option<String>,
    pub approval_json: Option<String>,
    pub severity: String,
    pub created_at: i64,
    pub read_at: Option<i64>,
    pub superseded_by: Option<String>,
    pub action_taken: Option<String>,
    pub action_taken_at: Option<i64>,
    pub action_taken_by_device_id: Option<String>,
}

/// Insert-time shape for a `notification_deliveries` row.
#[derive(Debug, Clone)]
pub struct NewDelivery {
    pub notification_id: String,
    pub device_id: String,
    pub delivered_at: Option<i64>,
    pub fetched_at: Option<i64>,
}

/// Row-shaped projection of a `notification_deliveries` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryRow {
    pub notification_id: String,
    pub device_id: String,
    pub delivered_at: Option<i64>,
    pub fetched_at: Option<i64>,
}

const COLS: &str = "id, kind, subject_kind, subject_id, workspace_id, workarea_id, session_id, \
     title, body, chips_json, approval_json, severity, created_at, read_at, superseded_by, \
     action_taken, action_taken_at, action_taken_by_device_id";

/// Insert a new `notifications` row. Returns the row id.
pub async fn insert(conn: &mut SqliteConnection, row: NewNotification) -> Result<String> {
    let id = row.id.clone();
    sqlx::query(
        "INSERT INTO notifications (
            id, kind, subject_kind, subject_id, workspace_id, workarea_id, session_id,
            title, body, chips_json, approval_json, severity, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&row.id)
    .bind(&row.kind)
    .bind(&row.subject_kind)
    .bind(&row.subject_id)
    .bind(&row.workspace_id)
    .bind(&row.workarea_id)
    .bind(&row.session_id)
    .bind(&row.title)
    .bind(&row.body)
    .bind(&row.chips_json)
    .bind(&row.approval_json)
    .bind(&row.severity)
    .bind(row.created_at)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(id)
}

/// Fetch one notification by id.
pub async fn get(pool: &SqlitePool, id: &str) -> Result<Option<NotificationRow>> {
    let row = sqlx::query(&format!("SELECT {COLS} FROM notifications WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(row_to_notification))
}

/// Inbox feed: newest-first, optionally scoped to a workspace/workarea and/or
/// restricted to unread, capped at `limit` (0 ⇒ `DEFAULT_LIMIT`). Task 502
/// layers de-dup + retention on top; this is the base reader.
pub async fn list_inbox(
    pool: &SqlitePool,
    workspace_id: Option<&str>,
    workarea_id: Option<&str>,
    unread_only: bool,
    limit: u32,
) -> Result<Vec<NotificationRow>> {
    const DEFAULT_LIMIT: u32 = 100;
    let limit = if limit == 0 { DEFAULT_LIMIT } else { limit };
    let rows = sqlx::query(&format!(
        "SELECT {COLS} FROM notifications
          WHERE (?1 IS NULL OR workspace_id = ?1)
            AND (?2 IS NULL OR workarea_id = ?2)
            AND (?3 = 0 OR read_at IS NULL)
            AND superseded_by IS NULL
          ORDER BY created_at DESC
          LIMIT ?4"
    ))
    .bind(workspace_id)
    .bind(workarea_id)
    .bind(i64::from(unread_only))
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_notification).collect())
}

/// Mark a notification read (idempotent: only flips an unread row). Returns the
/// rows-affected count so callers can detect already-read.
pub async fn mark_read(conn: &mut SqliteConnection, id: &str, read_at: i64) -> Result<u64> {
    let res = sqlx::query("UPDATE notifications SET read_at = ? WHERE id = ? AND read_at IS NULL")
        .bind(read_at)
        .bind(id)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(res.rows_affected())
}

/// Record the denormalized first-wins UI marker AFTER the underlying action
/// resolved (PHASE5_PLANNING D5). First-write-wins via the `action_taken IS
/// NULL` guard; returns rows-affected (0 ⇒ already acted).
pub async fn set_action_taken(
    conn: &mut SqliteConnection,
    id: &str,
    action_taken: &str,
    action_taken_at: i64,
    action_taken_by_device_id: Option<&str>,
) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE notifications
            SET action_taken = ?, action_taken_at = ?, action_taken_by_device_id = ?
          WHERE id = ? AND action_taken IS NULL",
    )
    .bind(action_taken)
    .bind(action_taken_at)
    .bind(action_taken_by_device_id)
    .bind(id)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(res.rows_affected())
}

/// Point a superseded notification at its replacement (de-dup; Task 502).
pub async fn set_superseded(
    conn: &mut SqliteConnection,
    id: &str,
    superseded_by: &str,
) -> Result<u64> {
    let res = sqlx::query("UPDATE notifications SET superseded_by = ? WHERE id = ?")
        .bind(superseded_by)
        .bind(id)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(res.rows_affected())
}

/// Insert (or replace) a per-device delivery record.
pub async fn upsert_delivery(conn: &mut SqliteConnection, row: NewDelivery) -> Result<()> {
    sqlx::query(
        "INSERT INTO notification_deliveries (notification_id, device_id, delivered_at, fetched_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (notification_id, device_id) DO UPDATE SET
            delivered_at = COALESCE(excluded.delivered_at, delivered_at),
            fetched_at   = COALESCE(excluded.fetched_at, fetched_at)",
    )
    .bind(&row.notification_id)
    .bind(&row.device_id)
    .bind(row.delivered_at)
    .bind(row.fetched_at)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// List the delivery records for a notification.
pub async fn list_deliveries(pool: &SqlitePool, notification_id: &str) -> Result<Vec<DeliveryRow>> {
    let rows = sqlx::query(
        "SELECT notification_id, device_id, delivered_at, fetched_at
           FROM notification_deliveries WHERE notification_id = ?",
    )
    .bind(notification_id)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows
        .into_iter()
        .map(|r| DeliveryRow {
            notification_id: r.get::<String, _>("notification_id"),
            device_id: r.get::<String, _>("device_id"),
            delivered_at: r.get::<Option<i64>, _>("delivered_at"),
            fetched_at: r.get::<Option<i64>, _>("fetched_at"),
        })
        .collect())
}

/// Find the most-recent UNREAD, non-superseded notification matching the de-dup
/// key created at or after `since` (the de-dup window floor). The key is
/// `(workarea_id, kind, subject_id)` when a workarea is set, else
/// `(workspace_id, kind, subject_id)` with `workarea_id IS NULL`
/// (`design/14 §3.7`). Task 502.
pub async fn find_unread_for_dedup_key(
    pool: &SqlitePool,
    workspace_id: Option<&str>,
    workarea_id: Option<&str>,
    kind: &str,
    subject_id: &str,
    since: i64,
) -> Result<Option<NotificationRow>> {
    // Two scopings: workarea-keyed vs workspace-keyed (workarea NULL).
    let sql = if workarea_id.is_some() {
        format!(
            "SELECT {COLS} FROM notifications
              WHERE workarea_id = ?1 AND kind = ?2 AND subject_id = ?3
                AND created_at >= ?4 AND read_at IS NULL AND superseded_by IS NULL
              ORDER BY created_at DESC LIMIT 1"
        )
    } else {
        format!(
            "SELECT {COLS} FROM notifications
              WHERE workspace_id = ?1 AND workarea_id IS NULL AND kind = ?2 AND subject_id = ?3
                AND created_at >= ?4 AND read_at IS NULL AND superseded_by IS NULL
              ORDER BY created_at DESC LIMIT 1"
        )
    };
    let key1 = if workarea_id.is_some() {
        workarea_id
    } else {
        workspace_id
    };
    let row = sqlx::query(&sql)
        .bind(key1)
        .bind(kind)
        .bind(subject_id)
        .bind(since)
        .fetch_optional(pool)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(row_to_notification))
}

/// Refresh a de-dup-hit notification's `body` + `created_at` in place instead of
/// inserting a new row (`design/14 §3.7`: update, do not re-wakeup). Returns
/// rows-affected.
pub async fn update_body_and_at(
    conn: &mut SqliteConnection,
    id: &str,
    body: &str,
    at: i64,
) -> Result<u64> {
    let res = sqlx::query("UPDATE notifications SET body = ?, created_at = ? WHERE id = ?")
        .bind(body)
        .bind(at)
        .bind(id)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(res.rows_affected())
}

/// The eligible push targets (Task 504, design/14 §3.4): active devices
/// (`revoked_at IS NULL`) that have a push token + platform and are not currently
/// in Do-Not-Disturb (`now < dnd_until`). Returns `(device_id, push_token,
/// push_platform)`; the core fan-out maps these to `PushTarget`, dropping any row
/// whose stored platform is not a known variant.
pub async fn list_pushable_devices(
    pool: &SqlitePool,
    now: i64,
) -> Result<Vec<(String, String, String)>> {
    let rows = sqlx::query(
        "SELECT id, push_token, push_platform FROM devices
          WHERE revoked_at IS NULL
            AND push_token IS NOT NULL
            AND push_platform IS NOT NULL
            AND (dnd_until IS NULL OR dnd_until <= ?)
          ORDER BY id ASC",
    )
    .bind(now)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.get::<String, _>("id"),
                r.get::<String, _>("push_token"),
                r.get::<String, _>("push_platform"),
            )
        })
        .collect())
}

/// Count notifications older than `before` (retention/archival reporting,
/// `design/14 §3.9 R-9`: 90-day default; kept, not deleted in V1.0).
pub async fn count_older_than(pool: &SqlitePool, before: i64) -> Result<i64> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notifications WHERE created_at < ?")
        .bind(before)
        .fetch_one(pool)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(n)
}

fn row_to_notification(row: sqlx::sqlite::SqliteRow) -> NotificationRow {
    NotificationRow {
        id: row.get::<String, _>("id"),
        kind: row.get::<String, _>("kind"),
        subject_kind: row.get::<String, _>("subject_kind"),
        subject_id: row.get::<String, _>("subject_id"),
        workspace_id: row.get::<Option<String>, _>("workspace_id"),
        workarea_id: row.get::<Option<String>, _>("workarea_id"),
        session_id: row.get::<Option<String>, _>("session_id"),
        title: row.get::<String, _>("title"),
        body: row.get::<String, _>("body"),
        chips_json: row.get::<Option<String>, _>("chips_json"),
        approval_json: row.get::<Option<String>, _>("approval_json"),
        severity: row.get::<String, _>("severity"),
        created_at: row.get::<i64, _>("created_at"),
        read_at: row.get::<Option<i64>, _>("read_at"),
        superseded_by: row.get::<Option<String>, _>("superseded_by"),
        action_taken: row.get::<Option<String>, _>("action_taken"),
        action_taken_at: row.get::<Option<i64>, _>("action_taken_at"),
        action_taken_by_device_id: row.get::<Option<String>, _>("action_taken_by_device_id"),
    }
}
