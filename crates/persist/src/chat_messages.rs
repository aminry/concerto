//! `chat_messages` table helpers (Task 34).
//!
//! The `chat_messages` schema is locked by migration 0001 (Task 09):
//!
//! ```sql
//! CREATE TABLE chat_messages (
//!     id              TEXT PRIMARY KEY,
//!     chat_id         TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
//!     role            TEXT NOT NULL CHECK (role IN ('user','assistant','system','tool')),
//!     content_json    TEXT NOT NULL,
//!     created_at      INTEGER NOT NULL,
//!     parent_id       TEXT REFERENCES chat_messages(id),
//!     superseded_by   TEXT REFERENCES chat_messages(id)
//! );
//! ```
//!
//! Migration 0016 (Task 410) adds the nullable `metadata TEXT` column — the
//! carrier for the Maestro daily-condensation pass. A daily summary is an
//! ordinary `chat_messages` row whose text is `content_json` and whose
//! classification is `metadata.role_extra='daily_summary'` (`design/08
//! §3.7/§4.1`, D12). The tag lives in `metadata`, never in `content_json`.
//!
//! Task 34 needed two helpers: [`insert`] (used by the in-process checkpoint
//! test to set up a chat message to reference) and [`soft_delete_after`] (the
//! revert path marking every message later than the checkpoint as superseded).
//! Task 410 adds the maestro-chat read/write surface: [`list_in_day_range`]
//! (the verbatim/condense window selector), [`insert_daily_summary`] (the
//! tagged-summary writer), and [`list_daily_summaries`] (the summary reader),
//! plus the [`ChatMessage`] read-back struct. Accessors mirror
//! [`crate::schedules`]: free `pub async fn`s, writes over `&mut
//! SqliteConnection` and reads over `&SqlitePool`, errors wrapped via
//! `Error::Sqlx`, and a private `row_to_*` projector.

use concerto_error::{Error, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};

/// Insert-time shape for a `chat_messages` row.
///
/// `parent_id` and `superseded_by` are `Option<String>` mirroring the
/// nullable FKs. Caller-supplied UUIDv7 keeps the chronological
/// ordering on `created_at` deterministic.
#[derive(Debug, Clone)]
pub struct NewChatMessage {
    pub id: String,
    pub chat_id: String,
    /// One of `user|assistant|system|tool` per the CHECK set.
    pub role: String,
    pub content_json: String,
    pub created_at: i64,
    pub parent_id: Option<String>,
    pub superseded_by: Option<String>,
    /// Nullable JSON tag (migration 0016). `None` for ordinary rows; the
    /// daily-condensation pass writes `{"role_extra":"daily_summary"}` here.
    pub metadata: Option<String>,
}

/// Read-back shape for a `chat_messages` row (Task 410). Mirrors
/// [`NewChatMessage`] plus the read-only `id`. The maestro condensation
/// window (`crate`-external) projects these into the agent input window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub id: String,
    pub chat_id: String,
    pub role: String,
    pub content_json: String,
    pub created_at: i64,
    pub parent_id: Option<String>,
    pub superseded_by: Option<String>,
    pub metadata: Option<String>,
}

/// Insert a new `chat_messages` row.
pub async fn insert(conn: &mut SqliteConnection, row: NewChatMessage) -> Result<String> {
    let id = row.id.clone();
    sqlx::query(
        "INSERT INTO chat_messages (
            id, chat_id, role, content_json, created_at, parent_id, superseded_by, metadata
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&row.id)
    .bind(&row.chat_id)
    .bind(&row.role)
    .bind(&row.content_json)
    .bind(row.created_at)
    .bind(&row.parent_id)
    .bind(&row.superseded_by)
    .bind(&row.metadata)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(id)
}

/// The FROZEN tag a daily-summary row carries in its `metadata` column
/// (`design/08 §4.1`, D12). The summary *text* lives in `content_json`; this
/// JSON object lives in `metadata` and is what `list_daily_summaries` filters
/// on. Kept here so the writer and the read filter cannot drift apart.
pub const DAILY_SUMMARY_METADATA: &str = r#"{"role_extra":"daily_summary"}"#;

/// Select the `chat_messages` rows for `chat_id` in `[start_ms, end_ms)`,
/// non-superseded, ascending by `created_at` (read path).
///
/// Used for BOTH the 24-48h condense slice and the last-24h verbatim slice of
/// the Maestro condensation window. Superseded rows (rewound history) are
/// excluded so the agent never re-reads history the user reverted.
pub async fn list_in_day_range(
    pool: &SqlitePool,
    chat_id: &str,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<ChatMessage>> {
    let rows = sqlx::query(
        "SELECT id, chat_id, role, content_json, created_at, parent_id, superseded_by, metadata
         FROM chat_messages
         WHERE chat_id = ?
           AND created_at >= ?
           AND created_at < ?
           AND superseded_by IS NULL
         ORDER BY created_at ASC",
    )
    .bind(chat_id)
    .bind(start_ms)
    .bind(end_ms)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_chat_message).collect())
}

/// Persist a one-paragraph daily summary as a `chat_messages` row tagged
/// `metadata.role_extra='daily_summary'` (write path).
///
/// Thin wrapper over [`insert`] that hard-codes `role='assistant'`, no
/// `parent_id`/`superseded_by`, and `metadata = DAILY_SUMMARY_METADATA`. The
/// summary text is `content_json`; the classification is `metadata` — the tag
/// is NEVER folded into `content_json` (D12).
pub async fn insert_daily_summary(
    conn: &mut SqliteConnection,
    chat_id: &str,
    id: &str,
    content_json: &str,
    created_at: i64,
) -> Result<String> {
    insert(
        conn,
        NewChatMessage {
            id: id.to_string(),
            chat_id: chat_id.to_string(),
            role: "assistant".to_string(),
            content_json: content_json.to_string(),
            created_at,
            parent_id: None,
            superseded_by: None,
            metadata: Some(DAILY_SUMMARY_METADATA.to_string()),
        },
    )
    .await
}

/// List the daily summaries for `chat_id` (rows tagged
/// `metadata.role_extra='daily_summary'`), non-superseded, ascending (read
/// path). The `daily_summaries[:weekly]` source for the agent input window.
///
/// Uses SQLite's JSON1 `json_extract` (already linked via `chats.settings_json`
/// usage elsewhere). If a future build disables JSON1 this can fall back to a
/// `metadata LIKE '%"role_extra":"daily_summary"%'` filter.
pub async fn list_daily_summaries(pool: &SqlitePool, chat_id: &str) -> Result<Vec<ChatMessage>> {
    let rows = sqlx::query(
        "SELECT id, chat_id, role, content_json, created_at, parent_id, superseded_by, metadata
         FROM chat_messages
         WHERE chat_id = ?
           AND metadata IS NOT NULL
           AND json_extract(metadata, '$.role_extra') = 'daily_summary'
           AND superseded_by IS NULL
         ORDER BY created_at ASC",
    )
    .bind(chat_id)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_chat_message).collect())
}

/// Mark every chat message in `chat_id` whose `created_at >
/// checkpoint_created_at` as superseded by `superseded_by_message_id`.
///
/// This is the Task 34 soft-delete primitive used by
/// `Sessions.RevertToCheckpoint`: the checkpoint's `chat_message_id`
/// serves as the new tip of the conversation, and every message later
/// than the checkpoint is "rewound" by pointing `superseded_by` at the
/// checkpoint's message. The UI hides superseded messages by default;
/// they remain in the DB for audit + (V1.0) rebranching.
///
/// Returns the number of rows touched. Already-superseded rows are
/// re-superseded by a later revert call — design choice: the last
/// revert wins (`superseded_by` is overwritten, not chained). V1.0
/// rebranching may swap to chain semantics.
pub async fn soft_delete_after(
    conn: &mut SqliteConnection,
    chat_id: &str,
    checkpoint_created_at: i64,
    superseded_by_message_id: &str,
) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE chat_messages
            SET superseded_by = ?
          WHERE chat_id = ?
            AND created_at > ?
            AND id != ?",
    )
    .bind(superseded_by_message_id)
    .bind(chat_id)
    .bind(checkpoint_created_at)
    .bind(superseded_by_message_id)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(res.rows_affected())
}

fn row_to_chat_message(row: sqlx::sqlite::SqliteRow) -> ChatMessage {
    ChatMessage {
        id: row.get::<String, _>("id"),
        chat_id: row.get::<String, _>("chat_id"),
        role: row.get::<String, _>("role"),
        content_json: row.get::<String, _>("content_json"),
        created_at: row.get::<i64, _>("created_at"),
        parent_id: row.get::<Option<String>, _>("parent_id"),
        superseded_by: row.get::<Option<String>, _>("superseded_by"),
        metadata: row.get::<Option<String>, _>("metadata"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Persistence, PersistenceConfig};

    async fn fresh_db() -> (tempfile::TempDir, Persistence) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let persist = Persistence::open(PersistenceConfig {
            db_path,
            max_readers: 2,
        })
        .await
        .expect("open");
        (dir, persist)
    }

    /// Bootstrap a `kind='maestro'` chat (session_id NULL) to attach rows to.
    async fn maestro_chat(persist: &Persistence, chat_id: &str) {
        let mut w = persist.writer().await;
        sqlx::query(
            "INSERT INTO chats (id, session_id, kind, created_at) VALUES (?, NULL, 'maestro', 0)",
        )
        .bind(chat_id)
        .execute(&mut *w)
        .await
        .expect("insert maestro chat");
    }

    async fn insert_msg(
        persist: &Persistence,
        chat_id: &str,
        id: &str,
        created_at: i64,
        superseded_by: Option<&str>,
        metadata: Option<&str>,
    ) {
        let mut w = persist.writer().await;
        insert(
            &mut w,
            NewChatMessage {
                id: id.to_string(),
                chat_id: chat_id.to_string(),
                role: "user".to_string(),
                content_json: format!("{{\"text\":\"{id}\"}}"),
                created_at,
                parent_id: None,
                superseded_by: superseded_by.map(str::to_string),
                metadata: metadata.map(str::to_string),
            },
        )
        .await
        .expect("insert msg");
    }

    #[tokio::test]
    async fn metadata_round_trips_some_and_none() {
        let (_dir, persist) = fresh_db().await;
        maestro_chat(&persist, "c1").await;
        insert_msg(
            &persist,
            "c1",
            "tagged",
            10,
            None,
            Some(r#"{"role_extra":"daily_summary"}"#),
        )
        .await;
        insert_msg(&persist, "c1", "plain", 20, None, None).await;

        let rows = list_in_day_range(persist.readers(), "c1", 0, 100)
            .await
            .expect("range");
        assert_eq!(rows.len(), 2);
        let tagged = rows.iter().find(|r| r.id == "tagged").unwrap();
        assert_eq!(
            tagged.metadata.as_deref(),
            Some(r#"{"role_extra":"daily_summary"}"#)
        );
        let plain = rows.iter().find(|r| r.id == "plain").unwrap();
        assert_eq!(plain.metadata, None, "metadata=None reads back NULL");
    }

    #[tokio::test]
    async fn list_in_day_range_is_half_open_ordered_and_excludes_superseded() {
        let (_dir, persist) = fresh_db().await;
        maestro_chat(&persist, "c1").await;
        // A supersede target so the FK is satisfiable.
        insert_msg(&persist, "c1", "tip", 5, None, None).await;
        insert_msg(&persist, "c1", "before", 9, None, None).await; // < start: excluded
        insert_msg(&persist, "c1", "at-start", 10, None, None).await; // included
        insert_msg(&persist, "c1", "mid", 15, None, None).await; // included
        insert_msg(&persist, "c1", "superseded", 16, Some("tip"), None).await; // excluded
        insert_msg(&persist, "c1", "at-end", 20, None, None).await; // == end: excluded

        let rows = list_in_day_range(persist.readers(), "c1", 10, 20)
            .await
            .expect("range");
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["at-start", "mid"],
            "half-open, ordered, no superseded"
        );
    }

    #[tokio::test]
    async fn insert_daily_summary_tags_metadata_not_content() {
        let (_dir, persist) = fresh_db().await;
        maestro_chat(&persist, "c1").await;
        let summary_text = r#"{"text":"the day in one paragraph"}"#;
        {
            let mut w = persist.writer().await;
            insert_daily_summary(&mut w, "c1", "sum-1", summary_text, 86_400_000)
                .await
                .expect("insert summary");
        }

        let rows = list_daily_summaries(persist.readers(), "c1")
            .await
            .expect("list summaries");
        assert_eq!(rows.len(), 1);
        let s = &rows[0];
        assert_eq!(s.role, "assistant", "summaries are assistant rows");
        assert_eq!(s.content_json, summary_text, "text in content_json");
        assert!(
            !s.content_json.contains("role_extra"),
            "the tag must NOT live in content_json"
        );
        assert_eq!(
            s.metadata.as_deref(),
            Some(DAILY_SUMMARY_METADATA),
            "tag lives in metadata"
        );
    }

    #[tokio::test]
    async fn list_daily_summaries_returns_only_tagged_rows_ascending() {
        let (_dir, persist) = fresh_db().await;
        maestro_chat(&persist, "c1").await;
        // Two summaries (out of order) + an ordinary message + a differently
        // tagged row — only the two daily-summaries come back, ascending.
        {
            let mut w = persist.writer().await;
            insert_daily_summary(&mut w, "c1", "sum-late", "{}", 200)
                .await
                .unwrap();
            insert_daily_summary(&mut w, "c1", "sum-early", "{}", 100)
                .await
                .unwrap();
        }
        insert_msg(&persist, "c1", "ordinary", 150, None, None).await;
        insert_msg(
            &persist,
            "c1",
            "other-tag",
            175,
            None,
            Some(r#"{"role_extra":"digest"}"#),
        )
        .await;

        let rows = list_daily_summaries(persist.readers(), "c1")
            .await
            .expect("list summaries");
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["sum-early", "sum-late"]);
    }
}
