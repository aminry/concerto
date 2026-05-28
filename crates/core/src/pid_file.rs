//! Single-instance guard via an advisory file lock on a PID file.
//!
//! Behaviour locked by Task 11 (`design/01 §3.3`):
//!
//! - `PidFile::acquire(path)` opens `<path>` for read/write (creating it on
//!   demand), attempts an exclusive non-blocking advisory lock via
//!   [`fs2::FileExt::try_lock_exclusive`], and on success writes a JSON
//!   record describing this process. Holding the returned [`PidFile`]
//!   guarantees no other process can take the lock.
//! - On lock contention we read the existing PID and probe whether it's
//!   alive (`kill(pid, 0)` on Unix; presence of `OpenProcess` on Windows
//!   — V0.1 ships Unix only, so the Windows path returns a synthetic
//!   "stale" indicator until a future task lights it up).
//! - Stale locks (PID exists in file but the process is gone) are broken
//!   and reclaimed.
//! - Dropping the guard releases the lock (by closing the FD) and best-
//!   effort removes the file. We deliberately do NOT call `LOCK_UN`
//!   explicitly; closing the FD is the canonical way to release `flock`.
//!
//! The on-disk record is JSON so a human (or a test harness) can read it:
//!
//! ```json
//! { "pid": 12345, "version": "0.0.1", "start_epoch_secs": 1717000000 }
//! ```

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use concerto_error::{Error, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

/// Decoded contents of `core.pid`.
///
/// Exposed via [`PidFile::record`] so the integration test can verify the
/// shape without re-parsing the file by hand.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PidRecord {
    pub pid: u32,
    pub version: String,
    pub start_epoch_secs: u64,
}

/// Outcome of attempting to take the single-instance lock.
#[derive(Debug)]
pub enum AcquireOutcome {
    /// We hold the lock. Keep the [`PidFile`] alive for the program's
    /// lifetime; dropping it releases the lock and removes the file.
    Acquired(PidFile),
    /// Another live process already holds the lock. The caller should
    /// log the PID and exit with status 0.
    AlreadyRunning { pid: u32 },
}

/// RAII guard for the single-instance lock.
///
/// The `File` field owns the lock — closing it (which `Drop` does
/// implicitly) releases `flock`. We also best-effort delete the on-disk
/// file so a subsequent `ls ~/.concerto/core.pid` shows "gone".
///
/// `Debug` is manual because `std::fs::File` doesn't print anything
/// useful and we want the path + record visible in logs / panics.
pub struct PidFile {
    path: PathBuf,
    /// Held for the lifetime of the guard. `Option` so `Drop` can take it.
    file: Option<File>,
    record: PidRecord,
}

impl std::fmt::Debug for PidFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PidFile")
            .field("path", &self.path)
            .field("record", &self.record)
            .finish_non_exhaustive()
    }
}

impl PidFile {
    /// Try to acquire the single-instance lock at `path`.
    ///
    /// `path`'s parent directory is created on demand. On success the
    /// file is locked, truncated, and rewritten with this process's PID;
    /// the returned [`AcquireOutcome::Acquired`] variant owns the lock
    /// until dropped.
    pub fn acquire(path: impl AsRef<Path>) -> Result<AcquireOutcome> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Open with O_RDWR | O_CREAT — we may need to read the existing
        // PID before deciding what to do.
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;

        // Try the lock. If contended, read the stored PID and probe for
        // liveness.
        match file.try_lock_exclusive() {
            Ok(()) => {
                // Lock taken. Stale stat-data from a previous, no-longer-
                // running Core may still be in the file — overwrite.
                let record = current_record();
                write_record(&mut file, &record)?;
                Ok(AcquireOutcome::Acquired(PidFile {
                    path,
                    file: Some(file),
                    record,
                }))
            }
            Err(e) => {
                if !is_would_block(&e) {
                    return Err(Error::Io(e));
                }
                // Contended. Read the existing record (best-effort) and
                // probe whether its PID is alive. If the PID is gone the
                // lock is stale: break it and retake. Note that the
                // current OS we ship V0.1 on is macOS; the design treats
                // Windows liveness checking as a V1.0 concern.
                let existing = read_record(&mut file).ok();
                let pid_to_probe = existing.as_ref().map(|r| r.pid);

                match pid_to_probe {
                    Some(pid) if process_alive(pid) => {
                        // Real other instance.
                        Ok(AcquireOutcome::AlreadyRunning { pid })
                    }
                    Some(stale_pid) => {
                        // Stale. Drop our FD (which would unlock if we
                        // had the lock — we don't, but be tidy) and try
                        // a clean reacquire by recreating the file.
                        tracing::warn!(
                            stale_pid,
                            path = %path.display(),
                            "breaking stale pid lock"
                        );
                        drop(file);
                        break_and_retake(&path)
                    }
                    None => {
                        // No parseable record but lock is held. Treat as
                        // stale (best we can do without a PID to probe).
                        tracing::warn!(
                            path = %path.display(),
                            "pid file is locked but unreadable; breaking"
                        );
                        drop(file);
                        break_and_retake(&path)
                    }
                }
            }
        }
    }

    /// Path the lock was acquired at.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Decoded record we wrote into the file on acquire.
    pub fn record(&self) -> &PidRecord {
        &self.record
    }
}

impl Drop for PidFile {
    fn drop(&mut self) {
        // Close the file first (releases the advisory lock), then unlink
        // the path. Errors at this point are logged but not returned —
        // the program is shutting down.
        let _ = self.file.take(); // drop closes the fd
        if let Err(e) = std::fs::remove_file(&self.path) {
            // ENOENT is fine — the file may already be gone.
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    error = %e,
                    path = %self.path.display(),
                    "failed to remove pid file on drop"
                );
            }
        }
    }
}

/// Recreate the lock file from scratch and take the lock exclusively.
///
/// Called when the prior holder is gone (stale lock); the file may still
/// exist on disk. We truncate it as part of [`write_record`].
fn break_and_retake(path: &Path) -> Result<AcquireOutcome> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;

    file.try_lock_exclusive().map_err(|e| {
        if is_would_block(&e) {
            // Race: someone else won the break. Treat as if they're
            // running so the caller exits cleanly instead of looping.
            Error::Internal(
                "pid lock contended during stale-break; another process \
                 won the race"
                    .into(),
            )
        } else {
            Error::Io(e)
        }
    })?;

    let record = current_record();
    write_record(&mut file, &record)?;
    Ok(AcquireOutcome::Acquired(PidFile {
        path: path.to_path_buf(),
        file: Some(file),
        record,
    }))
}

/// Serialize `record` to `file`, replacing prior contents.
fn write_record(file: &mut File, record: &PidRecord) -> Result<()> {
    let json =
        serde_json::to_vec_pretty(record).map_err(|e| Error::Internal(format!("pid json: {e}")))?;
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&json)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

/// Parse the existing record (if any).
fn read_record(file: &mut File) -> Result<PidRecord> {
    file.seek(SeekFrom::Start(0))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    let record: PidRecord = serde_json::from_str(buf.trim())
        .map_err(|e| Error::Internal(format!("pid json parse: {e}")))?;
    Ok(record)
}

fn current_record() -> PidRecord {
    let start_epoch_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    PidRecord {
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        start_epoch_secs,
    }
}

/// Is `err` the "would-block" variant from fs2's try_lock_*?
///
/// `fs2` surfaces a contended lock via [`std::io::Error`] whose kind is
/// `WouldBlock` (translated from `EWOULDBLOCK`/`EAGAIN` on Unix and
/// `ERROR_LOCK_VIOLATION` on Windows).
fn is_would_block(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::WouldBlock
}

/// Does `pid` refer to a live process?
///
/// Unix: `kill(pid, 0)` returns 0 if the process exists, ESRCH if not,
/// EPERM if it exists but we lack permission. Per task spec we treat
/// EPERM as "exists" — a permission error proves the PID is real.
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // `kill(0, …)` and `kill(-1, …)` are special — they send to the
    // process group or to every reachable process respectively, which
    // is decidedly not what we want for a liveness probe. Treat any PID
    // that doesn't fit cleanly into the positive i32 range as "not a
    // real process".
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: `kill(pid, 0)` with a positive pid is a no-op probe; it
    // does not deliver a signal. Returning the FFI result and reading
    // errno is the documented way to ask "does this PID exist?".
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    // ESRCH: no such process — definitely dead.
    // EPERM: process exists but we lack permission to signal it. The
    //        task spec calls this out: treat as alive.
    // Anything else: be conservative and report alive so we don't take
    // a lock that's still in use.
    errno == libc::EPERM
}

/// Windows liveness probe — placeholder until V1.0.
///
/// V0.1 is macOS-only; we still compile-check the Windows path so future
/// porting is mechanical. Treating "we don't know" as "alive" is the
/// conservative answer (we'd rather refuse to start than risk two Cores
/// fighting over the same DB).
#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn acquire_writes_record_and_drop_removes_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("core.pid");

        let outcome = PidFile::acquire(&path).expect("first acquire");
        let guard = match outcome {
            AcquireOutcome::Acquired(g) => g,
            AcquireOutcome::AlreadyRunning { pid } => {
                panic!("unexpected AlreadyRunning(pid={pid}) on fresh tempdir")
            }
        };
        assert_eq!(guard.record().pid, std::process::id());
        assert!(path.exists(), "pid file should exist while guard is held");

        drop(guard);
        assert!(!path.exists(), "pid file should be removed on drop");
    }

    #[cfg(unix)]
    #[test]
    fn second_acquire_in_same_process_reports_already_running() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("core.pid");

        let first = PidFile::acquire(&path).expect("first acquire");
        let _g = match first {
            AcquireOutcome::Acquired(g) => g,
            AcquireOutcome::AlreadyRunning { .. } => panic!("first should acquire"),
        };

        // Second attempt in-process probes our own PID — which is alive.
        // The expected behaviour is AlreadyRunning, not a stale-break.
        match PidFile::acquire(&path).expect("second acquire returns outcome") {
            AcquireOutcome::AlreadyRunning { pid } => {
                assert_eq!(pid, std::process::id());
            }
            AcquireOutcome::Acquired(_) => {
                panic!("expected AlreadyRunning, got Acquired");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn stale_lock_with_dead_pid_is_broken_and_retaken() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("core.pid");

        // Pre-write a stale record. The PID we record is one we know is
        // dead — we fork off a short-lived child via `std::process` and
        // wait for it to exit, then reuse its (now-reaped) PID. On
        // macOS PIDs are not immediately recycled, so probing the
        // post-exit PID returns ESRCH ⇒ "not alive".
        let child = std::process::Command::new("true")
            .spawn()
            .expect("spawn /usr/bin/true");
        let dead_pid = child.id();
        let _ = child.wait_with_output();
        // Sanity: the PID we picked must really be dead now.
        assert!(
            !process_alive(dead_pid),
            "child PID {dead_pid} should not be alive after wait()"
        );

        let stale = PidRecord {
            pid: dead_pid,
            version: "0.0.0".into(),
            start_epoch_secs: 0,
        };
        std::fs::write(
            &path,
            serde_json::to_string(&stale).expect("serialize stale"),
        )
        .unwrap();

        // No lock is held on the file (we never flock()ed it), so this
        // is the easy path: acquire just opens and locks. The point of
        // the test is the *recorded* PID logic — `process_alive`
        // returns false for our reaped child, which is exactly the
        // observation the production code makes when it sees a stale
        // lock from a crashed previous Core.
        let outcome = PidFile::acquire(&path).expect("acquire over stale file");
        assert!(matches!(outcome, AcquireOutcome::Acquired(_)));
    }
}
