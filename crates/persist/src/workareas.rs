//! `workareas` + `workarea_repos` CRUD (Task 20).
//!
//! Schema is locked by migration 0001 (Task 09):
//!
//! ```sql
//! CREATE TABLE workareas (
//!     id                          TEXT PRIMARY KEY,
//!     workspace_id                TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
//!     composer_name               TEXT NOT NULL,
//!     branch_name                 TEXT NOT NULL,
//!     worktree_root               TEXT NOT NULL,
//!     status                      TEXT NOT NULL CHECK (status IN (
//!         'created','active','running','awaiting','paused','archived','crashed'
//!     )),
//!     permission_mode             TEXT CHECK (permission_mode IS NULL OR permission_mode IN ('strict','normal','auto','yolo')),
//!     bypass_destructive_guard    INTEGER CHECK (bypass_destructive_guard IS NULL OR bypass_destructive_guard IN (0,1)),
//!     created_at                  INTEGER NOT NULL,
//!     archived_at                 INTEGER,
//!     last_activity_at            INTEGER,
//!     UNIQUE(workspace_id, composer_name)
//! );
//!
//! CREATE TABLE workarea_repos (
//!     workarea_id         TEXT NOT NULL REFERENCES workareas(id) ON DELETE CASCADE,
//!     repository_id       TEXT NOT NULL REFERENCES repositories(id),
//!     worktree_path       TEXT NOT NULL,
//!     branch_override     TEXT,
//!     sparse_cones_json   TEXT NOT NULL DEFAULT '[]',
//!     PRIMARY KEY (workarea_id, repository_id)
//! );
//! ```
//!
//! `status` is a lowercase string matching the CHECK constraint;
//! `permission_mode` is nullable for "inherit from parent" per
//! `design/03 §3.2`. The Workspace Manager handles the slug-style retry
//! on UNIQUE(`workspace_id, composer_name`) collisions via
//! [`is_unique_violation`].

use std::collections::HashSet;

use concerto_error::{Error, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::api::{NewWorkarea, NewWorkareaRepo, Workarea, WorkareaId, WorkspaceId};

/// SQLite extended result code for UNIQUE constraint violations. The
/// Workspace Manager retries composer-name allocation on this code.
pub const SQLITE_CONSTRAINT_UNIQUE: &str = "2067";

/// Insert a new `workareas` row.
///
/// Takes `&mut SqliteConnection` so callers can scope the workarea row +
/// junction rows + status transition in one transaction. Status is set
/// to whatever the caller passes (the Workspace Manager inserts with
/// `"created"` and follows up with [`update_status`] for the
/// `created → active` transition inside the same transaction).
pub async fn insert(conn: &mut SqliteConnection, wa: NewWorkarea) -> Result<WorkareaId> {
    let id = wa.id.clone();
    sqlx::query(
        "INSERT INTO workareas (
            id, workspace_id, composer_name, branch_name, worktree_root,
            status, permission_mode, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id.0)
    .bind(&wa.workspace_id)
    .bind(&wa.composer_name)
    .bind(&wa.branch_name)
    .bind(&wa.worktree_root)
    .bind(&wa.status)
    .bind(&wa.permission_mode)
    .bind(wa.created_at)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(id)
}

/// Insert one `workarea_repos` junction row.
///
/// V0.1 ships single-repo workareas so callers invoke this once per
/// create; V1.0's multi-repo path will loop.
pub async fn insert_workarea_repo(conn: &mut SqliteConnection, row: NewWorkareaRepo) -> Result<()> {
    sqlx::query(
        "INSERT INTO workarea_repos (
            workarea_id, repository_id, worktree_path, branch_override
         ) VALUES (?, ?, ?, ?)",
    )
    .bind(&row.workarea_id.0)
    .bind(&row.repository_id.0)
    .bind(&row.worktree_path)
    .bind(&row.branch_override)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Update the `status` column on a `workareas` row.
///
/// Used by the Workspace Manager to drive the `created → active`
/// transition after the on-disk worktree + `.context/` skeleton is in
/// place. The CHECK constraint enforces the allowed string set.
pub async fn update_status(
    conn: &mut SqliteConnection,
    id: &WorkareaId,
    status: &str,
) -> Result<()> {
    sqlx::query("UPDATE workareas SET status = ? WHERE id = ?")
        .bind(status)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Fetch one workarea by id (read-only).
pub async fn get(pool: &SqlitePool, id: &WorkareaId) -> Result<Option<Workarea>> {
    let row = sqlx::query(
        "SELECT id, workspace_id, composer_name, branch_name, worktree_root,
                status, permission_mode, created_at, archived_at, last_activity_at,
                settings_json
         FROM workareas WHERE id = ?",
    )
    .bind(&id.0)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(row_to_workarea))
}

/// List workareas attached to a workspace (read-only). Sorted by
/// `composer_name` for deterministic UI output.
///
/// When `include_archived` is false, rows whose `archived_at` is set
/// are filtered out.
pub async fn list_by_workspace(
    pool: &SqlitePool,
    workspace_id: &WorkspaceId,
    include_archived: bool,
) -> Result<Vec<Workarea>> {
    let sql = if include_archived {
        "SELECT id, workspace_id, composer_name, branch_name, worktree_root,
                status, permission_mode, created_at, archived_at, last_activity_at,
                settings_json
         FROM workareas WHERE workspace_id = ? ORDER BY composer_name"
    } else {
        "SELECT id, workspace_id, composer_name, branch_name, worktree_root,
                status, permission_mode, created_at, archived_at, last_activity_at,
                settings_json
         FROM workareas WHERE workspace_id = ? AND archived_at IS NULL
         ORDER BY composer_name"
    };
    let rows = sqlx::query(sql)
        .bind(&workspace_id.0)
        .fetch_all(pool)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_workarea).collect())
}

/// Mark a workarea archived: sets `archived_at` to `at` (unix epoch ms)
/// AND sets `status` to `'archived'`. Idempotent.
pub async fn archive(conn: &mut SqliteConnection, id: &WorkareaId, at: i64) -> Result<()> {
    sqlx::query("UPDATE workareas SET archived_at = ?, status = 'archived' WHERE id = ?")
        .bind(at)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Restore an archived workarea (Task 31).
///
/// Clears `archived_at`, sets `status = 'active'`, and resets
/// `permission_mode` to `NULL` per `design/03 §3.7` (security stance:
/// restored workareas inherit the workspace's current default rather than
/// silently resuming any prior elevated mode such as `yolo`).
///
/// Idempotent — restoring a non-archived workarea is a no-op at the
/// transition level (status update is still applied but values are
/// unchanged).
pub async fn restore(conn: &mut SqliteConnection, id: &WorkareaId) -> Result<()> {
    sqlx::query(
        "UPDATE workareas
         SET archived_at = NULL, status = 'active', permission_mode = NULL
         WHERE id = ?",
    )
    .bind(&id.0)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// List non-archived workareas in a workspace, returning only the
/// (id, worktree_root, branch_name) fields the archive cascade needs.
///
/// Task 31's `archive_workspace` uses this to enumerate which workareas
/// must be archived in the same transaction as the workspace itself.
/// Returning a narrow projection keeps the query cheap when a workspace
/// has many workareas.
pub async fn list_non_archived_minimal(
    pool: &SqlitePool,
    workspace_id: &WorkspaceId,
) -> Result<Vec<(WorkareaId, String, String)>> {
    let rows = sqlx::query(
        "SELECT id, worktree_root, branch_name
         FROM workareas
         WHERE workspace_id = ? AND archived_at IS NULL",
    )
    .bind(&workspace_id.0)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                WorkareaId(r.get::<String, _>("id")),
                r.get::<String, _>("worktree_root"),
                r.get::<String, _>("branch_name"),
            )
        })
        .collect())
}

/// List every non-archived workarea (across all workspaces). Used by the
/// boot-time crash adoption sweep (`design/03 §6.5`) to probe disks and
/// transition workareas whose worktree has gone missing into the
/// `crashed` state.
pub async fn list_all_non_archived(pool: &SqlitePool) -> Result<Vec<(WorkareaId, String)>> {
    let rows = sqlx::query(
        "SELECT id, worktree_root
         FROM workareas
         WHERE archived_at IS NULL AND status != 'crashed'",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                WorkareaId(r.get::<String, _>("id")),
                r.get::<String, _>("worktree_root"),
            )
        })
        .collect())
}

/// Composer names currently in use within a workspace (sourced from the
/// `workareas` table). Used by the Workspace Manager's allocation loop
/// to find the lowest-index unused composer.
///
/// Archived workareas are still counted as "in use" — keeping the same
/// composer name available after archive avoids accidental confusion if
/// the row is ever re-activated, and the namespace is large enough that
/// this trade-off is invisible to users.
pub async fn list_composer_names_in_workspace(
    pool: &SqlitePool,
    workspace_id: &WorkspaceId,
) -> Result<HashSet<String>> {
    let rows = sqlx::query("SELECT composer_name FROM workareas WHERE workspace_id = ?")
        .bind(&workspace_id.0)
        .fetch_all(pool)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows
        .into_iter()
        .map(|r| r.get::<String, _>("composer_name"))
        .collect())
}

/// List `(repository_id, worktree_path)` pairs attached to a workarea
/// via `workarea_repos`. Used by Task 34's checkpoint path to iterate
/// every repo whose worktree state needs snapshotting on a turn-complete
/// boundary.
///
/// Returns rows in repository_id-ascending order so checkpoint creation
/// is deterministic across multi-repo workareas (V1.0).
pub async fn list_workarea_repos(
    pool: &SqlitePool,
    workarea_id: &WorkareaId,
) -> Result<Vec<(crate::api::RepositoryId, String)>> {
    let rows = sqlx::query(
        "SELECT repository_id, worktree_path
         FROM workarea_repos
         WHERE workarea_id = ?
         ORDER BY repository_id ASC",
    )
    .bind(&workarea_id.0)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                crate::api::RepositoryId(r.get::<String, _>("repository_id")),
                r.get::<String, _>("worktree_path"),
            )
        })
        .collect())
}

/// Look up the `worktree_path` column on the `workarea_repos` junction
/// row for a given (workarea, repository) pair. Used by Task 29's
/// `Workareas.GetWorkareaRepoDiff` handler to resolve the per-repo
/// worktree the diff should run against.
///
/// Returns `None` when no junction row exists for the pair.
pub async fn get_workarea_repo_worktree_path(
    pool: &SqlitePool,
    workarea_id: &WorkareaId,
    repository_id: &crate::api::RepositoryId,
) -> Result<Option<String>> {
    let row = sqlx::query(
        "SELECT worktree_path FROM workarea_repos
         WHERE workarea_id = ? AND repository_id = ?",
    )
    .bind(&workarea_id.0)
    .bind(&repository_id.0)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(|r| r.get::<String, _>("worktree_path")))
}

/// True iff `err` wraps SQLite's `SQLITE_CONSTRAINT_UNIQUE` (extended
/// code `2067`). The Workspace Manager uses this to detect a composer
/// name collision in the UNIQUE(`workspace_id, composer_name`) constraint
/// and retry with an `-N` suffix.
pub fn is_unique_violation(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Database(db) => db.code().as_deref() == Some(SQLITE_CONSTRAINT_UNIQUE),
        _ => false,
    }
}

fn row_to_workarea(row: sqlx::sqlite::SqliteRow) -> Workarea {
    Workarea {
        id: WorkareaId(row.get::<String, _>("id")),
        workspace_id: WorkspaceId(row.get::<String, _>("workspace_id")),
        composer_name: row.get::<String, _>("composer_name"),
        branch_name: row.get::<String, _>("branch_name"),
        worktree_root: row.get::<String, _>("worktree_root"),
        status: row.get::<String, _>("status"),
        permission_mode: row.get::<Option<String>, _>("permission_mode"),
        created_at: row.get::<i64, _>("created_at"),
        archived_at: row.get::<Option<i64>, _>("archived_at"),
        last_activity_at: row.get::<Option<i64>, _>("last_activity_at"),
        settings_json: row.get::<String, _>("settings_json"),
    }
}

/// Overwrite `workareas.permission_mode` for `id`. Pass `None` to
/// restore inherit-from-workspace semantics. Task 32 uses this for
/// `Workareas.UpdateWorkareaPermissionMode`.
pub async fn set_permission_mode(
    conn: &mut SqliteConnection,
    id: &WorkareaId,
    mode: Option<&str>,
) -> Result<()> {
    sqlx::query("UPDATE workareas SET permission_mode = ? WHERE id = ?")
        .bind(mode)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Overwrite `workareas.bypass_destructive_guard` for `id`. Pass `None`
/// to restore inherit-from-workspace semantics. Task 32 uses this for
/// `Workareas.SetWorkareaBypassDestructiveGuard`.
pub async fn set_bypass_destructive_guard(
    conn: &mut SqliteConnection,
    id: &WorkareaId,
    bypass: Option<bool>,
) -> Result<()> {
    sqlx::query("UPDATE workareas SET bypass_destructive_guard = ? WHERE id = ?")
        .bind(bypass.map(|b| b as i64))
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Overwrite a workarea's `settings_json` column with `payload` (a JSON
/// string the caller has already serialized). Used by Task 30's
/// files-to-copy resolver to stamp the idempotency flag after rules are
/// applied. Idempotent — re-running with the same payload is a no-op
/// from the row's perspective.
pub async fn set_settings_json(
    conn: &mut SqliteConnection,
    id: &WorkareaId,
    payload: &str,
) -> Result<()> {
    sqlx::query("UPDATE workareas SET settings_json = ? WHERE id = ?")
        .bind(payload)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}
