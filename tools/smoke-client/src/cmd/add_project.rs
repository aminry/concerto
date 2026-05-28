//! `smoke-client add-project --name <s>` — direct sqlx insert.
//!
//! V0.1 ships no `Projects.CreateProject` RPC (see Task 24 Handoff
//! Notes — only `Projects.ListProjects` exists), so the smoke client
//! writes a `projects` row through the canonical sqlx surface. The
//! DB path resolution matches `crates/core/src/runtime.rs::db_path`:
//!
//!   1. `$CONCERTO_DB_PATH` if set + non-empty.
//!   2. else `$CONCERTO_DATA_DIR/concerto.db` if `CONCERTO_DATA_DIR`
//!      is set + non-empty.
//!   3. else `~/concerto/concerto.db`.
//!
//! Per Task 27's deliberate-debt note, this is the documented V0.1
//! workaround; the Phase 3 `Projects.CreateProject` RPC obsoletes it.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection};
use uuid::Uuid;

use super::RPC_TIMEOUT;

pub async fn run(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("add-project: --name must be non-empty".to_string());
    }

    let db_path = resolve_db_path()?;
    let id = Uuid::now_v7().to_string();
    let created_at = current_millis()?;

    // Open the writer connection directly (no pool — single insert).
    // `create_if_missing(false)`: the smoke gate already booted Core
    // which ran the migrations; if the DB is missing the project
    // insert would race the migration runner — fail loudly instead.
    let opts = SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(false);

    let connect_fut = async {
        let mut conn = opts
            .connect()
            .await
            .map_err(|e| format!("open {}: {e}", db_path.display()))?;

        // Schema columns are locked by migration 0001 (`crates/persist/migrations/`);
        // settings_json has a default of `'{}'` so we omit it.
        sqlx::query("INSERT INTO projects (id, name, created_at) VALUES (?, ?, ?)")
            .bind(&id)
            .bind(name)
            .bind(created_at)
            .execute(&mut conn)
            .await
            .map_err(|e| format!("insert: {e}"))?;

        conn.close().await.map_err(|e| format!("close: {e}"))?;

        Ok::<(), String>(())
    };

    tokio::time::timeout(RPC_TIMEOUT, connect_fut)
        .await
        .map_err(|_| format!("add-project timed out after {RPC_TIMEOUT:?}"))??;

    println!("{id}");
    Ok(())
}

/// Mirror `crates/core/src/runtime.rs::RuntimeConfig::db_path`'s
/// precedence so the smoke client and Core agree on the DB location.
fn resolve_db_path() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("CONCERTO_DB_PATH") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    if let Ok(d) = std::env::var("CONCERTO_DATA_DIR") {
        if !d.is_empty() {
            return Ok(PathBuf::from(d).join("concerto.db"));
        }
    }
    let home = home::home_dir().ok_or_else(|| "home::home_dir() returned None".to_string())?;
    Ok(home.join("concerto").join("concerto.db"))
}

fn current_millis() -> Result<i64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .map_err(|e| format!("clock before UNIX_EPOCH: {e}"))
}
