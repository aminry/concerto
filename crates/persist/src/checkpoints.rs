//! `checkpoints` table CRUD (Task 34).
//!
//! Schema is locked by migration 0001 (Task 09):
//!
//! ```sql
//! CREATE TABLE checkpoints (
//!     id              TEXT PRIMARY KEY,
//!     workarea_id     TEXT NOT NULL REFERENCES workareas(id) ON DELETE CASCADE,
//!     repository_id   TEXT NOT NULL REFERENCES repositories(id),
//!     chat_message_id TEXT NOT NULL REFERENCES chat_messages(id),
//!     git_ref         TEXT NOT NULL,
//!     created_at      INTEGER NOT NULL,
//!     diff_stats_json TEXT
//! );
//! ```
//!
//! A turn-complete event spawns one checkpoint row per repo touched by
//! the workarea (V0.1 single-repo workareas → usually one row).
//! `chat_message_id` ties siblings together so `revert_to_checkpoint`
//! can reverse the whole turn atomically. `git_ref` is the namespaced
//! ref name `refs/concerto/checkpoints/<workarea_id>/<repository_id>/<n>`
//! where `n` is monotonic per `(workarea_id, repository_id)`.

use concerto_error::{Error, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::api::{RepositoryId, SessionId, WorkareaId};

/// Insert-time shape for a `checkpoints` row.
///
/// `id` is a caller-allocated UUIDv7. `git_ref` is the FROZEN
/// `refs/concerto/checkpoints/<workarea>/<repo>/<n>` form so callers
/// don't have to re-derive it from `(workarea, repo, n)` on every read.
#[derive(Debug, Clone)]
pub struct NewCheckpoint {
    pub id: String,
    pub workarea_id: WorkareaId,
    pub repository_id: RepositoryId,
    pub chat_message_id: String,
    pub git_ref: String,
    pub created_at: i64,
    pub diff_stats_json: Option<String>,
}

/// Row-shaped projection of a `checkpoints` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub id: String,
    pub workarea_id: WorkareaId,
    pub repository_id: RepositoryId,
    pub chat_message_id: String,
    pub git_ref: String,
    pub created_at: i64,
    pub diff_stats_json: Option<String>,
}

/// Insert a new `checkpoints` row.
pub async fn insert(conn: &mut SqliteConnection, row: NewCheckpoint) -> Result<String> {
    let id = row.id.clone();
    sqlx::query(
        "INSERT INTO checkpoints (
            id, workarea_id, repository_id, chat_message_id, git_ref, created_at, diff_stats_json
         ) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&row.id)
    .bind(&row.workarea_id.0)
    .bind(&row.repository_id.0)
    .bind(&row.chat_message_id)
    .bind(&row.git_ref)
    .bind(row.created_at)
    .bind(&row.diff_stats_json)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(id)
}

/// Fetch one checkpoint by id (read-only).
pub async fn get(pool: &SqlitePool, id: &str) -> Result<Option<Checkpoint>> {
    let row = sqlx::query(
        "SELECT id, workarea_id, repository_id, chat_message_id, git_ref, created_at, diff_stats_json
           FROM checkpoints WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(row_to_checkpoint))
}

/// List every checkpoint for a workarea, newest first.
pub async fn list_by_workarea(
    pool: &SqlitePool,
    workarea_id: &WorkareaId,
) -> Result<Vec<Checkpoint>> {
    let rows = sqlx::query(
        "SELECT id, workarea_id, repository_id, chat_message_id, git_ref, created_at, diff_stats_json
           FROM checkpoints WHERE workarea_id = ? ORDER BY created_at DESC",
    )
    .bind(&workarea_id.0)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_checkpoint).collect())
}

/// Sibling checkpoints (across repos) that share the same
/// `chat_message_id`. A multi-repo turn writes N rows; revert needs
/// all of them so the worktree-vs-branch resets stay consistent.
pub async fn get_with_siblings(
    pool: &SqlitePool,
    chat_message_id: &str,
) -> Result<Vec<Checkpoint>> {
    let rows = sqlx::query(
        "SELECT id, workarea_id, repository_id, chat_message_id, git_ref, created_at, diff_stats_json
           FROM checkpoints WHERE chat_message_id = ? ORDER BY repository_id ASC",
    )
    .bind(chat_message_id)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_checkpoint).collect())
}

/// Look up the highest `<n>` suffix used by an existing
/// `refs/concerto/checkpoints/<workarea>/<repo>/<n>` for the
/// `(workarea_id, repository_id)` pair. Returns `0` when no checkpoints
/// exist yet so the caller can write `max_n + 1`.
///
/// Implementation parses the trailing path segment of `git_ref` rather
/// than storing `n` as a column — the ref name is the canonical fact and
/// keeping the schema lean avoids an extra column to keep in sync.
pub async fn max_n_for(
    pool: &SqlitePool,
    workarea_id: &WorkareaId,
    repository_id: &RepositoryId,
) -> Result<i64> {
    let rows =
        sqlx::query("SELECT git_ref FROM checkpoints WHERE workarea_id = ? AND repository_id = ?")
            .bind(&workarea_id.0)
            .bind(&repository_id.0)
            .fetch_all(pool)
            .await
            .map_err(|e| Error::Sqlx(Box::new(e)))?;
    let mut max_n: i64 = 0;
    for r in rows {
        let git_ref: String = r.get("git_ref");
        if let Some(n) = parse_trailing_n(&git_ref) {
            if n > max_n {
                max_n = n;
            }
        }
    }
    Ok(max_n)
}

/// List checkpoint rows for every session that has ever lived on a
/// workarea — surfaced for the `Sessions.RevertToCheckpoint` happy
/// path that needs to look up the workarea's siblings without trusting
/// the gRPC caller to know the chat message id.
///
/// Returns one row per `(session.id, checkpoint.id)` join. Used by the
/// audit log handoff in V1.0; V0.1 callers only need the
/// [`get_with_siblings`] lookup.
#[allow(dead_code)]
pub async fn list_for_session(
    pool: &SqlitePool,
    session_id: &SessionId,
) -> Result<Vec<Checkpoint>> {
    let rows = sqlx::query(
        "SELECT c.id, c.workarea_id, c.repository_id, c.chat_message_id, c.git_ref,
                c.created_at, c.diff_stats_json
           FROM checkpoints c
           JOIN sessions s ON s.workarea_id = c.workarea_id
          WHERE s.id = ? ORDER BY c.created_at DESC",
    )
    .bind(&session_id.0)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_checkpoint).collect())
}

fn row_to_checkpoint(row: sqlx::sqlite::SqliteRow) -> Checkpoint {
    Checkpoint {
        id: row.get::<String, _>("id"),
        workarea_id: WorkareaId(row.get::<String, _>("workarea_id")),
        repository_id: RepositoryId(row.get::<String, _>("repository_id")),
        chat_message_id: row.get::<String, _>("chat_message_id"),
        git_ref: row.get::<String, _>("git_ref"),
        created_at: row.get::<i64, _>("created_at"),
        diff_stats_json: row.get::<Option<String>, _>("diff_stats_json"),
    }
}

fn parse_trailing_n(git_ref: &str) -> Option<i64> {
    git_ref
        .rsplit('/')
        .next()
        .and_then(|s| s.parse::<i64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trailing_n_from_ref() {
        assert_eq!(
            parse_trailing_n("refs/concerto/checkpoints/wa-1/repo-1/3"),
            Some(3)
        );
        assert_eq!(
            parse_trailing_n("refs/concerto/checkpoints/wa/r/42"),
            Some(42)
        );
        assert_eq!(parse_trailing_n("refs/heads/main"), None);
        assert_eq!(parse_trailing_n(""), None);
    }
}
