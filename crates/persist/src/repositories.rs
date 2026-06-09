//! `repositories` table CRUD (Task 18).
//!
//! Owns the persistence-side API for the V0.1 surface of
//! `crates/gix-wrap`. Schema is locked by migration 0001 (Task 09):
//!
//! Repositories are a global registry after the Project→Workspace
//! collapse (D9): there is no parent project, and `url`/`name` are
//! globally unique. Schema is locked by migration 0001:
//!
//! ```sql
//! CREATE TABLE repositories (
//!     id TEXT PRIMARY KEY,
//!     name TEXT NOT NULL,
//!     url TEXT NOT NULL,
//!     local_path TEXT NOT NULL,
//!     clone_strategy TEXT NOT NULL, -- full | blobless | treeless
//!     default_branch TEXT NOT NULL,
//!     cone_defaults_json TEXT NOT NULL DEFAULT '[]',
//!     fs_monitor_pid INTEGER,
//!     last_fetch_at INTEGER,
//!     UNIQUE(url),
//!     UNIQUE(name)
//! );
//! ```
//!
//! The public types and functions are re-exported through
//! [`crate::api`] so the interface generator surfaces them.

use concerto_error::{Error, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::api::{NewRepository, Repository, RepositoryId};

/// Insert a new `repositories` row.
///
/// Takes `&mut SqliteConnection` rather than the workspace's
/// [`crate::WriterGuard`] so callers can scope a transaction across
/// multiple inserts. The typical pattern is:
///
/// ```ignore
/// let mut w = persist.writer().await;
/// let id = repositories::insert(&mut *w, NewRepository { ... }).await?;
/// ```
pub async fn insert(conn: &mut SqliteConnection, repo: NewRepository) -> Result<RepositoryId> {
    let id = repo.id.clone();
    sqlx::query(
        "INSERT INTO repositories (
            id, name, url, local_path,
            clone_strategy, default_branch
         ) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id.0)
    .bind(&repo.name)
    .bind(&repo.url)
    .bind(&repo.local_path)
    .bind(&repo.clone_strategy)
    .bind(&repo.default_branch)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(id)
}

/// Fetch one repository by id (read-only).
pub async fn get(pool: &SqlitePool, id: &RepositoryId) -> Result<Option<Repository>> {
    let row = sqlx::query(
        "SELECT id, name, url, local_path,
                clone_strategy, default_branch, cone_defaults_json,
                action_prefs_json, last_fetch_at, fs_monitor_pid
         FROM repositories WHERE id = ?",
    )
    .bind(&id.0)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(row_to_repository))
}

/// Fetch one repository by `url` (read-only) — the registry de-dup lookup
/// (D9). The Core checks this before cloning a URL so an already-present
/// repository is reused rather than re-registered.
pub async fn get_by_url(pool: &SqlitePool, url: &str) -> Result<Option<Repository>> {
    let row = sqlx::query(
        "SELECT id, name, url, local_path,
                clone_strategy, default_branch, cone_defaults_json,
                action_prefs_json, last_fetch_at, fs_monitor_pid
         FROM repositories WHERE url = ?",
    )
    .bind(url)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(row_to_repository))
}

/// List every repository in the database (read-only). Sorted by `name`
/// for deterministic UI output. The Task 28 fsmonitor supervisor also
/// walks this list every 30s to probe each recorded daemon PID.
pub async fn list_all(pool: &SqlitePool) -> Result<Vec<Repository>> {
    let rows = sqlx::query(
        "SELECT id, name, url, local_path,
                clone_strategy, default_branch, cone_defaults_json,
                action_prefs_json, last_fetch_at, fs_monitor_pid
         FROM repositories ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_repository).collect())
}

/// Update `last_fetch_at` to `at` (unix epoch milliseconds).
pub async fn update_last_fetch(
    conn: &mut SqliteConnection,
    id: &RepositoryId,
    at: i64,
) -> Result<()> {
    sqlx::query("UPDATE repositories SET last_fetch_at = ? WHERE id = ?")
        .bind(at)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Record (or clear) the `git fsmonitor--daemon` PID for `id`. Pass
/// `None` to clear the column when the supervisor has disabled the
/// daemon for the repo (3-in-60s restart-cap breach, or the underlying
/// filesystem refused the daemon outright).
pub async fn update_fs_monitor_pid(
    conn: &mut SqliteConnection,
    id: &RepositoryId,
    pid: Option<i64>,
) -> Result<()> {
    sqlx::query("UPDATE repositories SET fs_monitor_pid = ? WHERE id = ?")
        .bind(pid)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Overwrite the `cone_defaults_json` column on a `repositories` row with
/// `cones` (the repository's default sparse cone, design/02 §3.2).
///
/// `cones` is serialized to the FROZEN flat JSON `["<cone_path>", …]` shape
/// — the exact encoding [`get`]/[`row_to_repository`] reads back and the
/// three-layer cone resolver decodes as the least-specific layer. An empty
/// slice persists `"[]"` (clears the default). The Core's
/// `RepoManager::set_repo_cone_defaults` calls this before propagating the
/// new cone to every existing workarea of the repo.
pub async fn set_cone_defaults(
    conn: &mut SqliteConnection,
    id: &RepositoryId,
    cones: &[String],
) -> Result<()> {
    let json = serde_json::to_string(cones)
        .map_err(|e| Error::Internal(format!("serialize cone_defaults_json: {e}")))?;
    sqlx::query("UPDATE repositories SET cone_defaults_json = ? WHERE id = ?")
        .bind(&json)
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

fn row_to_repository(row: sqlx::sqlite::SqliteRow) -> Repository {
    Repository {
        id: RepositoryId(row.get::<String, _>("id")),
        name: row.get::<String, _>("name"),
        url: row.get::<String, _>("url"),
        local_path: row.get::<String, _>("local_path"),
        clone_strategy: row.get::<String, _>("clone_strategy"),
        default_branch: row.get::<String, _>("default_branch"),
        // Task 302: the repository-level cone-defaults layer (a flat
        // `["<cone_path>", …]` JSON array, migration 0001 default `'[]'`).
        // The three-layer resolver reads this as the least-specific layer.
        cone_defaults_json: row.get::<String, _>("cone_defaults_json"),
        // Task 310: the per-repo action-prefs local-DB layer (migration
        // 0011, `design/04 §3.13`). A JSON object `{ "<action>": "<pref>" }`;
        // SQL default `'{}'`. The settings resolver reads it under the
        // checked-in `.concerto/action_prefs.toml` override.
        action_prefs_json: row.get::<String, _>("action_prefs_json"),
        last_fetch_at: row.get::<Option<i64>, _>("last_fetch_at"),
        fs_monitor_pid: row.get::<Option<i64>, _>("fs_monitor_pid"),
    }
}
