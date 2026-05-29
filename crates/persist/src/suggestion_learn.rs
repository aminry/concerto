//! `suggestion_learn` table CRUD (Task 40).
//!
//! Schema is locked by migration 0006 (`tasks/40-suggestion-rule-engine.md`):
//!
//! ```sql
//! CREATE TABLE suggestion_learn (
//!     id            TEXT PRIMARY KEY,
//!     workarea_id   TEXT REFERENCES workareas(id) ON DELETE CASCADE,
//!     rule_id       TEXT NOT NULL,
//!     outcome       TEXT NOT NULL,
//!     context_hash  TEXT NOT NULL DEFAULT '',
//!     created_at    INTEGER NOT NULL
//! );
//! ```
//!
//! V0.1 ships [`insert`] + [`list_by_workarea`] only — the rule engine
//! does NOT write to this table itself. The
//! `Suggestions.RecordSuggestionOutcome` RPC currently just logs via
//! `tracing::info!` (`tasks/40 §"Implementation notes"`); V1.0's learning
//! loop will land behind the same RPC and start populating rows.

use concerto_error::{Error, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::api::{NewSuggestionLearn, SuggestionLearn, SuggestionLearnId, WorkareaId};

/// Insert one `suggestion_learn` row. Caller supplies the id (typically a
/// UUIDv7) and the `created_at` epoch-millis stamp — this layer is dumb
/// storage and does not read the wall clock.
pub async fn insert(conn: &mut SqliteConnection, s: NewSuggestionLearn) -> Result<()> {
    sqlx::query(
        "INSERT INTO suggestion_learn (
            id, workarea_id, rule_id, outcome, context_hash, created_at
         ) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&s.id.0)
    .bind(s.workarea_id.as_ref().map(|w| w.0.as_str()))
    .bind(&s.rule_id)
    .bind(&s.outcome)
    .bind(&s.context_hash)
    .bind(s.created_at)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// List rows for a workarea, newest first. Read-only — does not require
/// the writer guard.
pub async fn list_by_workarea(
    pool: &SqlitePool,
    workarea_id: &WorkareaId,
) -> Result<Vec<SuggestionLearn>> {
    let rows = sqlx::query(
        "SELECT id, workarea_id, rule_id, outcome, context_hash, created_at
         FROM suggestion_learn
         WHERE workarea_id = ?
         ORDER BY created_at DESC",
    )
    .bind(&workarea_id.0)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_learn).collect())
}

fn row_to_learn(row: sqlx::sqlite::SqliteRow) -> SuggestionLearn {
    SuggestionLearn {
        id: SuggestionLearnId(row.get::<String, _>("id")),
        workarea_id: row.get::<Option<String>, _>("workarea_id").map(WorkareaId),
        rule_id: row.get::<String, _>("rule_id"),
        outcome: row.get::<String, _>("outcome"),
        context_hash: row.get::<String, _>("context_hash"),
        created_at: row.get::<i64, _>("created_at"),
    }
}
