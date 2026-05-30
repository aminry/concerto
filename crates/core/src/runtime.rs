//! Core daemon runtime skeleton (Task 11).
//!
//! Brings together:
//!
//! - The single-instance guard ([`crate::pid_file`]).
//! - Signal handling ([`crate::signals`]).
//! - The persistence handle ([`concerto_persist::Persistence`]).
//! - A [`CancellationToken`] every actor will eventually subscribe to.
//!
//! Actor supervision lands in Task 12. This file is the plumbing the
//! supervisor will hook into; today there are no children yet, so
//! [`Runtime::stop`] reduces to "drop the signal listener, shut down
//! persistence, release the lock".
//!
//! Path layout, per `design/01 §4.1`:
//!
//! ```text
//! <data_dir>/                ← default ~/concerto/
//!     concerto.db
//!     logs/core-YYYY-MM-DD.log
//!     audit/                 ← Task 44
//! <config_dir>/              ← default ~/.concerto/
//!     core.pid               ← single-instance lock + record (Task 11)
//!     core.sock              ← UDS for local API (Task 13)
//!     config.json            ← user config (Task ~later)
//!     managed.json           ← org-managed overrides
//! ```
//!
//! Two directories: `data_dir` (persistent, large) and `config_dir`
//! (small, ephemeral process state). The split is what the design doc
//! locks; Task 08 already uses `data_dir/concerto.db` so we are simply
//! making the second half visible here.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use concerto_error::{Error, Result};
use concerto_persist::{Persistence, PersistenceConfig};
use tokio_util::sync::CancellationToken;

use crate::pid_file::{AcquireOutcome, PidFile};
use crate::signals::{self, ReloadEvent};
use crate::supervisor::RootSupervisor;

/// Filesystem layout for a running Core.
///
/// `default_for_user()` resolves to `~/concerto/` (data) +
/// `~/.concerto/` (config). Tests and the smoke gate override both
/// via environment.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Where persistent state lives (SQLite DB, logs, audit JSONL).
    /// Default: `~/concerto/`.
    pub data_dir: PathBuf,
    /// Where ephemeral process state lives (PID lock, local UDS).
    /// Default: `~/.concerto/`.
    pub config_dir: PathBuf,
    /// Time we wait between cancelling the shutdown token and
    /// shutting down persistence. V0.1 has no actors yet; Task 12+
    /// will start subscribing.
    pub shutdown_grace: Duration,
}

impl RuntimeConfig {
    /// Resolve from `$HOME` plus environment overrides.
    ///
    /// Recognized variables (precedence: env > default):
    ///
    /// - `CONCERTO_DATA_DIR` — overrides `data_dir`.
    /// - `CONCERTO_CONFIG_DIR` — overrides `config_dir`.
    /// - `CONCERTO_DB_PATH` — overrides the resolved SQLite path
    ///   (re-exposed by `main.rs` for backwards compatibility with the
    ///   Task 08 smoke gate; not a `RuntimeConfig` field).
    pub fn default_for_user() -> Result<Self> {
        let home = home::home_dir()
            .ok_or_else(|| Error::Internal("home::home_dir() returned None".into()))?;

        let data_dir = std::env::var("CONCERTO_DATA_DIR")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("concerto"));

        let config_dir = std::env::var("CONCERTO_CONFIG_DIR")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".concerto"));

        Ok(Self {
            data_dir,
            config_dir,
            shutdown_grace: Duration::from_secs(5),
        })
    }

    /// Absolute path of the PID lock file.
    pub fn pid_file_path(&self) -> PathBuf {
        self.config_dir.join("core.pid")
    }

    /// Absolute path of the SQLite database.
    ///
    /// Honours `CONCERTO_DB_PATH` for backwards compatibility with the
    /// Task 08 wiring (which the smoke gate already depends on); falls
    /// back to `<data_dir>/concerto.db` otherwise.
    pub fn db_path(&self) -> PathBuf {
        if let Ok(p) = std::env::var("CONCERTO_DB_PATH") {
            if !p.is_empty() {
                return PathBuf::from(p);
            }
        }
        self.data_dir.join("concerto.db")
    }
}

/// What [`Runtime::start`] returns to the caller.
///
/// `Started` is the happy path. `AlreadyRunning` means another Core
/// already holds the PID lock — the binary should log and exit 0, NOT
/// loop or treat this as an error.
///
/// `Runtime` is the dominant variant by design; constructed at most
/// once per process and consumed shortly thereafter. The size disparity
/// vs `AlreadyRunning { pid }` is intentional — boxing would force
/// every caller through a redundant pointer dereference.
#[allow(clippy::large_enum_variant)]
pub enum StartOutcome {
    Started(Runtime),
    AlreadyRunning { pid: u32 },
}

/// The live Core runtime.
///
/// Holds (in shutdown order):
/// 1. The signal listener join handle.
/// 2. The persistence handle.
/// 3. The pid-file guard (lock + on-disk file).
///
/// `stop` consumes `self` and shuts these down in that order. `Drop`
/// is best-effort: if the caller forgets to `stop().await`, the
/// pid-file guard still releases the lock and removes the file when
/// the struct goes out of scope; persistence may leak a pool.
pub struct Runtime {
    pid_file: Option<PidFile>,
    shutdown: CancellationToken,
    /// Persistence is shared with every supervised actor via
    /// `Arc::clone`. `None` after [`Runtime::stop`] has consumed it.
    persistence: Option<Arc<Persistence>>,
    signal_listener: Option<tokio::task::JoinHandle<()>>,
    reload_rx: Option<tokio::sync::mpsc::Receiver<ReloadEvent>>,
    shutdown_grace: Duration,
    /// Task 12 supervision tree. Embedded so subsequent tasks (Task 13
    /// onward) can register actors via [`Runtime::supervisor_mut`].
    /// `None` after [`Runtime::stop`] has consumed the supervisor.
    supervisor: Option<RootSupervisor>,
    /// Wall-clock instant at which `Runtime::start` succeeded. Wrapped in
    /// `Arc` so the gRPC `RuntimeHandler` (Task 13) can clone a snapshot
    /// once at construction and read it without further synchronization
    /// on each `GetStatus` call.
    started_at: Arc<SystemTime>,
}

impl Runtime {
    /// Boot the runtime.
    ///
    /// Steps, in order — any failure here unwinds cleanly:
    ///
    /// 1. Acquire the PID lock. If contended by a live process, return
    ///    [`StartOutcome::AlreadyRunning`].
    /// 2. Create `data_dir` (where the DB will live; persistence will
    ///    re-create the parent of `db_path` too, but doing it here keeps
    ///    error messages tidy).
    /// 3. Open persistence — runs migrations + integrity check.
    /// 4. Install signal handlers; they subscribe to a shared
    ///    [`CancellationToken`].
    pub async fn start(config: RuntimeConfig) -> Result<StartOutcome> {
        // 1. Signals FIRST — before the pid file is written.
        //
        // The pid file is the readiness signal external supervisors (and the
        // `runtime_lifecycle` integration test) wait on. Arming the SIGTERM /
        // SIGINT / SIGHUP handlers before it exists guarantees that any signal
        // observed once the pid file is present triggers a graceful shutdown
        // (token cancellation) rather than the kernel's default terminate
        // disposition. Previously signals were installed several steps later
        // (after `Persistence::open`), leaving a window where a SIGTERM landing
        // between the pid-file write and signal install killed the process with
        // status "signal 15" — a startup race that flaked under load.
        let shutdown = CancellationToken::new();
        let (signal_listener, reload_rx) = signals::install(shutdown.clone())?;
        tracing::info!("signal handlers installed");

        // 2. Single-instance lock.
        let pid_path = config.pid_file_path();
        let pid_file = match PidFile::acquire(&pid_path)? {
            AcquireOutcome::Acquired(g) => g,
            AcquireOutcome::AlreadyRunning { pid } => {
                tracing::info!(
                    other_pid = pid,
                    pid_file = %pid_path.display(),
                    "another concerto-core instance is already running"
                );
                return Ok(StartOutcome::AlreadyRunning { pid });
            }
        };
        tracing::info!(
            pid = pid_file.record().pid,
            version = pid_file.record().version,
            pid_file = %pid_path.display(),
            "acquired single-instance lock"
        );

        // 3. Ensure data_dir exists for downstream subsystems.
        tokio::fs::create_dir_all(&config.data_dir).await?;

        // 4. Persistence.
        let persist_config = PersistenceConfig {
            db_path: config.db_path(),
            max_readers: 8,
        };
        tracing::info!(
            db_path = %persist_config.db_path.display(),
            max_readers = persist_config.max_readers,
            "opening persistence"
        );
        let persistence = Arc::new(Persistence::open(persist_config).await?);
        tracing::info!("persistence ready");

        // 5. Task 12 supervision tree. No actors yet — they are
        // registered by later tasks via `Runtime::supervisor_mut`.
        let supervisor = RootSupervisor::new(Arc::clone(&persistence), shutdown.clone());
        tracing::debug!(actors = supervisor.actor_count(), "RootSupervisor ready");

        Ok(StartOutcome::Started(Runtime {
            pid_file: Some(pid_file),
            shutdown,
            persistence: Some(persistence),
            signal_listener: Some(signal_listener),
            reload_rx: Some(reload_rx),
            shutdown_grace: config.shutdown_grace,
            supervisor: Some(supervisor),
            started_at: Arc::new(SystemTime::now()),
        }))
    }

    /// Snapshot of the wall-clock instant at which the runtime booted.
    ///
    /// `Arc` is the storage so callers can clone cheaply and hand the
    /// value to subsystems (e.g. the gRPC `RuntimeHandler` from Task 13)
    /// without re-reading the clock on every RPC.
    pub fn started_at(&self) -> Arc<SystemTime> {
        Arc::clone(&self.started_at)
    }

    /// Hand out a [`CancellationToken`] subscribers cancel-on. Cheap
    /// to clone; pass clones to every spawned task that should join
    /// the shutdown party.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    /// Take the reload-event receiver. Returns `None` if the runtime
    /// already handed it out (only one consumer is meaningful).
    ///
    /// V0.1 ships no consumer; this is here so Task 12's supervisor
    /// can wire in without re-locking the public API.
    pub fn take_reload_rx(&mut self) -> Option<tokio::sync::mpsc::Receiver<ReloadEvent>> {
        self.reload_rx.take()
    }

    /// Block until either a signal fires or the shutdown token is
    /// cancelled programmatically (e.g. by an admin RPC in Task ~13).
    ///
    /// Returns `Ok(())` whenever shutdown is requested. There's no
    /// error path in V0.1; the signature is `Result` so future
    /// causes (e.g. watchdog-detected hang) can surface.
    pub async fn wait_for_shutdown(&self) -> Result<()> {
        self.shutdown.cancelled().await;
        Ok(())
    }

    /// Borrow the persistence handle.
    ///
    /// Returns the `Arc<Persistence>` shared with the supervision tree
    /// — `None` after [`Runtime::stop`] has consumed it.
    pub fn persistence(&self) -> Option<&Arc<Persistence>> {
        self.persistence.as_ref()
    }

    /// Mutable handle to the supervisor.
    ///
    /// Callers (Task 13+) register actors via this handle. Returns
    /// `None` after [`Runtime::stop`] has consumed the supervisor —
    /// in practice only the test suite ever sees that case.
    pub fn supervisor_mut(&mut self) -> Option<&mut RootSupervisor> {
        self.supervisor.as_mut()
    }

    /// Read-only handle to the supervisor (e.g. for `RuntimeAdmin::GetStatus`
    /// in Task 13, which only needs `list`).
    pub fn supervisor(&self) -> Option<&RootSupervisor> {
        self.supervisor.as_ref()
    }

    /// Graceful shutdown.
    ///
    /// Sequence per `design/01 §6.4`:
    /// 1. Cancel the shutdown token (idempotent — `wait_for_shutdown`
    ///    may already have observed the cancel that brought us here).
    /// 2. Drain the supervisor: each actor gets up to 10s
    ///    (`SHUTDOWN_DRAIN_BUDGET`) to finish before its task is
    ///    aborted.
    /// 3. Wait up to `shutdown_grace` for the signal listener to exit.
    /// 4. Shut down persistence (closes reader pool + writer conn).
    /// 5. Drop the PID guard (releases flock, removes the file).
    pub async fn stop(mut self) -> Result<()> {
        tracing::info!("runtime shutdown beginning");
        self.shutdown.cancel();

        // Step 2: supervisor. Consumes the RootSupervisor; it cancels
        // each child and joins with a per-actor budget.
        if let Some(supervisor) = self.supervisor.take() {
            supervisor.shutdown().await?;
        }

        // Step 3: drain the signal listener. It cancels on first signal
        // and exits; if it was the one that cancelled the token, this
        // is essentially immediate. If we cancelled programmatically,
        // the listener notices on its `select!`.
        if let Some(handle) = self.signal_listener.take() {
            match tokio::time::timeout(self.shutdown_grace, handle).await {
                Ok(Ok(())) => tracing::debug!("signal listener joined"),
                Ok(Err(join_err)) => {
                    tracing::warn!(error = %join_err, "signal listener panicked during shutdown")
                }
                Err(_) => tracing::warn!(
                    "signal listener did not exit within {:?}; abandoning",
                    self.shutdown_grace
                ),
            }
        }

        // Step 4: persistence. The `Arc` is shared with actors; by now
        // they've all been joined (or aborted), so this clone count
        // should be 1. If a leaked clone keeps it alive we degrade
        // gracefully — `Arc::try_unwrap` returns the inner Persistence
        // for the `shutdown` call, otherwise we drop our clone and let
        // the last holder close it on Drop.
        if let Some(persist_arc) = self.persistence.take() {
            tracing::info!("closing persistence");
            match Arc::try_unwrap(persist_arc) {
                Ok(persist) => persist.shutdown().await?,
                Err(arc) => {
                    tracing::warn!(
                        strong_count = Arc::strong_count(&arc),
                        "persistence still has outstanding references at shutdown; dropping our clone"
                    );
                    drop(arc);
                }
            }
        }

        // Step 5: pid file. Explicit `drop` so the order is visible in
        // the source even though the `Drop` impl would handle it.
        if let Some(pf) = self.pid_file.take() {
            drop(pf);
        }

        tracing::info!("runtime shutdown complete");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a `RuntimeConfig` that's fully sandboxed under `tmp`.
    fn sandbox(tmp: &TempDir) -> RuntimeConfig {
        RuntimeConfig {
            data_dir: tmp.path().join("data"),
            config_dir: tmp.path().join("config"),
            shutdown_grace: Duration::from_secs(2),
        }
    }

    #[tokio::test]
    async fn start_then_stop_cleans_up_pid_file() {
        let tmp = TempDir::new().unwrap();
        let cfg = sandbox(&tmp);
        let pid_path = cfg.pid_file_path();

        let runtime = match Runtime::start(cfg).await.expect("start") {
            StartOutcome::Started(r) => r,
            StartOutcome::AlreadyRunning { pid } => {
                panic!("unexpected AlreadyRunning(pid={pid}) on fresh tempdir")
            }
        };
        assert!(
            pid_path.exists(),
            "pid file should exist while runtime runs"
        );

        runtime.stop().await.expect("stop");
        assert!(
            !pid_path.exists(),
            "pid file should be removed after Runtime::stop"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn second_start_returns_already_running() {
        let tmp = TempDir::new().unwrap();
        let cfg = sandbox(&tmp);

        let runtime = match Runtime::start(cfg.clone()).await.expect("first start") {
            StartOutcome::Started(r) => r,
            other => panic!("expected Started, got {:?}", outcome_kind(&other)),
        };

        match Runtime::start(cfg).await.expect("second start") {
            StartOutcome::AlreadyRunning { pid } => {
                assert_eq!(pid, std::process::id(), "should detect this process");
            }
            StartOutcome::Started(_) => panic!("second start should detect lock"),
        }

        runtime.stop().await.expect("stop");
    }

    #[tokio::test]
    async fn programmatic_cancel_unblocks_wait_for_shutdown() {
        let tmp = TempDir::new().unwrap();
        let runtime = match Runtime::start(sandbox(&tmp)).await.expect("start") {
            StartOutcome::Started(r) => r,
            other => panic!("expected Started, got {:?}", outcome_kind(&other)),
        };

        let token = runtime.shutdown_token();

        // Schedule a programmatic cancel.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            token.cancel();
        });

        // Should return promptly.
        let res = tokio::time::timeout(Duration::from_secs(2), runtime.wait_for_shutdown()).await;
        assert!(
            matches!(res, Ok(Ok(()))),
            "wait_for_shutdown should return after programmatic cancel"
        );

        runtime.stop().await.expect("stop");
    }

    fn outcome_kind(o: &StartOutcome) -> &'static str {
        match o {
            StartOutcome::Started(_) => "Started",
            StartOutcome::AlreadyRunning { .. } => "AlreadyRunning",
        }
    }
}
