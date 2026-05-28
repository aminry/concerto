//! Public surface of `concerto-persist`.
//!
//! Per the convention locked in Task 04, this module is what
//! `scripts/regen-interfaces.sh` scrapes to produce
//! `docs/interfaces/rust-api.md`. Types live here directly (not as
//! `pub use` re-exports) so the interface generator captures them.
//!
//! Public contract locked by Task 08:
//!
//! - [`Persistence::open`] opens (and creates, if missing) a SQLite database,
//!   applies the embedded migrations under `crates/persist/migrations/`, and
//!   runs `PRAGMA quick_check`. The on-disk SQLite pragmas
//!   `journal_mode = WAL`, `synchronous = NORMAL`, `busy_timeout = 5000`,
//!   `foreign_keys = ON` are non-negotiable.
//! - [`Persistence::writer`] returns an exclusive guard that callers hold
//!   across a transaction's `await` points. Task 08 implements the writer as
//!   a `tokio::sync::Mutex<SqliteConnection>`; the dedicated writer task /
//!   mpsc queue from design/09 §6.1 lands in ~Task 20.
//! - [`Persistence::readers`] hands out a `SqlitePool` whose connections have
//!   `PRAGMA query_only = ON` set, so callers cannot accidentally write
//!   through it.
//! - [`Persistence::shutdown`] closes both pools cleanly; once consumed, the
//!   handle is gone.

use std::path::PathBuf;
use std::sync::Arc;

use concerto_error::{Error, Result};
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteConnection, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx::{ConnectOptions, Connection, Executor, Row, SqlitePool};
use tokio::sync::{Mutex, MutexGuard};

/// Configuration for [`Persistence::open`].
///
/// `db_path` MUST point at a regular file location; its parent directory is
/// created on demand. `max_readers` caps the read-only pool size; pick small
/// numbers (the design target is `min(num_cpus, 8)`).
#[derive(Debug, Clone)]
pub struct PersistenceConfig {
    pub db_path: PathBuf,
    pub max_readers: u32,
}

impl PersistenceConfig {
    /// Default config: `~/concerto/concerto.db`, eight readers.
    ///
    /// The substitution `home::home_dir()` + `concerto/concerto.db` (rather
    /// than `dirs::data_dir()`) tracks the workspace's permissive-only
    /// license posture — `dirs` pulls in MPL-2.0 `option-ext` and is banned
    /// at the workspace level. See `crates/core/src/logging.rs` for the
    /// matching pattern.
    pub fn default_for_user() -> Result<Self> {
        let home = home::home_dir()
            .ok_or_else(|| Error::Internal("home::home_dir() returned None".into()))?;
        Ok(Self {
            db_path: home.join("concerto").join("concerto.db"),
            max_readers: 8,
        })
    }
}

/// Owned SQLite handle. Cloning is intentionally not supported — only one
/// runtime actor owns the persistence layer; everything else borrows it.
pub struct Persistence {
    writer: Arc<Mutex<SqliteConnection>>,
    readers: SqlitePool,
}

/// Exclusive write access to the single SQLite writer connection.
///
/// The guard is held across `.await` points (that's why the inner lock is
/// `tokio::sync::Mutex`, not `std::sync::Mutex`). Drop the guard the moment
/// the transaction is committed or rolled back; long-held guards block
/// every other writer.
///
/// The newtype wraps `tokio::sync::MutexGuard` directly. Task 08's spec
/// originally called for `impl AsyncDeref<SqliteConnection>` — that trait
/// isn't stable in Rust, so this concrete guard substitutes (see Task 08
/// Handoff Notes for drift detail). When Task ~20 introduces the dedicated
/// writer task + mpsc queue, this type becomes the receipt the caller awaits
/// on; the call-site signature stays `let mut w = persist.writer().await`.
pub struct WriterGuard<'a> {
    inner: MutexGuard<'a, SqliteConnection>,
}

impl<'a> std::ops::Deref for WriterGuard<'a> {
    type Target = SqliteConnection;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'a> std::ops::DerefMut for WriterGuard<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl Persistence {
    /// Open (creating if necessary) the SQLite database at `config.db_path`.
    ///
    /// Steps, in order — failure of any step aborts the open:
    /// 1. Create the DB file's parent directory.
    /// 2. Open the writer connection with WAL + busy_timeout + foreign_keys.
    /// 3. Build the reader pool; each connection gets `PRAGMA query_only`.
    /// 4. Run pending migrations via `sqlx::migrate!`.
    /// 5. `PRAGMA quick_check;` — abort if the result is not `"ok"`.
    pub async fn open(config: PersistenceConfig) -> Result<Self> {
        if let Some(parent) = config.db_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let writer_opts = base_connect_options(&config.db_path);
        let mut writer_conn = writer_opts
            .clone()
            .connect()
            .await
            .map_err(|e| Error::Sqlx(Box::new(e)))?;

        // Migrations must run on the writer connection so the implicit
        // _sqlx_migrations table participates in the same journal as the
        // schema it documents.
        sqlx::migrate!("./migrations")
            .run(&mut writer_conn)
            .await
            .map_err(|e| Error::Internal(format!("migrate: {e}")))?;

        // Integrity check after migrations: design/09 §6.3.
        let quick_check: String = sqlx::query_scalar("PRAGMA quick_check")
            .fetch_one(&mut writer_conn)
            .await
            .map_err(|e| Error::Sqlx(Box::new(e)))?;
        if quick_check != "ok" {
            return Err(Error::Internal(format!(
                "PRAGMA quick_check returned {quick_check:?}, expected \"ok\""
            )));
        }

        let readers: SqlitePool = SqlitePoolOptions::new()
            .max_connections(config.max_readers)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    conn.execute("PRAGMA query_only = ON;").await?;
                    Ok(())
                })
            })
            .connect_with(writer_opts.read_only(true))
            .await
            .map_err(|e| Error::Sqlx(Box::new(e)))?;

        Ok(Self {
            writer: Arc::new(Mutex::new(writer_conn)),
            readers,
        })
    }

    /// Acquire the writer guard. Awaits until any prior writer drops theirs.
    pub async fn writer(&self) -> WriterGuard<'_> {
        WriterGuard {
            inner: self.writer.lock().await,
        }
    }

    /// Borrow the read-only pool. Acquire connections directly via
    /// `pool.acquire()`; every connection has `PRAGMA query_only = ON`, so
    /// write attempts surface as SQLite errors rather than silently
    /// committing.
    pub fn readers(&self) -> &SqlitePool {
        &self.readers
    }

    /// Drop both pools cleanly.
    ///
    /// Consumes `self`; once shutdown returns, no further reads or writes
    /// can be issued through this handle. Best-effort: an error closing the
    /// writer is surfaced, but the reader pool is always closed first to
    /// avoid leaking connections on the error path.
    pub async fn shutdown(self) -> Result<()> {
        // Close the reader pool first — it's the more numerous of the two
        // and `SqlitePool::close` is non-fallible.
        self.readers.close().await;

        // The writer connection is unique (Arc count must be 1 by the time
        // we reach shutdown). If somehow another reference is alive, we
        // can't close the connection — that's a logic error worth surfacing.
        let mutex = Arc::try_unwrap(self.writer).map_err(|_| {
            Error::Internal("Persistence::shutdown called while writer guard is still held".into())
        })?;
        let conn = mutex.into_inner();
        conn.close().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        Ok(())
    }

    /// Inspect the SQLite `PRAGMA journal_mode` of the writer. Used by
    /// integration tests; not part of the public contract beyond that.
    #[doc(hidden)]
    pub async fn journal_mode(&self) -> Result<String> {
        let mut guard = self.writer.lock().await;
        let row = sqlx::query("PRAGMA journal_mode")
            .fetch_one(&mut *guard)
            .await
            .map_err(|e| Error::Sqlx(Box::new(e)))?;
        Ok(row.get::<String, _>(0))
    }

    /// Inspect the SQLite `PRAGMA foreign_keys` of the writer.
    #[doc(hidden)]
    pub async fn foreign_keys(&self) -> Result<bool> {
        let mut guard = self.writer.lock().await;
        let row = sqlx::query("PRAGMA foreign_keys")
            .fetch_one(&mut *guard)
            .await
            .map_err(|e| Error::Sqlx(Box::new(e)))?;
        Ok(row.get::<i64, _>(0) != 0)
    }
}

// ---------------------------------------------------------------------------
// Repositories (Task 18).
//
// Surfaced through `api.rs` so the interface generator picks the types up.
// The CRUD impls live in `crates/persist/src/repositories.rs`.
// ---------------------------------------------------------------------------

/// Newtype around a `repositories.id` (UUIDv7 string per migration 0001).
///
/// Wraps `String` rather than `uuid::Uuid` to keep the schema's TEXT
/// primary key honest at the type system level — callers don't need to
/// parse-and-format on every boundary.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepositoryId(pub String);

impl RepositoryId {
    /// View as a borrowed string slice (`&str`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RepositoryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Insert-time shape for a `repositories` row. Task 18 ships only the
/// V0.1 columns; `clone_strategy` is always `"full"` in V0.1, and
/// `cone_defaults_json` defaults to `[]` at the SQL layer.
#[derive(Debug, Clone)]
pub struct NewRepository {
    pub id: RepositoryId,
    pub project_id: String,
    pub name: String,
    pub url: String,
    pub local_path: String,
    pub clone_strategy: String,
    pub default_branch: String,
}

/// Row-shaped projection of a `repositories` row. V0.1 omits
/// `cone_defaults_json` — it's written by a V1.0 sparse + cones task.
/// `fs_monitor_pid` is populated by Task 28 (fsmonitor supervisor); a
/// `None` (or `Some(0)`) value means no daemon is recorded for the repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub id: RepositoryId,
    pub project_id: String,
    pub name: String,
    pub url: String,
    pub local_path: String,
    pub clone_strategy: String,
    pub default_branch: String,
    pub last_fetch_at: Option<i64>,
    /// PID of the `git fsmonitor--daemon` process supervising this repo,
    /// or `None` when no daemon is recorded. Task 28 writes this via
    /// [`crate::repositories::update_fs_monitor_pid`].
    pub fs_monitor_pid: Option<i64>,
}

// ---------------------------------------------------------------------------
// Projects + Workspaces (Task 19).
//
// The `Projects` gRPC service does not exist in V0.1; the persistence
// helpers in `crates/persist/src/projects.rs` are the only surface.
// `Workspaces` ships its gRPC surface in Task 19; the schema is locked by
// migration 0001 (Task 09).
// ---------------------------------------------------------------------------

/// Newtype around a `projects.id` (UUIDv7 string per migration 0001).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectId(pub String);

impl ProjectId {
    /// View as a borrowed string slice (`&str`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Insert-time shape for a `projects` row.
#[derive(Debug, Clone)]
pub struct NewProject {
    pub id: ProjectId,
    pub name: String,
    pub icon: Option<String>,
    /// Unix epoch milliseconds. Supplied by the caller to keep this
    /// layer pure (no wall-clock reads).
    pub created_at: i64,
}

/// Row-shaped projection of a `projects` row. `settings_json` is
/// intentionally omitted — V0.1 has no callers that consume it; future
/// tasks (per-project settings) will add it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub icon: Option<String>,
    pub created_at: i64,
    pub archived_at: Option<i64>,
}

/// Newtype around a `workspaces.id` (UUIDv7 string per migration 0001).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspaceId(pub String);

impl WorkspaceId {
    /// View as a borrowed string slice (`&str`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorkspaceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Insert-time shape for a `workspaces` row. `slug` is derived by the
/// workspace manager (Task 19); the persistence layer takes whatever
/// the caller supplies and lets the UNIQUE(project_id, slug) constraint
/// surface a collision via `is_unique_violation`.
///
/// `permission_mode` is the **lowercase** SQL form (`"strict" |
/// "normal" | "auto" | "yolo"`) or `None` for "inherit from project"
/// per `design/03 §3.2`. The CHECK constraint enforces the allowed set.
#[derive(Debug, Clone)]
pub struct NewWorkspace {
    pub id: WorkspaceId,
    pub project_id: String,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub permission_mode: Option<String>,
    /// Unix epoch milliseconds.
    pub created_at: i64,
}

/// Row-shaped projection of a `workspaces` row. `bypass_destructive_guard`
/// and `settings_json` are intentionally omitted in V0.1 — V0.1 callers
/// don't read them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub project_id: String,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    /// Lowercase SQL form (`"strict" | "normal" | "auto" | "yolo"`) or
    /// `None` for "inherit from project".
    pub permission_mode: Option<String>,
    pub created_at: i64,
    pub archived_at: Option<i64>,
}

// ---------------------------------------------------------------------------
// Workareas (Task 20).
//
// The `Workareas` gRPC service ships in Task 20; the schema is locked by
// migration 0001 (Task 09). `permission_mode` is nullable for
// inherit-from-workspace per `design/03 §3.2`. `status` is a lowercase
// string from the CHECK set
// (`created|active|running|awaiting|paused|archived|crashed`).
// ---------------------------------------------------------------------------

/// Newtype around a `workareas.id` (UUIDv7 string per migration 0001).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkareaId(pub String);

impl WorkareaId {
    /// View as a borrowed string slice (`&str`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorkareaId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Insert-time shape for a `workareas` row.
///
/// `composer_name` is allocated by the Workspace Manager (Task 20) from
/// `crates/core/src/workspace_manager/composers.rs`. `branch_name`
/// follows the V0.1 convention `concerto/<composer>`; the branch-rename
/// hook lands in V1.0. `worktree_root` is the absolute path
/// `<data_dir>/workspaces/<workspace.slug>/<composer>/`.
#[derive(Debug, Clone)]
pub struct NewWorkarea {
    pub id: WorkareaId,
    pub workspace_id: String,
    pub composer_name: String,
    pub branch_name: String,
    pub worktree_root: String,
    /// One of `created|active|running|awaiting|paused|archived|crashed`.
    /// The Workspace Manager inserts with `"created"` and immediately
    /// transitions to `"active"` inside the same transaction once the
    /// on-disk worktree + `.context/` skeleton exist.
    pub status: String,
    /// Lowercase SQL form (`"strict" | "normal" | "auto" | "yolo"`) or
    /// `None` for "inherit from workspace".
    pub permission_mode: Option<String>,
    /// Unix epoch milliseconds.
    pub created_at: i64,
}

/// Insert-time shape for a `workarea_repos` junction row.
///
/// `worktree_path` is `<worktree_root>/<repo.name>` — i.e. each repo's
/// worktree sits one directory below the workarea root, alongside the
/// `.context/` skeleton.
#[derive(Debug, Clone)]
pub struct NewWorkareaRepo {
    pub workarea_id: WorkareaId,
    pub repository_id: RepositoryId,
    pub worktree_path: String,
    pub branch_override: Option<String>,
}

/// Row-shaped projection of a `workareas` row. V0.1 omits
/// `bypass_destructive_guard` — V0.1 callers don't read it.
///
/// `settings_json` is the raw JSON string from migration 0002. Task 30
/// stamps `{"files_to_copy_applied": true}` onto it after the
/// files-to-copy resolver finishes so reruns are idempotent; design/03
/// §3.14 / design/04 §3.12 reserve other keys (`exclude_from_maestro`,
/// `default_deliberation_mode`, …) for later tasks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workarea {
    pub id: WorkareaId,
    pub workspace_id: WorkspaceId,
    pub composer_name: String,
    pub branch_name: String,
    pub worktree_root: String,
    pub status: String,
    pub permission_mode: Option<String>,
    pub created_at: i64,
    pub archived_at: Option<i64>,
    pub last_activity_at: Option<i64>,
    pub settings_json: String,
}

// ---------------------------------------------------------------------------
// Sessions + Chats (Task 22).
//
// The `sessions` table schema is locked by migration 0001 (Task 09);
// the helpers live in `crates/persist/src/sessions.rs`. `chat_id` is a
// NOT NULL FK to `chats`, so every session creation inserts a `chats`
// row first inside the same transaction.
// ---------------------------------------------------------------------------

/// Newtype around a `sessions.id` (UUIDv7 string per migration 0001).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(pub String);

impl SessionId {
    /// View as a borrowed string slice (`&str`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Insert-time shape for a `chats` row.
///
/// `kind = "session"` for the per-session chat (with `session_id` set to
/// the new session's UUID); `kind = "maestro"` is the singleton chat
/// keyed off `session_id IS NULL`.
#[derive(Debug, Clone)]
pub struct NewChat {
    pub id: String,
    pub session_id: Option<String>,
    /// One of `session|maestro`. CHECK enforced at the SQL layer.
    pub kind: String,
    pub created_at: i64,
}

/// Insert-time shape for a `sessions` row.
///
/// `agent_kind` is one of `claude|codex|gemini|maestro` per the CHECK
/// constraint in migration 0001. Status starts as `"starting"` and is
/// transitioned to `"running"` after `Hello/Ready`, then `"finished"` on
/// clean stop or `"crashed"` on error.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub id: SessionId,
    pub workarea_id: WorkareaId,
    pub chat_id: String,
    pub agent_kind: String,
    pub agent_version: Option<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub host_pid: Option<i64>,
    pub host_socket: Option<String>,
    pub pty_cookie: Option<Vec<u8>>,
    pub external_session_id: Option<String>,
    /// One of `strict|normal|auto|yolo` — never NULL on the sessions row
    /// (`DEFAULT 'normal'`).
    pub permission_mode: String,
    pub bypass_destructive_guard: bool,
    pub started_at: i64,
    /// One of `starting|running|awaiting|finished|crashed`.
    pub status: String,
}

/// Row-shaped projection of a `sessions` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub workarea_id: WorkareaId,
    pub chat_id: String,
    pub agent_kind: String,
    pub agent_version: Option<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub host_pid: Option<i64>,
    pub host_socket: Option<String>,
    pub pty_cookie: Option<Vec<u8>>,
    pub external_session_id: Option<String>,
    pub permission_mode: String,
    pub bypass_destructive_guard: bool,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub last_heartbeat: Option<i64>,
    pub status: String,
}

/// Build the `SqliteConnectOptions` shared by writer + reader pools.
///
/// Every pragma the design doc lists as mandatory (`journal_mode = WAL`,
/// `synchronous = NORMAL`, `busy_timeout = 5000`, `foreign_keys = ON`) is set
/// here, so both pools inherit them.
fn base_connect_options(db_path: &std::path::Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_millis(5_000))
        .foreign_keys(true)
}
