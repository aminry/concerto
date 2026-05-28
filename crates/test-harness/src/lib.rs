//! Shared integration-test harness for Concerto.
//!
//! Every later Phase 2/3 task that adds integration tests should use
//! [`CoreUnderTest::spawn`] instead of reinventing the spawn-and-connect
//! dance. See `README.md` for usage and constraints.
//!
//! ## Surface
//!
//! - [`CoreUnderTest::spawn`] — boots a fresh `concerto-core` binary in a
//!   tempdir, with `CONCERTO_CONFIG_DIR` / `CONCERTO_DATA_DIR` overridden,
//!   and waits for the UDS socket to appear (15 s deadline).
//! - [`CoreUnderTest::runtime_client`] — returns a Tonic `RuntimeClient`
//!   connected to the UDS. Build one per logical call site; the underlying
//!   channel is a fresh dial each time.
//! - [`CoreUnderTest::db`] — returns a read-only `SqlitePool` pointed at
//!   the Core's database. WAL mode makes this safe to use while the Core
//!   is running.
//! - [`CoreUnderTest::shutdown`] — SIGTERM → wait → SIGKILL fallback.
//! - `impl Drop` — last-resort SIGKILL if the caller forgets to call
//!   `shutdown().await`.
//!
//! ## Workspaces / Workareas / Sessions accessors
//!
//! Task 07 added the **messages** for these subsystems (`Workspace`,
//! `Workarea`, `Session`) but the **services** that expose them over gRPC
//! land in Phase 2 (Tasks 19, 20, 23). The task spec sketched
//! `workspaces_client()` / `workareas_client()` / `sessions_client()`
//! accessors as a forward-looking signature; the Phase 1 harness ships
//! `runtime_client()` only. Phase 2 tasks add the rest as the services
//! they front come online — fewer surface lies that way.

pub mod clients;
pub mod process;

use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::TempDir;

pub use crate::clients::{ClientError, RuntimeClient};
pub use crate::process::ProcessError;

/// Errors surfaced by the harness's public API.
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    /// Anything in the spawn / wait / shutdown lifecycle.
    #[error("process: {0}")]
    Process(#[from] ProcessError),
    /// Anything in the gRPC client construction path.
    #[error("client: {0}")]
    Client(#[from] ClientError),
    /// Anything talking to SQLite from `db()`.
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
    /// I/O escape hatch — file creation, tempdir, etc.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Result alias used by every public method on [`CoreUnderTest`].
pub type Result<T> = std::result::Result<T, HarnessError>;

/// Default budget for the socket-appearance wait in [`CoreUnderTest::spawn`].
///
/// 15 s matches `scripts/smoke.sh`'s budget so the harness and the smoke
/// gate agree on what "the Core didn't come up" means.
pub const SPAWN_SOCKET_TIMEOUT: Duration = Duration::from_secs(15);

/// Budget for the graceful-shutdown SIGTERM path in
/// [`CoreUnderTest::shutdown`]. After this elapses we escalate to SIGKILL.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// Threshold above which `shutdown` logs a `tracing::warn!`. Picked to
/// match the task spec's implementation note.
pub const SLOW_SHUTDOWN_WARN: Duration = Duration::from_secs(5);

/// A live `concerto-core` subprocess plus the paths it was spawned with.
///
/// Construct via [`CoreUnderTest::spawn`]. Drop or call
/// [`CoreUnderTest::shutdown`] to tear it down.
///
/// The tempdir backing `config_dir` / `data_dir` is owned by the harness
/// and removed when this struct is dropped. Callers MUST NOT reach into
/// `process::Handle` directly — every supported operation lives behind a
/// `&self` method on this type.
pub struct CoreUnderTest {
    /// `~/.concerto/`-shaped directory under the tempdir. Holds
    /// `core.sock` and `core.pid`.
    pub config_dir: PathBuf,
    /// `~/concerto/`-shaped directory under the tempdir. Holds
    /// `concerto.db` and `logs/`.
    pub data_dir: PathBuf,
    /// `<config_dir>/core.sock`. Where the Tonic `RuntimeClient` dials.
    pub socket_path: PathBuf,
    /// `<data_dir>/concerto.db`. What [`CoreUnderTest::db`] opens.
    pub db_path: PathBuf,
    /// Backing tempdir. Kept alive for the lifetime of the harness so
    /// the directory is not cleaned up while the subprocess is using it.
    _tempdir: TempDir,
    /// Wrapped subprocess handle. `Option` so `shutdown` can take it.
    process: Option<process::Handle>,
}

impl CoreUnderTest {
    /// Boot a fresh `concerto-core` in a tempdir.
    ///
    /// Steps:
    /// 1. Create a tempdir; lay down `config/` and `data/` subdirs.
    /// 2. Launch `cargo_bin!("concerto-core")` with `CONCERTO_CONFIG_DIR`
    ///    + `CONCERTO_DATA_DIR` overridden.
    /// 3. Poll for `<config>/core.sock` to appear (15 s deadline).
    ///
    /// Fails if the binary can't be located (build the workspace first),
    /// if the subprocess exits before the socket appears, or if 15 s
    /// elapses with no socket.
    pub async fn spawn() -> Result<Self> {
        let tempdir = TempDir::new()?;
        let config_dir = tempdir.path().join("config");
        let data_dir = tempdir.path().join("data");
        tokio::fs::create_dir_all(&config_dir).await?;
        tokio::fs::create_dir_all(&data_dir).await?;

        let socket_path = config_dir.join("core.sock");
        let db_path = data_dir.join("concerto.db");

        let process =
            process::Handle::spawn(&config_dir, &data_dir, &socket_path, SPAWN_SOCKET_TIMEOUT)
                .await?;

        Ok(Self {
            config_dir,
            data_dir,
            socket_path,
            db_path,
            _tempdir: tempdir,
            process: Some(process),
        })
    }

    /// Connect a Tonic [`RuntimeClient`] to the running Core.
    ///
    /// Each call dials a fresh channel — Tonic re-uses the connection
    /// across method calls on the same client, but separate `runtime_client()`
    /// calls return independent channels. This matches the task spec's
    /// "fresh connection per call" semantics.
    pub async fn runtime_client(&self) -> Result<RuntimeClient> {
        Ok(clients::runtime_client(self.socket_path.clone()).await?)
    }

    /// Open a read-only [`sqlx::SqlitePool`] to the Core's database.
    ///
    /// WAL mode (set by `concerto-persist`) makes concurrent readers
    /// safe while the Core's writer connection is live. The pool's
    /// connections do NOT set `PRAGMA query_only` — callers asserting
    /// on schema content should not be writing, but the pool will not
    /// stop them. If you accidentally write, you'll race the Core's
    /// writer and the test will fail loudly.
    pub async fn db(&self) -> Result<sqlx::SqlitePool> {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        let opts = SqliteConnectOptions::new()
            .filename(&self.db_path)
            // Don't create the DB; if the Core didn't make it, surface
            // that as an error rather than masking it.
            .create_if_missing(false)
            .read_only(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(2)
            .connect_with(opts)
            .await?;
        Ok(pool)
    }

    /// Graceful shutdown.
    ///
    /// Sends SIGTERM, waits up to [`SHUTDOWN_TIMEOUT`] for the Core to
    /// exit, then escalates to SIGKILL. Logs a `tracing::warn!` if the
    /// elapsed time exceeds [`SLOW_SHUTDOWN_WARN`].
    ///
    /// Consumes `self`; further calls are impossible. The tempdir is
    /// removed as part of the drop that follows.
    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(handle) = self.process.take() {
            handle
                .shutdown(SHUTDOWN_TIMEOUT, SLOW_SHUTDOWN_WARN)
                .await?;
        }
        Ok(())
    }

    /// PID of the running subprocess.
    ///
    /// Surface kept narrow on purpose — most tests should call
    /// `shutdown` and `runtime_client` only. The PID accessor is here
    /// for tests that assert on lifecycle (e.g. "did the Core actually
    /// die after we SIGKILLed it").
    pub fn pid(&self) -> Option<u32> {
        self.process.as_ref().and_then(|h| h.pid())
    }

    /// Path to the backing tempdir's root.
    ///
    /// Useful when a test wants to read `<root>/data/logs/core-*.log`
    /// without hard-coding the layout.
    pub fn tempdir_root(&self) -> &Path {
        self._tempdir.path()
    }
}

impl Drop for CoreUnderTest {
    fn drop(&mut self) {
        // Last-resort SIGKILL. `shutdown().await` is the right answer;
        // this fallback exists so a panicking test doesn't leak
        // subprocesses.
        if let Some(handle) = self.process.take() {
            handle.kill_blocking();
        }
    }
}
