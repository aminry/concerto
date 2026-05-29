//! `concerto-agent-host` process spawning (Task 22).
//!
//! Two locked behaviours live here:
//!
//! 1. **Detachment via `pre_exec(setsid)`.** Per Task 21's Handoff Notes
//!    the host binary does NOT fork itself; the Core arranges
//!    session-leader status by setting a `pre_exec` callback that calls
//!    `libc::setsid()` before the host's `execve`. After this the host's
//!    parent becomes `launchd`/`init` on the next reparent, satisfying
//!    the surviving-host invariant from `design/01 §6.3`.
//!
//! 2. **Socket-poll wait.** The host binds its UDS asynchronously after
//!    spawning. The Core polls for the socket file to appear with a
//!    10-second budget per `design/04 §6.1`; on timeout the host
//!    process is killed and an error returned.
//!
//! Both behaviours are Unix-only and the whole module is gated
//! `#[cfg(unix)]` at the parent.

use std::path::{Path, PathBuf};
use std::time::Duration;

use concerto_error::{Error, Result};
use tokio::process::{Child, Command};

/// Default budget for the socket-appearance poll. Matches the
/// 10-second value called out in `design/04 §6.1` and Task 22's
/// implementation notes.
pub const SOCKET_POLL_BUDGET: Duration = Duration::from_secs(10);

/// Resolve the absolute path to the `concerto-agent-host` binary at
/// runtime. Production code locates the helper next to the running
/// `concerto-core` binary (`current_exe().parent()`); tests can pass
/// an override path obtained via `assert_cmd::cargo::cargo_bin`.
pub fn default_host_binary() -> Result<PathBuf> {
    let exe = std::env::current_exe().map_err(Error::Io)?;
    let parent = exe.parent().ok_or_else(|| {
        Error::Internal("current_exe() has no parent directory; cannot locate agent-host".into())
    })?;
    Ok(parent.join("concerto-agent-host"))
}

/// Spawn `concerto-agent-host` with the locked argv shape. The returned
/// [`Child`] is owned by the caller — drop or `kill().await` to stop it.
///
/// `pre_exec(setsid)` is applied on Unix so the host becomes the leader
/// of a new session and survives the Core's exit. This is `unsafe` only
/// because the callback runs between `fork` and `exec`; calling
/// `libc::setsid` there is one of the documented safe operations.
#[allow(clippy::too_many_arguments)]
pub fn spawn_host(
    host_bin: &Path,
    agent_bin: &str,
    agent_args: &[String],
    cwd: &Path,
    socket: &Path,
    cookie_hex: &str,
    final_info: &Path,
    resume_jsonl: Option<&str>,
) -> Result<Child> {
    let mut cmd = Command::new(host_bin);
    cmd.arg("--agent-bin").arg(agent_bin);
    // Use the `=` form so agent-args that start with `-` (e.g. `-c`)
    // are not parsed by clap as separate flags. The echo path passes
    // `["-c", "echo hello; sleep 0.1"]` to `/bin/sh`.
    for a in agent_args {
        cmd.arg(format!("--agent-arg={a}"));
    }
    cmd.arg("--cwd").arg(cwd);
    cmd.arg("--socket").arg(socket);
    cmd.arg("--cookie").arg(cookie_hex);
    cmd.arg("--final-info").arg(final_info);
    // Task 37: forward the agent CLI's own resume token so the wrapped
    // CLI (Claude / Codex) loads its conversation JSONL from disk. The
    // agent-host CLI parameter is `--resume-jsonl` for historical
    // reasons (Task 21 named it after the on-disk artefact); the
    // wrapped agent CLI receives a plain `--resume <token>`.
    if let Some(token) = resume_jsonl {
        cmd.arg("--resume-jsonl").arg(token);
    }

    // Detach via setsid so the host outlives the Core. The closure is
    // `unsafe` because it runs in the fragile post-fork/pre-exec window;
    // calling `libc::setsid` there is safe (no allocator interaction,
    // signal-safe per POSIX).
    // SAFETY: `pre_exec` runs after fork and before exec. The closure
    // may only call signal-safe / async-signal-safe operations.
    // `libc::setsid()` is documented as async-signal-safe and is the
    // canonical way to detach a child from the parent's controlling tty
    // + session. `tokio::process::Command::pre_exec` matches
    // `std::os::unix::process::CommandExt::pre_exec` semantics; we use
    // tokio's inherent method directly so no extra trait import is
    // needed.
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            let _ = libc::setsid();
            Ok(())
        });
    }

    // Inherit stderr so the host's tracing output lands in the same
    // place as the Core's; close stdin/stdout to the child since the
    // wire traffic flows over the UDS, not the std streams.
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::inherit());

    cmd.spawn().map_err(Error::Io)
}

/// Poll for the host's socket file to appear, with a budget.
///
/// Polls every 50 ms; returns as soon as the path exists. On timeout
/// returns `Error::Internal` so the caller can clean up the host
/// process.
pub async fn wait_for_socket(socket: &Path, budget: Duration) -> Result<()> {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if tokio::fs::metadata(socket).await.is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Error::Internal(format!(
                "agent-host socket {} did not appear within {:?}",
                socket.display(),
                budget
            )));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
