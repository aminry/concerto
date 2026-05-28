//! `workspaces` + `workspace_repos` CRUD (Task 19).
//!
//! Schema is locked by migration 0001 (Task 09):
//!
//! ```sql
//! CREATE TABLE workspaces (
//!     id                          TEXT PRIMARY KEY,
//!     project_id                  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
//!     name                        TEXT NOT NULL,
//!     slug                        TEXT NOT NULL,
//!     description                 TEXT,
//!     permission_mode             TEXT CHECK (permission_mode IS NULL OR permission_mode IN ('strict','normal','auto','yolo')),
//!     bypass_destructive_guard    INTEGER CHECK (bypass_destructive_guard IS NULL OR bypass_destructive_guard IN (0,1)),
//!     settings_json               TEXT NOT NULL DEFAULT '{}',
//!     created_at                  INTEGER NOT NULL,
//!     archived_at                 INTEGER,
//!     UNIQUE(project_id, slug)
//! );
//!
//! CREATE TABLE workspace_repos (
//!     workspace_id    TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
//!     repository_id   TEXT NOT NULL REFERENCES repositories(id),
//!     PRIMARY KEY (workspace_id, repository_id)
//! );
//! ```
//!
//! `permission_mode` is nullable — NULL means "inherit from project"
//! per `design/03 §3.2`. Callers serialize permission modes via the
//! lowercase strings the CHECK constraint enforces.

use concerto_error::{Error, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::api::{NewWorkspace, RepositoryId, Workspace, WorkspaceId};

/// SQLite extended result code surfaced when a UNIQUE constraint
/// (here `(project_id, slug)`) is violated. Used by the workspace
/// manager's slug auto-suffix retry loop.
pub const SQLITE_CONSTRAINT_UNIQUE: &str = "2067";

/// Insert a new `workspaces` row.
///
/// Takes `&mut SqliteConnection` so a multi-table write (workspace +
/// `workspace_repos` rows) can be scoped under one transaction.
pub async fn insert(conn: &mut SqliteConnection, ws: NewWorkspace) -> Result<WorkspaceId> {
    let id = ws.id.clone();
    sqlx::query(
        "INSERT INTO workspaces (
            id, project_id, name, slug, description,
            permission_mode, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id.0)
    .bind(&ws.project_id)
    .bind(&ws.name)
    .bind(&ws.slug)
    .bind(&ws.description)
    .bind(&ws.permission_mode)
    .bind(ws.created_at)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(id)
}

/// Fetch one workspace by id (read-only).
pub async fn get(pool: &SqlitePool, id: &WorkspaceId) -> Result<Option<Workspace>> {
    let row = sqlx::query(
        "SELECT id, project_id, name, slug, description,
                permission_mode, created_at, archived_at
         FROM workspaces WHERE id = ?",
    )
    .bind(&id.0)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(row_to_workspace))
}

/// List workspaces in a project (read-only). Sorted by `name`.
pub async fn list_by_project(pool: &SqlitePool, project_id: &str) -> Result<Vec<Workspace>> {
    let rows = sqlx::query(
        "SELECT id, project_id, name, slug, description,
                permission_mode, created_at, archived_at
         FROM workspaces WHERE project_id = ? ORDER BY name",
    )
    .bind(project_id)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_workspace).collect())
}

/// Mark a workspace archived by setting `archived_at` to `at` (unix
/// epoch ms). Idempotent: re-archiving overwrites the prior timestamp.
pub async fn archive(conn: &mut SqliteConnection, id: &WorkspaceId, at: i64) -> Result<()> {
    sqlx::query("UPDATE workspaces SET archived_at = ? WHERE id = ?")
        .bind(at)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Restore an archived workspace (Task 31).
///
/// Clears `archived_at` only. Per `design/03 §3.7`, restoring a workspace
/// does NOT auto-restore its workareas — those remain individually
/// archived and the user restores them one at a time. Idempotent.
pub async fn restore(conn: &mut SqliteConnection, id: &WorkspaceId) -> Result<()> {
    sqlx::query("UPDATE workspaces SET archived_at = NULL WHERE id = ?")
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Replace the set of `workspace_repos` rows for a workspace.
///
/// V0.1 only ever writes a single repo; the API is plural so V1.0
/// multi-repo workspaces can re-use it. Clears existing junction rows
/// before inserting, so the operation is idempotent under retry.
pub async fn update_repos(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    repo_ids: &[RepositoryId],
) -> Result<()> {
    sqlx::query("DELETE FROM workspace_repos WHERE workspace_id = ?")
        .bind(&workspace_id.0)
        .execute(&mut *conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    for repo_id in repo_ids {
        sqlx::query("INSERT INTO workspace_repos (workspace_id, repository_id) VALUES (?, ?)")
            .bind(&workspace_id.0)
            .bind(&repo_id.0)
            .execute(&mut *conn)
            .await
            .map_err(|e| Error::Sqlx(Box::new(e)))?;
    }
    Ok(())
}

/// List repository ids attached to a workspace via `workspace_repos`.
/// Sorted by `repository_id` so output is deterministic.
pub async fn list_repos(
    pool: &SqlitePool,
    workspace_id: &WorkspaceId,
) -> Result<Vec<RepositoryId>> {
    let rows = sqlx::query(
        "SELECT repository_id FROM workspace_repos WHERE workspace_id = ? ORDER BY repository_id",
    )
    .bind(&workspace_id.0)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows
        .into_iter()
        .map(|r| RepositoryId(r.get::<String, _>("repository_id")))
        .collect())
}

/// True iff the provided sqlx error wraps SQLite's
/// `SQLITE_CONSTRAINT_UNIQUE` (extended code `2067`). The workspace
/// manager uses this to drive its slug auto-suffix retry.
pub fn is_unique_violation(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Database(db) => db.code().as_deref() == Some(SQLITE_CONSTRAINT_UNIQUE),
        _ => false,
    }
}

/// Overwrite `workspaces.permission_mode` for `id`. Pass `None` to
/// restore inherit-from-project semantics. Task 32 uses this for
/// `Workspaces.UpdateWorkspaceSettings`.
pub async fn set_permission_mode(
    conn: &mut SqliteConnection,
    id: &WorkspaceId,
    mode: Option<&str>,
) -> Result<()> {
    sqlx::query("UPDATE workspaces SET permission_mode = ? WHERE id = ?")
        .bind(mode)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

fn row_to_workspace(row: sqlx::sqlite::SqliteRow) -> Workspace {
    Workspace {
        id: WorkspaceId(row.get::<String, _>("id")),
        project_id: row.get::<String, _>("project_id"),
        name: row.get::<String, _>("name"),
        slug: row.get::<String, _>("slug"),
        description: row.get::<Option<String>, _>("description"),
        permission_mode: row.get::<Option<String>, _>("permission_mode"),
        created_at: row.get::<i64, _>("created_at"),
        archived_at: row.get::<Option<i64>, _>("archived_at"),
    }
}
