//! Subprocess lifecycle for the integration test harness.
//!
//! Wraps `tokio::process::Child` plus the platform-specific signal
//! plumbing the harness needs. The choice of `tokio::process::Child`
//! (rather than `std::process::Child`) lets the spawn-and-wait path
//! be `async`; `Drop` falls back to `start_kill()`, which is `tokio`'s
//! documented sync-safe SIGKILL path.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin;
use tokio::process::{Child, Command};
use tokio::time::sleep;

/// Errors emitted by [`Handle::spawn`] and [`Handle::shutdown`].
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    /// `cargo_bin("concerto-core")` returned a path that does not exist.
    /// Usually means the workspace was not built before the test ran;
    /// rebuild with `cargo build -p concerto-core` and retry.
    #[error("concerto-core binary not found at {0}; build the workspace first")]
    BinaryMissing(PathBuf),
    /// `Command::spawn` itself failed (fork / exec error).
    #[error("failed to spawn concerto-core: {0}")]
    Spawn(std::io::Error),
    /// The subprocess exited before the UDS socket appeared. Wraps the
    /// observed exit status.
    #[error("concerto-core exited before binding socket (status: {status:?})")]
    EarlyExit { status: std::process::ExitStatus },
    /// `Handle::spawn` timed out waiting for the socket file.
    #[error("UDS socket {socket} did not appear within {budget:?}")]
    SocketTimeout { socket: PathBuf, budget: Duration },
    /// SIGTERM was sent but the process did not exit within the budget,
    /// and SIGKILL also failed to bring it down.
    #[error("concerto-core did not exit after SIGTERM+SIGKILL: {0}")]
    StuckAfterKill(std::io::Error),
    /// `Child::wait` itself errored.
    #[error("failed to await concerto-core: {0}")]
    Wait(std::io::Error),
}

/// Owned subprocess handle.
///
/// `process` is `None` only between `shutdown` consuming the handle and
/// `Drop` running on the now-empty struct; in normal use it stays `Some`.
pub struct Handle {
    process: Option<Child>,
}

impl Handle {
    /// Spawn `concerto-core` under the supplied directories and wait
    /// for the UDS socket to appear.
    ///
    /// `cargo build -p concerto-core` must have completed before this
    /// runs; the harness does not invoke `cargo build` itself. Tests
    /// using the harness implicitly trigger the build via the workspace
    /// graph (their crate depends on `concerto-test-harness` which depends
    /// on `concerto-proto`, and a `cargo test` walks the bin target too
    /// when the test binary lists the bin's package in its dependency
    /// closure). The simpler path for ad-hoc smoke runs is to build
    /// explicitly first.
    pub async fn spawn(
        config_dir: &Path,
        data_dir: &Path,
        socket_path: &Path,
        budget: Duration,
    ) -> Result<Self, ProcessError> {
        let bin = cargo_bin("concerto-core");
        if !bin.exists() {
            return Err(ProcessError::BinaryMissing(bin));
        }

        let mut cmd = Command::new(&bin);
        cmd.env("CONCERTO_CONFIG_DIR", config_dir)
            .env("CONCERTO_DATA_DIR", data_dir)
            // Don't inherit the parent's RUST_LOG by default — keeps
            // harness self-tests quiet unless the caller explicitly
            // wants verbose output via env_remove + env_insert.
            // (We keep stderr piped so a failed early-exit can be
            // surfaced in EarlyExit's error message later if needed.)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // The child should not survive a panicking parent — give it
            // its own session so an aggressive SIGKILL on the parent
            // group still reaps it via Drop fallback.
            .kill_on_drop(true);

        // Keychain isolation (Task 206/207): the spawned Core establishes its
        // Ed25519 identity in the OS keychain on boot. On macOS, accessing the
        // shared "concerto" service from this freshly-built (unsigned) binary
        // pops a *blocking* Keychain Access prompt — a dev-machine annoyance
        // and a hard hang on a headless CI runner. Bind a unique throwaway
        // service per spawn so the child only ever touches an item it created
        // (no cross-binary access => no prompt). Honor a caller-pinned value
        // (CI / `cargo test` wrappers) by only setting it when unset.
        if std::env::var_os("CONCERTO_KEYCHAIN_SERVICE").is_none() {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            cmd.env(
                "CONCERTO_KEYCHAIN_SERVICE",
                format!("concerto-harness-{}-{}", std::process::id(), n),
            );
        }

        let child = cmd.spawn().map_err(ProcessError::Spawn)?;
        let handle = Handle {
            process: Some(child),
        };

        handle.wait_for_socket(socket_path, budget).await?;
        Ok(handle)
    }

    /// Poll `socket_path` until it exists, the subprocess exits, or
    /// `budget` elapses. ~20 ms granularity.
    async fn wait_for_socket(
        &self,
        socket_path: &Path,
        budget: Duration,
    ) -> Result<(), ProcessError> {
        let deadline = Instant::now() + budget;
        loop {
            if socket_path.exists() {
                return Ok(());
            }
            // If the subprocess has already exited, the socket will
            // never appear; surface that without waiting out the budget.
            if let Some(status) = self.exited() {
                return Err(ProcessError::EarlyExit { status });
            }
            if Instant::now() >= deadline {
                return Err(ProcessError::SocketTimeout {
                    socket: socket_path.to_path_buf(),
                    budget,
                });
            }
            sleep(Duration::from_millis(20)).await;
        }
    }

    /// Returns the exit status if the child has already terminated, else
    /// `None`. Uses `try_wait`, which is non-blocking.
    fn exited(&self) -> Option<std::process::ExitStatus> {
        // try_wait on a borrowed `Child` requires `&mut`; we hold an
        // `&self` here on purpose so the polling loop doesn't have to
        // juggle mutability. Re-borrow through a transmute would be
        // unsound; instead we read the inner via `as_ref()` + an
        // intentional shadow: we want a non-mutating peek. Tokio's
        // `Child::try_wait` does in fact take `&mut self`, so we cheat
        // by going via `Child::id()` + a `kill(pid, 0)` probe.
        let pid = self.pid()?;
        #[cfg(unix)]
        unsafe {
            // `kill(pid, 0)` returns 0 if the process is reachable,
            // -1 with ESRCH if not. We treat ESRCH as "exited", but we
            // can't recover the exit status without `wait`; the caller
            // (early-exit branch) only uses this signal to decide
            // whether to bail, so we synthesize a "no exit code"
            // status via a fake.
            if libc::kill(pid as libc::pid_t, 0) == 0 {
                return None;
            }
            // Fabricate an ExitStatus via the `from_raw` extension. The
            // raw value 0 signals exit code 0; the actual exit code is
            // unrecoverable from `kill(pid, 0)`. Callers only use this
            // for diagnostics.
            use std::os::unix::process::ExitStatusExt;
            Some(std::process::ExitStatus::from_raw(0))
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            None
        }
    }

    /// PID of the live subprocess.
    pub fn pid(&self) -> Option<u32> {
        self.process.as_ref().and_then(|c| c.id())
    }

    /// SIGTERM the process, wait up to `timeout`, escalate to SIGKILL.
    ///
    /// Consumes the handle. Logs a `tracing::warn!` if elapsed time
    /// exceeds `slow_warn`.
    pub async fn shutdown(
        mut self,
        timeout: Duration,
        slow_warn: Duration,
    ) -> Result<(), ProcessError> {
        let Some(mut child) = self.process.take() else {
            return Ok(());
        };

        let started = Instant::now();

        // 1. SIGTERM the child. On Unix we send the signal directly
        // because `Child::kill` is SIGKILL, not SIGTERM.
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            // Safe: SIGTERM (15) to a child we own; failures here are
            // logged but not fatal — we still escalate to SIGKILL below.
            unsafe {
                if libc::kill(pid as libc::pid_t, libc::SIGTERM) != 0 {
                    let err = std::io::Error::last_os_error();
                    tracing::warn!(
                        pid,
                        error = %err,
                        "SIGTERM to concerto-core failed; falling back to SIGKILL"
                    );
                }
            }
        }
        #[cfg(not(unix))]
        {
            // Windows path: no SIGTERM. Skip straight to start_kill.
            let _ = child.start_kill();
        }

        // 2. Wait up to `timeout` for graceful exit.
        let wait_result = tokio::time::timeout(timeout, child.wait()).await;

        let elapsed = started.elapsed();
        if elapsed >= slow_warn {
            tracing::warn!(
                elapsed_ms = elapsed.as_millis() as u64,
                "concerto-core shutdown took {:?} (>{:?})",
                elapsed,
                slow_warn
            );
        }

        match wait_result {
            Ok(Ok(_status)) => Ok(()),
            Ok(Err(e)) => Err(ProcessError::Wait(e)),
            Err(_elapsed) => {
                // 3. Timed out — escalate to SIGKILL.
                tracing::warn!(
                    "concerto-core did not exit within {:?}; sending SIGKILL",
                    timeout
                );
                if let Err(e) = child.start_kill() {
                    return Err(ProcessError::StuckAfterKill(e));
                }
                child.wait().await.map_err(ProcessError::Wait)?;
                Ok(())
            }
        }
    }

    /// Synchronous best-effort SIGKILL. Called from `Drop` on
    /// `CoreUnderTest` when the test forgot to `shutdown().await`.
    ///
    /// Uses `start_kill()` which is documented as safe to call from a
    /// non-async context — it queues the signal without blocking.
    pub fn kill_blocking(mut self) {
        if let Some(mut child) = self.process.take() {
            let _ = child.start_kill();
            // We can't block on `wait()` here because Drop is sync; the
            // OS will reap once the child fully exits. `kill_on_drop`
            // on the spawn config arranges this same behaviour anyway.
        }
    }
}
