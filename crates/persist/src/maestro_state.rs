//! `maestro_state` singleton accessors + the `chats(kind='maestro')`
//! singleton-chat bootstrap (Task 403).
//!
//! Schema is locked by migration 0015 (`design/08 §4.1`):
//!
//! ```sql
//! CREATE TABLE maestro_state (
//!     id              INTEGER PRIMARY KEY CHECK (id = 1),  -- singleton
//!     daily_in_today  INTEGER NOT NULL DEFAULT 0,
//!     daily_out_today INTEGER NOT NULL DEFAULT 0,
//!     budget_resets_at INTEGER NOT NULL,
//!     last_digest_at  INTEGER,
//!     enabled         INTEGER NOT NULL DEFAULT 1
//! );
//! ```
//!
//! This is the persistence root of the Maestro budget + lifecycle state.
//! These accessors are deliberately thin storage seams — the budget *policy*
//! (the 200K/50K caps, inert-on-exhaust, the UTC-midnight reset clock) is
//! Task 412's, the digest cadence is Task 414's, and the daily-summary
//! `chat_messages` are Task 410's. FROZEN per
//! `tasks/v1.0/PHASE4_PLANNING.md §4.6` (D6).
//!
//! Pattern mirrors [`crate::schedules`]: free `pub async fn`s, writes over
//! `&mut SqliteConnection` and reads over `&SqlitePool`, errors wrapped via
//! `Error::Sqlx`, and a private `row_to_*` projector.

use concerto_error::{Error, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::api::MaestroState;

/// Fetch the `id = 1` singleton (read path). `None` ⇒ the row was never
/// initialized (the caller should [`ensure_initialized`] first).
pub async fn get(pool: &SqlitePool) -> Result<Option<MaestroState>> {
    let row = sqlx::query(
        "SELECT id, daily_in_today, daily_out_today, budget_resets_at,
                last_digest_at, enabled
         FROM maestro_state WHERE id = 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(row_to_maestro_state))
}

/// Idempotently create the singleton with defaults if absent (`INSERT OR
/// IGNORE` on `id = 1`). Never clobbers live counters — re-running on an
/// existing row is a no-op. `budget_resets_at` seeds the first reset instant;
/// `daily_in_today`/`daily_out_today` default to 0, `enabled` to 1, and
/// `last_digest_at` to NULL. Call once per boot (Task 414).
pub async fn ensure_initialized(conn: &mut SqliteConnection, budget_resets_at: i64) -> Result<()> {
    sqlx::query("INSERT OR IGNORE INTO maestro_state (id, budget_resets_at) VALUES (1, ?)")
        .bind(budget_resets_at)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Additive cumulative-across-backends counter bump (Task 412). Additive in
/// SQL (`SET x = x + ?`), not read-modify-write in Rust, so per-turn bumps
/// stay correct under the writer mutex without a select-then-update race.
pub async fn bump_daily_counters(
    conn: &mut SqliteConnection,
    in_delta: i64,
    out_delta: i64,
) -> Result<()> {
    sqlx::query(
        "UPDATE maestro_state
         SET daily_in_today = daily_in_today + ?,
             daily_out_today = daily_out_today + ?
         WHERE id = 1",
    )
    .bind(in_delta)
    .bind(out_delta)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Zero both daily counters and set the next reset instant (UTC-midnight or
/// manual, Task 412).
pub async fn reset_budget(conn: &mut SqliteConnection, budget_resets_at: i64) -> Result<()> {
    sqlx::query(
        "UPDATE maestro_state
         SET daily_in_today = 0,
             daily_out_today = 0,
             budget_resets_at = ?
         WHERE id = 1",
    )
    .bind(budget_resets_at)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Patch the digest-cadence cursor `last_digest_at` (Task 414).
pub async fn set_last_digest(conn: &mut SqliteConnection, last_digest_at: i64) -> Result<()> {
    sqlx::query("UPDATE maestro_state SET last_digest_at = ? WHERE id = 1")
        .bind(last_digest_at)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Enable/disable the Maestro (Task 414 `set_enabled` / the
/// `enterpriseDataPrivacy` gate). `bool` is mapped to the stored `0/1`.
pub async fn set_enabled(conn: &mut SqliteConnection, enabled: bool) -> Result<()> {
    sqlx::query("UPDATE maestro_state SET enabled = ? WHERE id = 1")
        .bind(enabled as i64)
        .execute(conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

/// Bootstrap the singleton `chats(kind='maestro', session_id NULL)` row if
/// absent. No schema change — the row validates against migration 0001's
/// `CHECK (kind IN ('session','maestro'))` + `CHECK ((session_id IS NOT NULL)
/// OR kind='maestro')`. Idempotent: only inserts when no `kind='maestro'`
/// row already exists, so re-running never creates a second maestro chat (the
/// caller-supplied `id` is honored only on first bootstrap). Task 410 attaches
/// daily-summary `chat_messages` to this chat.
pub async fn ensure_maestro_chat(
    conn: &mut SqliteConnection,
    id: &str,
    created_at: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO chats (id, session_id, kind, created_at)
         SELECT ?, NULL, 'maestro', ?
         WHERE NOT EXISTS (SELECT 1 FROM chats WHERE kind = 'maestro')",
    )
    .bind(id)
    .bind(created_at)
    .execute(conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(())
}

fn row_to_maestro_state(row: sqlx::sqlite::SqliteRow) -> MaestroState {
    MaestroState {
        id: row.get::<i64, _>("id"),
        daily_in_today: row.get::<i64, _>("daily_in_today"),
        daily_out_today: row.get::<i64, _>("daily_out_today"),
        budget_resets_at: row.get::<i64, _>("budget_resets_at"),
        last_digest_at: row.get::<Option<i64>, _>("last_digest_at"),
        enabled: row.get::<i64, _>("enabled") != 0,
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

    #[tokio::test]
    async fn ensure_initialized_creates_defaults_singleton_and_is_idempotent() {
        let (_dir, persist) = fresh_db().await;

        {
            let mut w = persist.writer().await;
            ensure_initialized(&mut w, 1_700_000_000_000)
                .await
                .expect("init");
        }

        let state = get(persist.readers())
            .await
            .expect("get")
            .expect("singleton present");
        assert_eq!(state.id, 1);
        assert_eq!(state.daily_in_today, 0);
        assert_eq!(state.daily_out_today, 0);
        assert_eq!(state.budget_resets_at, 1_700_000_000_000);
        assert_eq!(state.last_digest_at, None);
        assert!(state.enabled);

        // Mutate, then re-init: the second `ensure_initialized` must NOT
        // clobber the live counters / enabled flag.
        {
            let mut w = persist.writer().await;
            bump_daily_counters(&mut w, 50, 10).await.expect("bump");
            set_enabled(&mut w, false).await.expect("disable");
            // Different budget_resets_at to prove INSERT OR IGNORE no-ops.
            ensure_initialized(&mut w, 9_999_999_999_999)
                .await
                .expect("re-init");
        }

        let after = get(persist.readers())
            .await
            .expect("get")
            .expect("present")
            .clone();
        assert_eq!(after.daily_in_today, 50, "counter preserved across re-init");
        assert_eq!(after.daily_out_today, 10);
        assert_eq!(
            after.budget_resets_at, 1_700_000_000_000,
            "re-init must not overwrite budget_resets_at"
        );
        assert!(!after.enabled, "enabled preserved across re-init");
    }

    #[tokio::test]
    async fn bump_daily_counters_is_additive_and_cumulative() {
        let (_dir, persist) = fresh_db().await;
        let mut w = persist.writer().await;
        ensure_initialized(&mut w, 0).await.expect("init");
        bump_daily_counters(&mut w, 100, 20).await.expect("bump 1");
        bump_daily_counters(&mut w, 100, 20).await.expect("bump 2");
        drop(w);

        let state = get(persist.readers()).await.expect("get").expect("present");
        assert_eq!(state.daily_in_today, 200);
        assert_eq!(state.daily_out_today, 40);
    }

    #[tokio::test]
    async fn reset_budget_zeroes_counters_and_sets_instant() {
        let (_dir, persist) = fresh_db().await;
        let mut w = persist.writer().await;
        ensure_initialized(&mut w, 1).await.expect("init");
        bump_daily_counters(&mut w, 500, 250).await.expect("bump");
        reset_budget(&mut w, 4_242).await.expect("reset");
        drop(w);

        let state = get(persist.readers()).await.expect("get").expect("present");
        assert_eq!(state.daily_in_today, 0);
        assert_eq!(state.daily_out_today, 0);
        assert_eq!(state.budget_resets_at, 4_242);
    }

    #[tokio::test]
    async fn last_digest_and_enabled_round_trip() {
        let (_dir, persist) = fresh_db().await;
        let mut w = persist.writer().await;
        ensure_initialized(&mut w, 0).await.expect("init");
        set_last_digest(&mut w, 1_800_000_000_000)
            .await
            .expect("set_last_digest");
        set_enabled(&mut w, false).await.expect("set_enabled");
        drop(w);

        let state = get(persist.readers()).await.expect("get").expect("present");
        assert_eq!(state.last_digest_at, Some(1_800_000_000_000));
        assert!(!state.enabled);
    }

    #[tokio::test]
    async fn check_constraint_rejects_id_two() {
        let (_dir, persist) = fresh_db().await;
        let mut w = persist.writer().await;
        let result = sqlx::query("INSERT INTO maestro_state (id, budget_resets_at) VALUES (2, ?)")
            .bind(0_i64)
            .execute(&mut *w)
            .await;
        assert!(result.is_err(), "CHECK(id = 1) must reject id = 2");
    }

    #[tokio::test]
    async fn get_returns_none_before_init() {
        let (_dir, persist) = fresh_db().await;
        let state = get(persist.readers()).await.expect("get");
        assert!(state.is_none(), "uninitialized maestro_state ⇒ None");
    }

    #[tokio::test]
    async fn ensure_maestro_chat_is_a_singleton() {
        let (_dir, persist) = fresh_db().await;
        let mut w = persist.writer().await;

        ensure_maestro_chat(&mut w, "maestro-chat-a", 1_000)
            .await
            .expect("bootstrap 1");
        // A second call with a different id must NOT create a second row.
        ensure_maestro_chat(&mut w, "maestro-chat-b", 2_000)
            .await
            .expect("bootstrap 2");

        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chats WHERE kind = 'maestro'")
            .fetch_one(&mut *w)
            .await
            .expect("count maestro chats");
        assert_eq!(n, 1, "exactly one kind='maestro' chat row");

        // The row that exists is the first one, with a NULL session_id.
        let (id, session_null): (String, i64) = {
            let row = sqlx::query(
                "SELECT id, (session_id IS NULL) AS session_null
                 FROM chats WHERE kind = 'maestro'",
            )
            .fetch_one(&mut *w)
            .await
            .expect("fetch maestro chat");
            (
                row.get::<String, _>("id"),
                row.get::<i64, _>("session_null"),
            )
        };
        assert_eq!(id, "maestro-chat-a", "first bootstrap wins");
        assert_eq!(session_null, 1, "maestro chat has NULL session_id");
    }
}
