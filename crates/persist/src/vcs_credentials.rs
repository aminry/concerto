//! `vcs_credentials` table CRUD (Task 313, migration 0012).
//!
//! Stores the **non-secret** VCS credential metadata (`design/13 §4`,
//! `tasks/v1.0/PHASE3_PLANNING.md §3`/`§4.1`): which provider/scope a credential
//! belongs to, the GitHub App / installation references, the human-facing
//! account, and the token-expiry hint the Core uses to decide whether to
//! refresh. The secret material itself (App private keys, webhook secrets,
//! Linear/Jira OAuth tokens) lives ONLY in the OS keychain via the parameterized
//! `VcsSecretSlot` accessor — locked decision D4. There is deliberately no key
//! or token column here.
//!
//! Schema is locked by migration 0012:
//!
//! ```sql
//! CREATE TABLE vcs_credentials (
//!     id TEXT PRIMARY KEY,
//!     provider TEXT NOT NULL,            -- github | linear | jira
//!     scope_id TEXT NOT NULL,            -- app_id | repo_id | provider account id
//!     external_account TEXT,
//!     app_id TEXT,
//!     installation_id TEXT,
//!     token_expires_at INTEGER,          -- epoch ms, nullable
//!     created_at INTEGER NOT NULL,
//!     updated_at INTEGER NOT NULL,
//!     UNIQUE(provider, scope_id)
//! );
//! ```
//!
//! Public types are declared in [`crate::api`] so the interface generator
//! picks them up.

use concerto_error::{Error, Result};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::api::{NewVcsCredential, VcsCredential, VcsCredentialId};

/// Insert OR update the row keyed by `(provider, scope_id)`.
///
/// `id` is caller-generated (UUIDv7) and used only on the first insert —
/// subsequent upserts keep the existing primary key (and `created_at`) so
/// callers holding the original id stay valid. `updated_at` always advances to
/// the caller-supplied value. Returns the canonical id (the existing one on an
/// UPDATE, the new one otherwise).
pub async fn upsert(conn: &mut SqliteConnection, row: NewVcsCredential) -> Result<VcsCredentialId> {
    sqlx::query(
        "INSERT INTO vcs_credentials (
            id, provider, scope_id, external_account, app_id,
            installation_id, token_expires_at, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(provider, scope_id) DO UPDATE SET
            external_account = excluded.external_account,
            app_id           = excluded.app_id,
            installation_id  = excluded.installation_id,
            token_expires_at = excluded.token_expires_at,
            updated_at       = excluded.updated_at",
    )
    .bind(&row.id.0)
    .bind(&row.provider)
    .bind(&row.scope_id)
    .bind(&row.external_account)
    .bind(&row.app_id)
    .bind(&row.installation_id)
    .bind(row.token_expires_at)
    .bind(row.created_at)
    .bind(row.updated_at)
    .execute(&mut *conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;

    let resolved: String =
        sqlx::query_scalar("SELECT id FROM vcs_credentials WHERE provider = ? AND scope_id = ?")
            .bind(&row.provider)
            .bind(&row.scope_id)
            .fetch_one(conn)
            .await
            .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(VcsCredentialId(resolved))
}

/// Fetch one credential by primary key (read-only).
pub async fn get(pool: &SqlitePool, id: &VcsCredentialId) -> Result<Option<VcsCredential>> {
    let row = sqlx::query(
        "SELECT id, provider, scope_id, external_account, app_id,
                installation_id, token_expires_at, created_at, updated_at
         FROM vcs_credentials WHERE id = ?",
    )
    .bind(&id.0)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(row_to_credential))
}

/// Fetch one credential by its `(provider, scope_id)` natural key (read-only).
pub async fn get_by_scope(
    pool: &SqlitePool,
    provider: &str,
    scope_id: &str,
) -> Result<Option<VcsCredential>> {
    let row = sqlx::query(
        "SELECT id, provider, scope_id, external_account, app_id,
                installation_id, token_expires_at, created_at, updated_at
         FROM vcs_credentials WHERE provider = ? AND scope_id = ?",
    )
    .bind(provider)
    .bind(scope_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(row.map(row_to_credential))
}

/// List every credential for `provider` (read-only), ordered by `scope_id` for
/// deterministic UI rendering.
pub async fn list_by_provider(pool: &SqlitePool, provider: &str) -> Result<Vec<VcsCredential>> {
    let rows = sqlx::query(
        "SELECT id, provider, scope_id, external_account, app_id,
                installation_id, token_expires_at, created_at, updated_at
         FROM vcs_credentials WHERE provider = ? ORDER BY scope_id",
    )
    .bind(provider)
    .fetch_all(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows.into_iter().map(row_to_credential).collect())
}

fn row_to_credential(row: sqlx::sqlite::SqliteRow) -> VcsCredential {
    VcsCredential {
        id: VcsCredentialId(row.get::<String, _>("id")),
        provider: row.get::<String, _>("provider"),
        scope_id: row.get::<String, _>("scope_id"),
        external_account: row.get::<Option<String>, _>("external_account"),
        app_id: row.get::<Option<String>, _>("app_id"),
        installation_id: row.get::<Option<String>, _>("installation_id"),
        token_expires_at: row.get::<Option<i64>, _>("token_expires_at"),
        created_at: row.get::<i64, _>("created_at"),
        updated_at: row.get::<i64, _>("updated_at"),
    }
}
