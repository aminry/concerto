//! `workspaces` + `workspace_repos` CRUD (Task 19).
//!
//! Schema is locked by migration 0001 (Task 09):
//!
//! ```sql
//! CREATE TABLE workspaces (
//!     id                          TEXT PRIMARY KEY,
//!     name                        TEXT NOT NULL,
//!     slug                        TEXT NOT NULL,
//!     icon                        TEXT,
//!     description                 TEXT,
//!     permission_mode             TEXT CHECK (permission_mode IS NULL OR permission_mode IN ('strict','normal','auto','yolo')),
//!     bypass_destructive_guard    INTEGER CHECK (bypass_destructive_guard IS NULL OR bypass_destructive_guard IN (0,1)),
//!     settings_json               TEXT NOT NULL DEFAULT '{}',
//!     created_at                  INTEGER NOT NULL,
//!     archived_at                 INTEGER,
//!     UNIQUE(slug)
//! );
//!
//! CREATE TABLE workspace_repos (
//!     workspace_id      TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
//!     repository_id     TEXT NOT NULL REFERENCES repositories(id),
//!     position          INTEGER NOT NULL DEFAULT 0,
//!     sparse_cones_json TEXT NOT NULL DEFAULT '[]',
//!     PRIMARY KEY (workspace_id, repository_id)
//! );
//! ```
//!
//! Workspaces are top-level after the Project→Workspace collapse (D5):
//! there is no parent project, and `slug` is globally unique.
//!
//! `permission_mode` is nullable — NULL means "inherit from workspace
//! defaults" per `design/03 §3.2`. Callers serialize permission modes via
//! the lowercase strings the CHECK constraint enforces.
//!
//! The per-`(workspace, repo)` sparse cones live in the
//! `workspace_repos.sparse_cones_json` COLUMN (D6); they are seeded
//! (snapshot) from the repository's `cone_defaults_json` when a repo is
//! attached (D3/D4). See [`get_repo_cones`] / [`update_repos`].
//!
//! ## Repo-ordering contract (FROZEN by Task 306)
//!
//! `workspace_repos.position` is the canonical, deterministic repo order
//! for a workspace. [`update_repos`] assigns `position` = the 0-based
//! index of each `RepositoryId` in the passed slice (insertion order =
//! declaration order = merge/UI order), and [`list_repos`] returns rows
//! ordered by `(position, repository_id)`. This is the ordering Task 309's
//! reference repo ("first by position") and the stable multi-repo UI
//! (Task 322) key off; do **not** re-derive repo order from
//! `repository_id` after this task.

use concerto_error::{Error, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::api::{NewWorkspace, RepositoryId, Workspace, WorkspaceId, WorkspaceRepoCones};

/// SQLite extended result code surfaced when a UNIQUE constraint
/// (here `(slug)`) is violated. Used by the workspace manager's slug
/// auto-suffix retry loop.
pub const SQLITE_CONSTRAINT_UNIQUE: &str = "2067";

/// Insert a new `workspaces` row.
///
/// Takes `&mut SqliteConnection` so a multi-table write (workspace +
/// `workspace_repos` rows) can be scoped under one transaction.
pub async fn insert(conn: &mut SqliteConnection, ws: NewWorkspace) -> Result<WorkspaceId> {
    let id = ws.id.clone();
    sqlx::query(
        "INSERT INTO workspaces (
            id, name, slug, icon, description,
            permission_mode, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id.0)
    .bind(&ws.name)
    .bind(&ws.slug)
    .bind(&ws.icon)
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
        "SELECT id, name, slug, icon, description,
                permission_mode, created_at, archived_at
         FROM workspaces WHERE id = ?",
    )
    .bind(&id.0)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(row_to_workspace))
}

/// List every workspace (read-only). Sorted by `name` for deterministic
/// UI / test output.
pub async fn list_all(pool: &SqlitePool) -> Result<Vec<Workspace>> {
    let rows = sqlx::query(
        "SELECT id, name, slug, icon, description,
                permission_mode, created_at, archived_at
         FROM workspaces ORDER BY name",
    )
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

/// Read the per-`(workspace, repo)` `sparse_cones_json` snapshot (D6).
/// Returns `None` when the repo is not attached to the workspace.
pub async fn get_repo_cones(
    pool: &SqlitePool,
    workspace_id: &WorkspaceId,
    repo_id: &RepositoryId,
) -> Result<Option<String>> {
    let row = sqlx::query(
        "SELECT sparse_cones_json FROM workspace_repos \
         WHERE workspace_id = ? AND repository_id = ?",
    )
    .bind(&workspace_id.0)
    .bind(&repo_id.0)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(|r| r.get::<String, _>("sparse_cones_json")))
}

/// Replace the set of `workspace_repos` rows for a workspace, stamping a
/// deterministic [`position`](self#repo-ordering-contract-frozen-by-task-306)
/// and seeding each row's per-`(workspace, repo)` sparse-cone snapshot (D6).
///
/// Each [`WorkspaceRepoCones`] entry carries the cone JSON snapshot seeded
/// (per D3/D4) from the repository's `cone_defaults_json` at attach time (or
/// `"[]"` via [`WorkspaceRepoCones::empty_cones`]). Editing repo defaults
/// later does NOT mutate these snapshots.
///
/// **Ordering contract (FROZEN by Task 306):** each row's `position` is
/// set to the 0-based index of its entry in `repos`, so the caller's slice
/// order is the canonical repo order (insertion order = declaration order =
/// merge/UI order). [`list_repos`] reads it back in `(position,
/// repository_id)` order. Clears existing junction rows before inserting,
/// so the operation is idempotent under retry and a re-call with a
/// reordered slice re-positions the set.
pub async fn update_repos(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    repos: &[WorkspaceRepoCones],
) -> Result<()> {
    sqlx::query("DELETE FROM workspace_repos WHERE workspace_id = ?")
        .bind(&workspace_id.0)
        .execute(&mut *conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    for (position, r) in repos.iter().enumerate() {
        sqlx::query(
            "INSERT INTO workspace_repos (workspace_id, repository_id, position, sparse_cones_json) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(&workspace_id.0)
        .bind(&r.repository_id.0)
        .bind(position as i64)
        .bind(&r.sparse_cones_json)
        .execute(&mut *conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    }
    Ok(())
}

/// List repository ids attached to a workspace via `workspace_repos`.
///
/// **Ordering contract (FROZEN by Task 306):** rows come back ordered by
/// `(position, repository_id)` — `position` (migration 0009) is the
/// canonical declaration order [`update_repos`] stamped; the
/// `repository_id` tiebreak keeps the read deterministic in the unlikely
/// event two rows ever share a position. Task 309's reference repo is
/// `list_repos(...)[0]`.
pub async fn list_repos(
    pool: &SqlitePool,
    workspace_id: &WorkspaceId,
) -> Result<Vec<RepositoryId>> {
    let rows = sqlx::query(
        "SELECT repository_id FROM workspace_repos WHERE workspace_id = ? \
         ORDER BY position, repository_id",
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

/// Read the raw `workspaces.settings_json` string for `id` (Task 302).
/// Returns `None` when the workspace row does not exist.
///
/// Per-`(workspace, repo)` sparse cones do NOT live in this JSON: after the
/// Project→Workspace collapse (D6) they live in the
/// `workspace_repos.sparse_cones_json` COLUMN (see [`get_repo_cones`]). This `settings_json` blob carries other
/// workspace-level settings (e.g. `permission_mode` defaults). This layer
/// stays dumb storage; callers serialize/deserialize the JSON themselves.
pub async fn get_settings_json(pool: &SqlitePool, id: &WorkspaceId) -> Result<Option<String>> {
    let row = sqlx::query("SELECT settings_json FROM workspaces WHERE id = ?")
        .bind(&id.0)
        .fetch_optional(pool)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(|r| r.get::<String, _>("settings_json")))
}

/// Overwrite `workspaces.settings_json` for `id` with `payload` (Task 302).
///
/// `payload` is a JSON object the caller has already serialized. Callers
/// mutating one settings key must read-modify-write (via
/// [`get_settings_json`]) so they never clobber other settings keys
/// (`permission_mode` overrides, etc.). Per-`(workspace, repo)` sparse
/// cones are NOT stored here — they live in the
/// `workspace_repos.sparse_cones_json` column (D6). Mirrors
/// [`crate::workareas::set_settings_json`].
pub async fn set_settings_json(
    conn: &mut SqliteConnection,
    id: &WorkspaceId,
    payload: &str,
) -> Result<()> {
    sqlx::query("UPDATE workspaces SET settings_json = ? WHERE id = ?")
        .bind(payload)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
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
/// restore inherit-from-workspace-defaults semantics. Task 32 uses this
/// for `Workspaces.UpdateWorkspaceSettings`.
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
        name: row.get::<String, _>("name"),
        slug: row.get::<String, _>("slug"),
        icon: row.get::<Option<String>, _>("icon"),
        description: row.get::<Option<String>, _>("description"),
        permission_mode: row.get::<Option<String>, _>("permission_mode"),
        created_at: row.get::<i64, _>("created_at"),
        archived_at: row.get::<Option<i64>, _>("archived_at"),
    }
}
