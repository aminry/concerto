//! `webhook_deliveries` table CRUD (Task 315, migration 0013).
//!
//! Restart-surviving delivery-id idempotency for inbound GitHub webhooks
//! (`design/13 §6.2`, `tasks/v1.0/PHASE3_PLANNING.md §3`/D9). The Core dedupes on
//! the `X-GitHub-Delivery` id **before** HMAC + parse; the dedup is persisted (not
//! in-memory) so a GitHub redelivery seconds after a Core bounce is still caught.
//! The 1h TTL is the prune window swept by [`prune_expired`].
//!
//! Secrets never touch this table — the per-repo HMAC secret lives only in the
//! keychain (`VcsSecretSlot::WebhookSecret`, D4).
//!
//! Schema is locked by migration 0013:
//!
//! ```sql
//! CREATE TABLE webhook_deliveries (
//!     delivery_id TEXT PRIMARY KEY,
//!     repo_id     TEXT NOT NULL,
//!     received_at INTEGER NOT NULL
//! );
//! ```

use concerto_error::{Error, Result};
use sqlx::SqliteConnection;

/// The webhook-delivery idempotency TTL (`design/13 §6.2`): **1 hour**, in
/// milliseconds. A delivery older than this is pruned, so a (rare) GitHub
/// redelivery beyond the window is reprocessed (harmless — the parse + targeted
/// invalidate is itself idempotent against GitHub-as-canonical).
pub const WEBHOOK_DELIVERY_TTL_MS: i64 = 60 * 60 * 1000;

/// Insert a delivery row **iff** its id is not already present, returning whether
/// it was newly inserted: `true` ⇒ first time seen ⇒ **process** the webhook;
/// `false` ⇒ a replay (same `delivery_id`) ⇒ **drop** it (the caller acks 200 so
/// GitHub stops retrying the dupe). Atomic via SQLite's `INSERT OR IGNORE` +
/// `changes()` so two concurrent deliveries of the same id race correctly (only
/// one inserts).
pub async fn insert_delivery_if_absent(
    conn: &mut SqliteConnection,
    delivery_id: &str,
    repo_id: &str,
    received_at: i64,
) -> Result<bool> {
    let result = sqlx::query(
        "INSERT OR IGNORE INTO webhook_deliveries (delivery_id, repo_id, received_at)
         VALUES (?, ?, ?)",
    )
    .bind(delivery_id)
    .bind(repo_id)
    .bind(received_at)
    .execute(&mut *conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    // `rows_affected() == 1` ⇒ the row was newly inserted (not a replay).
    Ok(result.rows_affected() == 1)
}

/// Delete every delivery row older than the TTL window (`now_ms -
/// `[`WEBHOOK_DELIVERY_TTL_MS`]). Returns the number of rows pruned. Idempotent;
/// safe to call on a schedule.
pub async fn prune_expired(conn: &mut SqliteConnection, now_ms: i64) -> Result<u64> {
    let cutoff = now_ms.saturating_sub(WEBHOOK_DELIVERY_TTL_MS);
    let result = sqlx::query("DELETE FROM webhook_deliveries WHERE received_at < ?")
        .bind(cutoff)
        .execute(&mut *conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(result.rows_affected())
}
