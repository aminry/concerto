//! `skills_index` table CRUD (Task 39).
//!
//! Schema is locked by migration 0005 (`tasks/39-skills-registry.md`):
//!
//! ```sql
//! CREATE TABLE skills_index (
//!     id              TEXT PRIMARY KEY,
//!     scope           TEXT NOT NULL CHECK (scope IN
//!                       ('personal','project','plugin','enterprise')),
//!     project_id      TEXT REFERENCES projects(id) ON DELETE CASCADE,
//!     name            TEXT NOT NULL,
//!     slash_command   TEXT,
//!     description     TEXT,
//!     tools_json      TEXT NOT NULL DEFAULT '[]',
//!     source_path     TEXT NOT NULL,
//!     enabled         INTEGER NOT NULL DEFAULT 1,
//!     discovered_at   INTEGER NOT NULL,
//!     UNIQUE(scope, project_id, name)
//! );
//! ```
//!
//! V0.1 ships discovery (personal + project scopes) and per-(scope,
//! project, name) enable/disable. Marketplace install, sandbox try, and
//! invocation tracking are V1.0 per `tasks/39 §"Scope — out"`; the
//! columns that surface those (`marketplace_id`, `pinned_version`,
//! `visibility`, `last_used_at`, `invocation_count`, `kind`) arrive
//! with a later migration once the V1.0 surface is finalised.

use concerto_error::{Error, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::api::{NewSkill, ProjectId, SkillFilter, SkillId, SkillRow, SkillScope};

/// Insert or update a `skills_index` row keyed on
/// `(scope, project_id, name)`. The `enabled` column is preserved across
/// upserts so a user's toggle survives re-discovery; everything else is
/// overwritten with the freshly-discovered values. Returns the id of the
/// row that now matches the key (which may be the existing id when the
/// row already existed, not the caller-supplied one).
pub async fn upsert(conn: &mut SqliteConnection, s: NewSkill) -> Result<SkillId> {
    // SQLite's ON CONFLICT requires the conflict target to honor NULL ==
    // NULL semantics, but UNIQUE(scope, project_id, name) treats NULL
    // project_id as distinct. We work around that by SELECT-then-INSERT-
    // or-UPDATE — keeps the upsert race-tolerant inside the single-
    // writer guarantee of `WriterGuard`.
    let scope_str = s.scope.as_sql_str();
    let project_id_str = s.project_id.as_ref().map(|p| p.0.as_str());

    let existing = sqlx::query(
        "SELECT id FROM skills_index
           WHERE scope = ?
             AND (project_id IS ? OR project_id = ?)
             AND name = ?",
    )
    .bind(scope_str)
    .bind(project_id_str)
    .bind(project_id_str)
    .bind(&s.name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;

    let id = if let Some(row) = existing {
        let existing_id: String = row.get("id");
        sqlx::query(
            "UPDATE skills_index
               SET slash_command = ?,
                   description   = ?,
                   tools_json    = ?,
                   source_path   = ?,
                   discovered_at = ?
             WHERE id = ?",
        )
        .bind(&s.slash_command)
        .bind(&s.description)
        .bind(&s.tools_json)
        .bind(&s.source_path)
        .bind(s.discovered_at)
        .bind(&existing_id)
        .execute(&mut *conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
        SkillId(existing_id)
    } else {
        sqlx::query(
            "INSERT INTO skills_index (
                id, scope, project_id, name, slash_command, description,
                tools_json, source_path, enabled, discovered_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?)",
        )
        .bind(&s.id.0)
        .bind(scope_str)
        .bind(project_id_str)
        .bind(&s.name)
        .bind(&s.slash_command)
        .bind(&s.description)
        .bind(&s.tools_json)
        .bind(&s.source_path)
        .bind(s.discovered_at)
        .execute(&mut *conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
        s.id
    };

    Ok(id)
}

/// Fetch one row by id (read-only). Returns `None` if the id is
/// unknown.
pub async fn get(pool: &SqlitePool, id: &SkillId) -> Result<Option<SkillRow>> {
    let row = sqlx::query(
        "SELECT id, scope, project_id, name, slash_command, description,
                tools_json, source_path, enabled, discovered_at
           FROM skills_index WHERE id = ?",
    )
    .bind(&id.0)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    row.map(row_to_skill).transpose()
}

/// List rows matching `filter`, sorted by `(scope, name)` for
/// deterministic UI output.
pub async fn list(pool: &SqlitePool, filter: &SkillFilter) -> Result<Vec<SkillRow>> {
    // Build a dynamic SQL string with the WHERE clauses we actually
    // need. The set is small and bounded so the cost is negligible.
    let mut sql = String::from(
        "SELECT id, scope, project_id, name, slash_command, description,
                tools_json, source_path, enabled, discovered_at
           FROM skills_index WHERE 1=1",
    );
    if filter.scope.is_some() {
        sql.push_str(" AND scope = ?");
    }
    if filter.project_id.is_some() {
        sql.push_str(" AND project_id = ?");
    }
    if filter.enabled_only {
        sql.push_str(" AND enabled = 1");
    }
    sql.push_str(" ORDER BY scope, name");

    let mut q = sqlx::query(&sql);
    if let Some(scope) = filter.scope {
        q = q.bind(scope.as_sql_str());
    }
    if let Some(project_id) = filter.project_id.as_ref() {
        q = q.bind(project_id.0.clone());
    }
    let rows = q
        .fetch_all(pool)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    rows.into_iter().map(row_to_skill).collect()
}

/// Set `enabled` on a row. Idempotent at the SQL level. Returns
/// `Ok(false)` when the row id is unknown so callers can return
/// `NOT_FOUND` without an extra `SELECT`.
pub async fn set_enabled(conn: &mut SqliteConnection, id: &SkillId, enabled: bool) -> Result<bool> {
    let result = sqlx::query("UPDATE skills_index SET enabled = ? WHERE id = ?")
        .bind(if enabled { 1_i64 } else { 0_i64 })
        .bind(&id.0)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(result.rows_affected() == 1)
}

fn row_to_skill(row: sqlx::sqlite::SqliteRow) -> Result<SkillRow> {
    let scope_str: String = row.get("scope");
    let scope = SkillScope::from_sql_str(&scope_str).ok_or_else(|| {
        Error::Internal(format!(
            "skills_index row has unknown scope {scope_str:?} (CHECK constraint broken?)"
        ))
    })?;
    Ok(SkillRow {
        id: SkillId(row.get::<String, _>("id")),
        scope,
        project_id: row.get::<Option<String>, _>("project_id").map(ProjectId),
        name: row.get::<String, _>("name"),
        slash_command: row.get::<Option<String>, _>("slash_command"),
        description: row.get::<Option<String>, _>("description"),
        tools_json: row.get::<String, _>("tools_json"),
        source_path: row.get::<String, _>("source_path"),
        enabled: row.get::<i64, _>("enabled") != 0,
        discovered_at: row.get::<i64, _>("discovered_at"),
    })
}
