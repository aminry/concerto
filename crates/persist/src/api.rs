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
    /// Steps, in order — failure of any step aborts the open (Task 110 adds
    /// the on-open integrity check + downgrade guard, both BEFORE the
    /// migrator touches the DB so a corrupt or future-version file fails
    /// loudly at boot rather than producing silent misbehaviour):
    /// 1. Create the DB file's parent directory.
    /// 2. Open the writer connection with WAL + busy_timeout + foreign_keys.
    /// 3. `PRAGMA quick_check;` on open — abort with [`Error::DatabaseCorrupt`]
    ///    if the result is not `"ok"` (design/09 §6.3, §8). Runs first so the
    ///    migrator never touches a corrupt file.
    /// 4. Downgrade guard (design/09 §8): if the DB's applied schema version is
    ///    newer than the highest migration this binary ships, abort with
    ///    [`Error::SchemaDowngrade`] naming both versions.
    /// 5. Run pending migrations via `sqlx::migrate!` (forward-only).
    /// 6. Build the reader pool; each connection gets `PRAGMA query_only`.
    /// 7. A second `PRAGMA quick_check;` after migrations (design/09 §6.3) —
    ///    the existing post-migration integrity check, retained.
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

        // Integrity check ON OPEN, before the migrator touches anything
        // (design/09 §6.3, §8). A corrupt file must fail here so the
        // forward-only migrator never runs against bad pages.
        let on_open_check: String = sqlx::query_scalar("PRAGMA quick_check")
            .fetch_one(&mut writer_conn)
            .await
            .map_err(|e| Error::Sqlx(Box::new(e)))?;
        if on_open_check != "ok" {
            return Err(Error::DatabaseCorrupt(format!(
                "database at {} appears corrupt: PRAGMA quick_check returned {on_open_check:?}, \
                 expected \"ok\". Restore from a backup (see `concerto backup`) or move the file \
                 aside and let Concerto recreate it.",
                config.db_path.display()
            )));
        }

        // Downgrade guard (design/09 §8): refuse to start when the DB's
        // applied schema version is newer than this binary understands. The
        // forward-only migrator can migrate UP but never DOWN, so a DB written
        // by a newer Core would otherwise be silently misinterpreted.
        let binary_max = binary_max_schema_version();
        if let Some(db_version) = applied_schema_version(&mut writer_conn).await? {
            if let Some(max) = binary_max {
                if db_version > max {
                    return Err(Error::SchemaDowngrade(format!(
                        "database at {} is at schema version {db_version}, but this binary only \
                         understands up to {max}. This Core is older than your data; install a \
                         newer Core to open it (downgrade is not supported).",
                        config.db_path.display()
                    )));
                }
            }
        }

        // Migrations run on the writer connection so the implicit
        // _sqlx_migrations table participates in the same journal as the
        // schema it documents. Forward-only (design/09 §6.2).
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
            return Err(Error::DatabaseCorrupt(format!(
                "database at {} failed PRAGMA quick_check after migrations: returned \
                 {quick_check:?}, expected \"ok\"",
                config.db_path.display()
            )));
        }

        // Single deterministic success signal for the smoke gate + operators
        // (Task 110): the integrity guards passed and the schema is current.
        tracing::info!(
            db_path = %config.db_path.display(),
            schema_version = binary_max.unwrap_or(0),
            "persistence integrity ok (quick_check passed, schema not downgraded)"
        );

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
    pub name: String,
    pub url: String,
    pub local_path: String,
    pub clone_strategy: String,
    pub default_branch: String,
}

/// Row-shaped projection of a `repositories` row.
///
/// `fs_monitor_pid` is populated by Task 28 (fsmonitor supervisor); a
/// `None` (or `Some(0)`) value means no daemon is recorded for the repo.
/// `cone_defaults_json` is the repository-level sparse-cone defaults layer
/// (Task 302) — a flat `["<cone_path>", …]` JSON array (migration 0001
/// default `'[]'`); the three-layer cone resolver reads it as the
/// least-specific layer (repo → workspace-default → workarea).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub id: RepositoryId,
    pub name: String,
    pub url: String,
    pub local_path: String,
    pub clone_strategy: String,
    pub default_branch: String,
    /// Repository-level sparse-cone defaults, a JSON `["<cone_path>", …]`
    /// array (Task 302, `design/02 §3.2`). Defaults to `"[]"`.
    pub cone_defaults_json: String,
    /// Per-repo action preferences, a JSON object keyed by the seven action
    /// names (Task 310, `design/04 §3.13`, migration 0011) — the local-DB
    /// layer of the settings precedence chain. Defaults to `"{}"`. Read by
    /// the `WorkspaceSettingsResolver` as the per-repo `action_prefs.<action>`
    /// layer (managed > checked-in `.concerto/action_prefs.toml` > this >
    /// default).
    pub action_prefs_json: String,
    pub last_fetch_at: Option<i64>,
    /// PID of the `git fsmonitor--daemon` process supervising this repo,
    /// or `None` when no daemon is recorded. Task 28 writes this via
    /// [`crate::repositories::update_fs_monitor_pid`].
    pub fs_monitor_pid: Option<i64>,
}

// ---------------------------------------------------------------------------
// Workspaces (Task 19).
//
// Workspaces are a top-level entity after the Project→Workspace collapse
// (D5): there is no parent project. The `Workspaces` gRPC surface ships in
// Task 19; the schema is locked by migration 0001 (Task 09).
// ---------------------------------------------------------------------------

/// One repository's attachment to a workspace: its id plus the
/// per-(workspace, repo) sparse-cone snapshot (`workspace_repos.sparse_cones_json`).
#[derive(Debug, Clone)]
pub struct WorkspaceRepoCones {
    pub repository_id: RepositoryId,
    /// Per-`(workspace, repo)` sparse-cone snapshot as a JSON
    /// `["<cone_path>", …]` array string (Task 302, D6).
    pub sparse_cones_json: String,
}

impl WorkspaceRepoCones {
    /// Attach a repo with an empty (`"[]"`) cone snapshot.
    pub fn empty_cones(repository_id: RepositoryId) -> Self {
        Self {
            repository_id,
            sparse_cones_json: "[]".to_string(),
        }
    }
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
/// the caller supplies and lets the UNIQUE(slug) constraint surface a
/// collision via `is_unique_violation`.
///
/// `permission_mode` is the **lowercase** SQL form (`"strict" |
/// "normal" | "auto" | "yolo"`) or `None` for "inherit from workspace
/// defaults" per `design/03 §3.2`. The CHECK constraint enforces the
/// allowed set.
#[derive(Debug, Clone)]
pub struct NewWorkspace {
    pub id: WorkspaceId,
    pub name: String,
    pub slug: String,
    pub icon: Option<String>,
    pub description: Option<String>,
    pub permission_mode: Option<String>, // None = inherit from workspace defaults
    pub created_at: i64,
}

/// Row-shaped projection of a `workspaces` row. `bypass_destructive_guard`
/// and `settings_json` are intentionally omitted in V0.1 — V0.1 callers
/// don't read them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub slug: String,
    pub icon: Option<String>,
    pub description: Option<String>,
    /// Lowercase SQL form (`"strict" | "normal" | "auto" | "yolo"`) or
    /// `None` for "inherit from workspace defaults".
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
///
/// `sparse_cones_json` is the per-(workarea, repo) sparse-cone set as a
/// JSON `["<cone_path>", …]` array (Task 302). The migration-0001 column
/// default is `'[]'`; callers that have a resolved initial cone (the
/// three-layer inheritance resolver) pass it here, others pass
/// [`NewWorkareaRepo::empty_cones`] / `"[]".to_string()`.
#[derive(Debug, Clone)]
pub struct NewWorkareaRepo {
    pub workarea_id: WorkareaId,
    pub repository_id: RepositoryId,
    pub worktree_path: String,
    pub branch_override: Option<String>,
    /// Initial per-(workarea, repo) cone set as a JSON array string
    /// (Task 302). Use [`NewWorkareaRepo::empty_cones`] for the `'[]'`
    /// default.
    pub sparse_cones_json: String,
}

impl NewWorkareaRepo {
    /// The empty-cone-set JSON literal (`"[]"`) — the migration default.
    pub fn empty_cones() -> String {
        "[]".to_string()
    }
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
    /// Task 36: highest `seq` the Core has ack'd to the agent host.
    /// Initial inserts always pass `0`; the bridge pump persists this
    /// opportunistically and `adopt_orphans` reads it on boot.
    pub last_acked_seq: i64,
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
    /// Task 36: highest `seq` the Core has ack'd to the agent host.
    /// Persisted opportunistically by the bridge pump (every ~5s); used
    /// by `adopt_orphans` on Core boot to resume the host's ring buffer
    /// past this watermark.
    pub last_acked_seq: i64,
}

// ---------------------------------------------------------------------------
// Schedules (Task 38).
//
// V0.1 surface: session-scoped `/loop` only. Persistent scheduled tasks
// (`kind = 'scheduled_task'`), cloud-task sync, promote, and budget
// guardrails are V1.0. The schema is locked by migration 0004; only the
// V0.1 columns are projected here.
// ---------------------------------------------------------------------------

/// Newtype around a `schedules.id` (UUIDv7 string per migration 0004).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScheduleId(pub String);

impl ScheduleId {
    /// View as a borrowed string slice (`&str`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ScheduleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Newtype around a `schedule_runs.id` (UUIDv7 string per migration 0004).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScheduleRunId(pub String);

impl ScheduleRunId {
    /// View as a borrowed string slice (`&str`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ScheduleRunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Insert-time shape for a `schedules` row.
///
/// `kind` is `"loop"` in V0.1 — the CHECK constraint will reject any
/// other value. `interval_seconds` MUST be in 30..=604800; the
/// [`crate::schedules::insert`] helper does not re-validate (the
/// [`crate::api::Persistence`] layer is dumb storage), so the caller
/// (the Scheduler) is responsible for bounds-checking. `expires_at`
/// defaults to `created_at + 3 days` in the Scheduler.
#[derive(Debug, Clone)]
pub struct NewSchedule {
    pub id: ScheduleId,
    pub workarea_id: WorkareaId,
    /// V0.1: always `"loop"`.
    pub kind: String,
    pub interval_seconds: i64,
    pub expires_at: i64,
    pub last_run_at: Option<i64>,
    pub paused: bool,
    pub prompt: String,
    /// One of `claude|codex|gemini|maestro` (CHECK enforced).
    pub agent_kind: String,
    pub created_at: i64,
}

/// Row-shaped projection of a `schedules` row. V0.1 projects only the
/// columns migration 0004 defines; the design/09 §4.3 columns deferred
/// to V1.0 are not modelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    pub id: ScheduleId,
    pub workarea_id: WorkareaId,
    pub kind: String,
    pub interval_seconds: i64,
    pub expires_at: i64,
    pub last_run_at: Option<i64>,
    pub paused: bool,
    pub prompt: String,
    pub agent_kind: String,
    pub created_at: i64,
}

/// Insert-time shape for a `schedule_runs` row.
///
/// `session_id` is `None` at insert time when the run is inserted as
/// part of the inflight-suppression check before the supervisor has
/// returned a session id; the Scheduler patches it via
/// [`crate::schedule_runs::update_session`] once `start_session`
/// resolves. `ended_at` and `terminal_state` are always `NULL` at
/// insert; they're set together by
/// [`crate::schedule_runs::update_terminal`].
#[derive(Debug, Clone)]
pub struct NewScheduleRun {
    pub id: ScheduleRunId,
    pub schedule_id: ScheduleId,
    pub session_id: Option<SessionId>,
    pub started_at: i64,
}

/// Row-shaped projection of a `schedule_runs` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleRun {
    pub id: ScheduleRunId,
    pub schedule_id: ScheduleId,
    pub session_id: Option<SessionId>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    /// `None` while inflight; one of `completed|failed|crashed` once
    /// the lifecycle watcher resolves the run.
    pub terminal_state: Option<String>,
}

// ---------------------------------------------------------------------------
// Maestro state (Task 403).
//
// The persistence root of the Maestro budget + lifecycle state. The schema is
// locked by migration 0015 (`design/08 §4.1`) as a `CHECK (id = 1)` singleton:
// exactly one row ever exists. This is the FIRST daily-counter/budget table —
// `schedules` (0004) deferred its token columns, so there is no precedent.
//
// FROZEN per `tasks/v1.0/PHASE4_PLANNING.md §4.6` (D6). Task 412 consumes the
// budget counters via [`crate::maestro_state::bump_daily_counters`] /
// [`crate::maestro_state::reset_budget`] / [`crate::maestro_state::get`];
// Task 414 reads `enabled`/`last_digest_at`; Task 410 attaches daily summaries
// to the `chats(kind='maestro')` row bootstrapped by
// [`crate::maestro_state::ensure_maestro_chat`].
// ---------------------------------------------------------------------------

/// Row-shaped projection of the singleton `maestro_state` row (migration
/// 0015 / `design/08 §4.1`). Always `id = 1`. `enabled` maps the stored
/// `INTEGER` 0/1 to a `bool`; `last_digest_at` is `None` until the first
/// digest. All timestamps are unix-ms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaestroState {
    pub id: i64,
    pub daily_in_today: i64,
    pub daily_out_today: i64,
    pub budget_resets_at: i64,
    pub last_digest_at: Option<i64>,
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// Skills (Task 39).
//
// V0.1 surface: discovery (personal + workspace scopes) + per-(scope,
// workspace, name) enable/disable. Marketplace install, sandbox try, and
// invocation tracking are V1.0 per `tasks/39 §"Scope — out"`. The schema
// is locked by migration 0005; only the V0.1 columns are projected here.
// ---------------------------------------------------------------------------

/// Newtype around a `skills_index.id` (UUIDv7 string per migration 0005).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SkillId(pub String);

impl SkillId {
    /// View as a borrowed string slice (`&str`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SkillId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Scope axis for a `skills_index` row. Mirrors the four-scope contract
/// in `design/06 §1`. V0.1 actively discovers `Personal` and `Workspace`;
/// `Plugin` / `Enterprise` exist on the row but are not walked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SkillScope {
    Personal,
    Workspace,
    Plugin,
    Enterprise,
}

impl SkillScope {
    /// Lowercase SQL form (matches the migration 0005 CHECK set).
    pub fn as_sql_str(self) -> &'static str {
        match self {
            SkillScope::Personal => "personal",
            SkillScope::Workspace => "workspace",
            SkillScope::Plugin => "plugin",
            SkillScope::Enterprise => "enterprise",
        }
    }

    /// Inverse of [`Self::as_sql_str`]. Returns `None` for unknown
    /// values (the SQL CHECK constraint should normally make this
    /// unreachable from the DB side).
    pub fn from_sql_str(s: &str) -> Option<Self> {
        match s {
            "personal" => Some(SkillScope::Personal),
            "workspace" => Some(SkillScope::Workspace),
            "plugin" => Some(SkillScope::Plugin),
            "enterprise" => Some(SkillScope::Enterprise),
            _ => None,
        }
    }
}

/// Insert/upsert-time shape for a `skills_index` row. `tools_json` is
/// the already-encoded JSON array (the caller serialises so this layer
/// stays dumb).
#[derive(Debug, Clone)]
pub struct NewSkill {
    pub id: SkillId,
    pub scope: SkillScope,
    /// MUST be `Some` when `scope == SkillScope::Workspace`; MUST be
    /// `None` otherwise.
    pub workspace_id: Option<WorkspaceId>,
    pub name: String,
    pub slash_command: Option<String>,
    pub description: Option<String>,
    /// JSON-encoded list of tool names (e.g. `'["Read","Edit"]'`).
    pub tools_json: String,
    pub source_path: String,
    pub discovered_at: i64,
}

/// Row-shaped projection of a `skills_index` row. `enabled` defaults to
/// `true` on insert; the toggle path flips it without touching anything
/// else so re-discovery preserves the user's choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRow {
    pub id: SkillId,
    pub scope: SkillScope,
    pub workspace_id: Option<WorkspaceId>,
    pub name: String,
    pub slash_command: Option<String>,
    pub description: Option<String>,
    pub tools_json: String,
    pub source_path: String,
    pub enabled: bool,
    pub discovered_at: i64,
}

/// Filter for [`crate::skills::list`]. All fields are optional; absent
/// means "no filter on that axis".
#[derive(Debug, Clone, Default)]
pub struct SkillFilter {
    pub scope: Option<SkillScope>,
    pub workspace_id: Option<WorkspaceId>,
    pub enabled_only: bool,
}

// ---------------------------------------------------------------------------
// Suggestion learn (Task 40).
//
// V0.1 ships the `suggestion_learn` table with insert + list-by-workarea
// helpers, but the rule engine does NOT write to it (per `design/07 §2`'s
// "rule engine only" row and `tasks/40 §"Scope — out"`). The table is
// created here so V1.0's learning loop can land behind the existing
// `Suggestions.RecordSuggestionOutcome` RPC stub without a wire-format
// break. The schema is locked by migration 0006.
// ---------------------------------------------------------------------------

/// Newtype around a `suggestion_learn.id` (UUIDv7 string per migration 0006).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SuggestionLearnId(pub String);

impl SuggestionLearnId {
    /// View as a borrowed string slice (`&str`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SuggestionLearnId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Insert-time shape for a `suggestion_learn` row. `workarea_id` is
/// `None` when the chip was Maestro-scoped (no workarea context).
/// `context_hash` is `''` in V0.1 — the field exists for V1.0's bucketed
/// weighting (`design/07 §6.2`); V0.1's `RecordSuggestionOutcome` stub
/// does not populate it.
#[derive(Debug, Clone)]
pub struct NewSuggestionLearn {
    pub id: SuggestionLearnId,
    pub workarea_id: Option<WorkareaId>,
    pub rule_id: String,
    /// Short free-form string (`accept | dismiss | snooze`). V0.1 does
    /// not CHECK the set so V1.0 experiments can add values.
    pub outcome: String,
    pub context_hash: String,
    /// Unix epoch milliseconds (caller-supplied).
    pub created_at: i64,
}

/// Row-shaped projection of a `suggestion_learn` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionLearn {
    pub id: SuggestionLearnId,
    pub workarea_id: Option<WorkareaId>,
    pub rule_id: String,
    pub outcome: String,
    pub context_hash: String,
    pub created_at: i64,
}

// ---------------------------------------------------------------------------
// Pull requests (Task 45).
//
// VCS Provider Integration cache for per-(workarea, repository) PR
// state. Schema is locked by migration 0008; helpers live in
// `crates/persist/src/pull_requests.rs`. Canonical state lives on
// GitHub — this table is a low-latency cache the UI reads from.
// ---------------------------------------------------------------------------

/// Newtype around a `pull_requests.id` (UUIDv7 string per migration 0008).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PullRequestId(pub String);

impl PullRequestId {
    /// View as a borrowed string slice (`&str`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PullRequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Upsert-time shape for a `pull_requests` row.
///
/// `provider` is `"github"` for the V0.1 backend; the column accepts
/// `"gitlab"` / `"bitbucket"` for V2.0 adapters without a schema
/// change. Timestamps are caller-supplied unix epoch milliseconds; the
/// persistence layer is dumb storage and does not read the wall clock.
#[derive(Debug, Clone)]
pub struct NewPullRequest {
    pub id: PullRequestId,
    pub workarea_id: WorkareaId,
    pub repository_id: RepositoryId,
    /// V0.1: always `"github"`.
    pub provider: String,
    pub pr_number: i64,
    pub base_ref: String,
    pub head_ref: String,
    /// One of `open|closed|merged|draft` for the GitHub provider.
    pub state: String,
    pub title: String,
    pub body: String,
    pub url: String,
    pub head_sha: String,
    /// Position of this PR within its workarea's merge plan (Task 319,
    /// migration 0014). Default = insertion order (`max(merge_order)+1`
    /// per workarea); the caller computes it (see
    /// [`crate::pull_requests::next_merge_order`]) and it is PRESERVED
    /// across upserts so a re-sync never clobbers a user's reorder.
    pub merge_order: i64,
    /// The PR's GraphQL node id (octocrab needs it for review-thread /
    /// resolve mutations, Task 316). `''` for rows created before Task 313
    /// wired octocrab. Refreshed on upsert.
    pub external_id: String,
    /// The `owner/repo` string the GraphQL endpoint keys on (Task 316).
    /// `''` for pre-octocrab rows. Refreshed on upsert.
    pub repository_full_name: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Row-shaped projection of a `pull_requests` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub id: PullRequestId,
    pub workarea_id: WorkareaId,
    pub repository_id: RepositoryId,
    pub provider: String,
    pub pr_number: i64,
    pub base_ref: String,
    pub head_ref: String,
    pub state: String,
    pub title: String,
    pub body: String,
    pub url: String,
    pub head_sha: String,
    /// Merge-plan position (migration 0014, Task 319). See
    /// [`NewPullRequest::merge_order`].
    pub merge_order: i64,
    /// GraphQL node id (Task 316); `''` for pre-octocrab rows.
    pub external_id: String,
    /// `owner/repo` for the GraphQL endpoint (Task 316); `''` for
    /// pre-octocrab rows.
    pub repository_full_name: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Identifier for a `vcs_credentials` row (Task 313). UUIDv7, caller-generated.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VcsCredentialId(pub String);

impl VcsCredentialId {
    /// View as a borrowed string slice (`&str`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for VcsCredentialId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Upsert-time shape for a `vcs_credentials` row (Task 313, migration 0012).
///
/// **Non-secret metadata only** — there is deliberately no key/token field. The
/// secret material lives in the OS keychain via `VcsSecretSlot`; this row holds
/// the *references* (which app/installation/account, when the token expires) so
/// the Core can decide whether to refresh (`design/13 §4`, locked decision D4).
/// Timestamps are caller-supplied unix epoch milliseconds.
#[derive(Debug, Clone)]
pub struct NewVcsCredential {
    pub id: VcsCredentialId,
    /// `'github'` | `'linear'` | `'jira'`.
    pub provider: String,
    /// App id (App auth) / repo id (webhook) / provider account id (Linear/Jira).
    pub scope_id: String,
    /// Human-facing login / org (display only).
    pub external_account: Option<String>,
    /// GitHub App id (App auth only).
    pub app_id: Option<String>,
    /// GitHub App installation id (App auth only).
    pub installation_id: Option<String>,
    /// Token expiry, epoch ms (nullable — PATs / personal keys do not expire).
    pub token_expires_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Row-shaped projection of a `vcs_credentials` row (Task 313).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VcsCredential {
    pub id: VcsCredentialId,
    pub provider: String,
    pub scope_id: String,
    pub external_account: Option<String>,
    pub app_id: Option<String>,
    pub installation_id: Option<String>,
    pub token_expires_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// The highest migration version this binary ships, derived from the
/// embedded `sqlx::migrate!` migrator rather than a hardcoded literal that
/// would drift from `crates/persist/migrations/`. Returns `None` only if the
/// binary ships zero migrations (which never happens in practice — there is
/// always at least `0001_initial_schema.sql`).
fn binary_max_schema_version() -> Option<i64> {
    sqlx::migrate!("./migrations")
        .iter()
        .map(|m| m.version)
        .max()
}

/// Read the DB's currently-applied schema version: the maximum `version` in
/// sqlx's internal `_sqlx_migrations` table. Returns `None` for a fresh DB
/// where the migrator has not yet created the table (no migrations applied).
async fn applied_schema_version(conn: &mut SqliteConnection) -> Result<Option<i64>> {
    // `_sqlx_migrations` does not exist until the migrator runs at least
    // once. On a fresh DB the table is absent, so probe `sqlite_master`
    // first and treat "no table" as "no applied version" rather than an
    // error.
    let table_exists: bool = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?
        != 0;

    if !table_exists {
        return Ok(None);
    }

    let version: Option<i64> = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(version)
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
