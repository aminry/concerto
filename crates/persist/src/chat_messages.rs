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
//! Task 34 only needs two helpers: [`insert`] (used by the in-process
//! checkpoint test to set up a chat message to reference), and
//! [`soft_delete_after`] (used by the revert path to mark every message
//! later than the checkpoint as superseded). V1.0 may add a richer CRUD
//! when the maestro chat surface lands.

use concerto_error::{Error, Result};
use sqlx::SqliteConnection;

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
}

/// Insert a new `chat_messages` row.
pub async fn insert(conn: &mut SqliteConnection, row: NewChatMessage) -> Result<String> {
    let id = row.id.clone();
    sqlx::query(
        "INSERT INTO chat_messages (
            id, chat_id, role, content_json, created_at, parent_id, superseded_by
         ) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&row.id)
    .bind(&row.chat_id)
    .bind(&row.role)
    .bind(&row.content_json)
    .bind(row.created_at)
    .bind(&row.parent_id)
    .bind(&row.superseded_by)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(id)
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
