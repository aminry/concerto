//! `projects` table CRUD (Task 19).
//!
//! V0.1 ships **no** `Projects` gRPC service — workspace creation requires
//! a parent project row (FK `workspaces.project_id`), but the only way to
//! create one in V0.1 is through these helpers. Integration tests + future
//! desktop bootstrap paths call into `insert` directly. A real `Projects`
//! service lands in a later phase.
//!
//! Schema is locked by migration 0001 (Task 09):
//!
//! ```sql
//! CREATE TABLE projects (
//!     id            TEXT PRIMARY KEY,
//!     name          TEXT NOT NULL,
//!     icon          TEXT,
//!     created_at    INTEGER NOT NULL,
//!     archived_at   INTEGER,
//!     settings_json TEXT NOT NULL DEFAULT '{}'
//! );
//! ```
//!
//! The public types and functions are re-exported through
//! [`crate::api`] so the interface generator surfaces them.

use concerto_error::{Error, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::api::{NewProject, Project, ProjectId};

/// Insert a new `projects` row.
///
/// Takes `&mut SqliteConnection` so callers can scope a transaction
/// across multiple inserts. `created_at` is supplied by the caller as
/// unix epoch milliseconds — keeps this layer pure (no wall-clock reads).
pub async fn insert(conn: &mut SqliteConnection, project: NewProject) -> Result<ProjectId> {
    let id = project.id.clone();
    sqlx::query("INSERT INTO projects (id, name, icon, created_at) VALUES (?, ?, ?, ?)")
        .bind(&id.0)
        .bind(&project.name)
        .bind(&project.icon)
        .bind(project.created_at)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(id)
}

/// Fetch one project by id (read-only).
pub async fn get(pool: &SqlitePool, id: &ProjectId) -> Result<Option<Project>> {
    let row = sqlx::query(
        "SELECT id, name, icon, created_at, archived_at
         FROM projects WHERE id = ?",
    )
    .bind(&id.0)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(row_to_project))
}

/// List every project (read-only). Sorted by `name` for deterministic
/// UI / test output.
pub async fn list_all(pool: &SqlitePool) -> Result<Vec<Project>> {
    let rows = sqlx::query(
        "SELECT id, name, icon, created_at, archived_at
         FROM projects ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_project).collect())
}

fn row_to_project(row: sqlx::sqlite::SqliteRow) -> Project {
    Project {
        id: ProjectId(row.get::<String, _>("id")),
        name: row.get::<String, _>("name"),
        icon: row.get::<Option<String>, _>("icon"),
        created_at: row.get::<i64, _>("created_at"),
        archived_at: row.get::<Option<i64>, _>("archived_at"),
    }
}
