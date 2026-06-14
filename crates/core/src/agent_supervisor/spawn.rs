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

use crate::agent_supervisor::actor::AgentKind;

/// Default budget for the socket-appearance poll. Matches the
/// 10-second value called out in `design/04 §6.1` and Task 22's
/// implementation notes.
pub const SOCKET_POLL_BUDGET: Duration = Duration::from_secs(10);

/// Environment variable that, when set to a non-empty absolute path,
/// overrides all other resolution strategies for the
/// `concerto-agent-host` binary. Highest-precedence override (Task 106).
///
/// Locked env contract: the value is an absolute path to the binary
/// itself (not a directory). `scripts/dev-embedded.sh` sets this to the
/// freshly built path so embedded-Core dev never depends on co-location
/// by accident.
pub const HOST_BIN_ENV: &str = "CONCERTO_AGENT_HOST_BIN";

/// Base name of the helper binary (without any platform extension).
const HOST_BIN_STEM: &str = "concerto-agent-host";

/// How many directory levels above the running executable's directory to
/// probe in the dev-layout search. Bounded on purpose — we never scan the
/// filesystem; this only covers the cargo-dev / embedded layouts where the
/// desktop (or test) binary and the helper share one `target/<profile>/`.
const TARGET_SEARCH_MAX_LEVELS: usize = 3;

/// Platform-specific file name for the helper binary. On Windows the
/// host gains a `.exe` suffix (Task 702 owns the actual ConPTY backend;
/// this is only so the *search* doesn't break the future Windows build).
fn host_bin_filename() -> String {
    if cfg!(windows) {
        format!("{HOST_BIN_STEM}.exe")
    } else {
        HOST_BIN_STEM.to_string()
    }
}

/// Resolve the absolute path to the `concerto-agent-host` binary at
/// runtime (Task 106).
///
/// Tries, in order, returning the first path that exists:
///
/// 1. `$CONCERTO_AGENT_HOST_BIN` — explicit absolute-path override, if
///    set and non-empty (highest precedence; locked env contract).
/// 2. `current_exe().parent()/concerto-agent-host[.exe]` — the
///    packaged / co-located case (unchanged behaviour from Task 22).
/// 3. A `concerto-agent-host[.exe]` reached by walking up a bounded
///    couple of levels from the executable's directory — the cargo-dev /
///    embedded case where the desktop (or test) binary and the helper
///    share one `target/<profile>/`. At each ancestor we probe both the
///    ancestor directory itself (covers `target/<profile>/deps/<exe>`,
///    cargo's layout for tests/benches) and a `target/<profile>/` sibling
///    (covers `tauri dev` nesting the desktop binary in a per-app target).
///
/// On failure returns an [`Error::Internal`] naming the
/// `CONCERTO_AGENT_HOST_BIN` override and listing every path tried, so
/// the renderer no longer surfaces a bare "Rpc" from an underlying
/// `io: No such file or directory`.
pub fn resolve_host_binary() -> Result<PathBuf> {
    let exe = std::env::current_exe().map_err(Error::Io)?;
    let base = exe
        .parent()
        .ok_or_else(|| {
            Error::Internal(
                "current_exe() has no parent directory; cannot locate agent-host".into(),
            )
        })?
        .to_path_buf();
    let override_val = std::env::var(HOST_BIN_ENV).ok();
    resolve_host_binary_in(override_val.as_deref(), &base, &host_bin_filename())
}

/// Backwards-compatible alias for [`resolve_host_binary`]. Task 22 named
/// this `default_host_binary`; Task 106 renamed it but keeps this thin
/// wrapper so the prior public call path (`boot.rs`, any out-of-tree
/// callers) does not break.
pub fn default_host_binary() -> Result<PathBuf> {
    resolve_host_binary()
}

/// Pure, testable core of the resolution order. Takes the override value
/// (`None` / `Some("")` → ignored), the base directory (the running
/// executable's directory), and the platform-specific binary file name.
/// Drives entirely off filesystem existence so unit tests can stand up a
/// `tempfile` tree with a fake executable.
///
/// See [`resolve_host_binary`] for the documented search order.
fn resolve_host_binary_in(
    override_val: Option<&str>,
    base: &Path,
    bin_filename: &str,
) -> Result<PathBuf> {
    // Record every candidate we probe so the failure message is
    // actionable (the motivation for this task).
    let mut tried: Vec<PathBuf> = Vec::new();

    // 1. Explicit override — highest precedence. Empty string is treated
    //    as unset (a common shell footgun: `CONCERTO_AGENT_HOST_BIN=`).
    if let Some(path) = override_val.filter(|p| !p.is_empty()) {
        let candidate = PathBuf::from(path);
        if candidate.exists() {
            return Ok(candidate);
        }
        tried.push(candidate);
    }

    // 2. Co-located beside the running executable (packaged install).
    let colocated = base.join(bin_filename);
    if colocated.exists() {
        return Ok(colocated);
    }
    tried.push(colocated);

    // 3. Bounded dev-layout walk. Only kicks in when co-location failed;
    //    never scans the filesystem. Two layouts are covered:
    //    - cargo tests/benches: the executable lives at
    //      `…/target/<profile>/deps/<exe>` while the helper is built to
    //      `…/target/<profile>/concerto-agent-host` — i.e. one ancestor up,
    //      probed directly.
    //    - `tauri dev` per-app target: the desktop binary nests under a
    //      separate `…/target/<profile>/` from the helper's workspace
    //      `target/<profile>/` — probed via the `target/<profile>/` sibling.
    //    `<profile>` is inferred from the exe dir's own leaf name (e.g.
    //    `debug` / `release`), matching how cargo names the level.
    let profile = base.file_name().and_then(|n| n.to_str());
    let mut dir = base;
    for _ in 0..TARGET_SEARCH_MAX_LEVELS {
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
        // (a) the ancestor directory itself.
        let direct = dir.join(bin_filename);
        if direct.exists() {
            return Ok(direct);
        }
        tried.push(direct);
        // (b) a `target/<profile>/` sibling under this ancestor.
        if let Some(profile) = profile {
            let sibling = dir.join("target").join(profile).join(bin_filename);
            if sibling.exists() {
                return Ok(sibling);
            }
            tried.push(sibling);
        }
    }

    let tried_list = tried
        .iter()
        .map(|p| format!("  - {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");
    Err(Error::Internal(format!(
        "could not locate the `{HOST_BIN_STEM}` binary. Set the `{HOST_BIN_ENV}` \
         environment variable to its absolute path to override resolution. Tried:\n{tried_list}"
    )))
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
    agent_kind: &AgentKind,
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
    // The Maestro runs claude headless `--print --input-format stream-json`,
    // which refuses a TTY — so it needs pipe-mode stdio. Every other kind
    // stays PTY (omit the flag → default).
    if *agent_kind == AgentKind::Maestro {
        cmd.arg("--io-mode").arg("pipe");
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

#[cfg(test)]
mod resolution_tests {
    //! Unit tests for the Task 106 resolution order. We exercise the pure
    //! [`resolve_host_binary_in`] directly so the search is driven by a
    //! `tempfile` tree with fake (empty-file) executables — resolution only
    //! consults filesystem existence, never execs anything — instead of the
    //! process's real `current_exe()`. Mirrors the `tempfile` style used by
    //! the integration tests in `crates/core/tests/agent_spawn.rs`.

    use super::{resolve_host_binary_in, HOST_BIN_ENV};
    use std::fs;
    use std::path::Path;

    const BIN: &str = "concerto-agent-host";

    /// Create an empty file at `path`, creating parent dirs as needed.
    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(path, b"").expect("write fake executable");
    }

    #[test]
    fn override_wins_over_colocated() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // A co-located binary also exists, but the override must win.
        let base = tmp.path().join("bin");
        touch(&base.join(BIN));
        let override_path = tmp.path().join("custom").join("agent-host-override");
        touch(&override_path);

        let resolved =
            resolve_host_binary_in(Some(override_path.to_str().unwrap()), &base, BIN).unwrap();
        assert_eq!(resolved, override_path);
    }

    #[test]
    fn empty_override_is_ignored() {
        // An empty `CONCERTO_AGENT_HOST_BIN=` must fall through to the
        // co-located case rather than resolving to "".
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().join("bin");
        let colocated = base.join(BIN);
        touch(&colocated);

        let resolved = resolve_host_binary_in(Some(""), &base, BIN).unwrap();
        assert_eq!(resolved, colocated);
    }

    #[test]
    fn colocated_found_when_no_override() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().join("target").join("debug");
        let colocated = base.join(BIN);
        touch(&colocated);

        let resolved = resolve_host_binary_in(None, &base, BIN).unwrap();
        assert_eq!(resolved, colocated);
    }

    #[test]
    fn target_sibling_found_when_not_colocated() {
        // cargo-dev / embedded layout: `tauri dev` nests the desktop binary
        // in a per-app target dir, while the helper lives in the shared
        // workspace `target/<profile>/`. We walk up from the exe dir to an
        // ancestor that owns a `target/<profile>/` sibling.
        //   <root>/target/debug/concerto-agent-host        <- helper (sibling)
        //   <root>/app/debug/<exe>                          <- running exe dir (base)
        // The exe dir's own name is the profile ("debug"), matching how a
        // `target/<profile>` layout names its leaf.
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let sibling = root.join("target").join("debug").join(BIN);
        touch(&sibling);
        // base is two levels below root, leaf named "debug" so profile matches.
        let base = root.join("app").join("debug");
        fs::create_dir_all(&base).expect("create base");
        // No co-located binary in `base`.

        let resolved = resolve_host_binary_in(None, &base, BIN).unwrap();
        assert_eq!(resolved, sibling);
    }

    #[test]
    fn deps_ancestor_found_when_not_colocated() {
        // cargo's tests/benches layout: the test binary runs from
        // `…/target/<profile>/deps/<exe>` while the helper is built to
        // `…/target/<profile>/concerto-agent-host`. The helper is one
        // ancestor up, probed directly (not via a `target/<profile>` join).
        let tmp = tempfile::tempdir().expect("tempdir");
        let profile_dir = tmp.path().join("target").join("debug");
        let helper = profile_dir.join(BIN);
        touch(&helper);
        let base = profile_dir.join("deps");
        fs::create_dir_all(&base).expect("create deps dir");

        let resolved = resolve_host_binary_in(None, &base, BIN).unwrap();
        assert_eq!(resolved, helper);
    }

    #[test]
    fn error_lists_tried_paths_and_env_override() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let base = tmp.path().join("target").join("debug");
        fs::create_dir_all(&base).expect("create base");
        let bogus_override = tmp.path().join("does-not-exist");

        let err = resolve_host_binary_in(Some(bogus_override.to_str().unwrap()), &base, BIN)
            .expect_err("resolution should fail when nothing exists");
        let msg = err.to_string();

        // Names the env override so the operator knows the escape hatch.
        assert!(
            msg.contains(HOST_BIN_ENV),
            "error should name the env override: {msg}"
        );
        // Lists the override path it tried…
        assert!(
            msg.contains(&bogus_override.display().to_string()),
            "error should list the tried override path: {msg}"
        );
        // …and the co-located path it tried.
        assert!(
            msg.contains(&base.join(BIN).display().to_string()),
            "error should list the tried co-located path: {msg}"
        );
    }
}
