//! Embedded-Core mode: boot `concerto-core` inside the desktop process.
//!
//! Compiled only under the `embedded-core` feature. Picks a launch mode
//! from the environment, resolves a [`RuntimeConfig`], and boots Core on
//! the host (Tauri) Tokio runtime via `tokio::spawn`. Core's PID
//! single-instance lock is the coexistence guard: if a daemon already
//! holds it, `boot::start` returns `AlreadyRunning` and we fall back to
//! dialing the live daemon.
//!
//! V0.1 tradeoff: Core's run loop + supervised actors share Tauri's
//! global runtime with the IPC/command machinery rather than running on
//! an isolated runtime. Fine for a single in-process Core; if Core's
//! workload grows, a dedicated runtime may be warranted.

use std::path::PathBuf;
use std::time::Duration;

use concerto_core::runtime::RuntimeConfig;
use tokio_util::sync::CancellationToken;

/// How this launch should obtain its Core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Boot Core in-process against real data (`~/concerto`, `~/.concerto`).
    EmbeddedReal,
    /// Boot Core in-process against an isolated scratch root.
    EmbeddedScratch { home: PathBuf },
    /// Do not embed — dial an externally running daemon.
    External,
}

/// Resolve the launch mode from env vars / flags.
///
/// Precedence: `CONCERTO_EMBEDDED=0` (or `--external`) → External; an
/// explicit `CONCERTO_HOME` → EmbeddedScratch; otherwise (or
/// `CONCERTO_EMBEDDED=1`) → EmbeddedReal.
///
/// Note: `--embedded-scratch` without a `CONCERTO_HOME` falls through to
/// `EmbeddedReal` rather than inventing a temp location — the caller is
/// expected to set the var. No error is raised.
pub fn resolve_mode(args: &[String], env_embedded: Option<&str>, env_home: Option<&str>) -> Mode {
    if args.iter().any(|a| a == "--external") || env_embedded == Some("0") {
        return Mode::External;
    }
    if let Some(home) = env_home.filter(|h| !h.is_empty()) {
        return Mode::EmbeddedScratch {
            home: PathBuf::from(home),
        };
    }
    if args.iter().any(|a| a == "--embedded-scratch") {
        // Scratch with no explicit home: caller must also set CONCERTO_HOME;
        // treated as real if absent to avoid a surprise temp location.
        return Mode::EmbeddedReal;
    }
    Mode::EmbeddedReal
}

/// Build a `RuntimeConfig` for a scratch home: `<home>` for data,
/// `<home>/.concerto` for config (mirrors the smoke-gate convention).
pub fn scratch_config(home: &std::path::Path) -> RuntimeConfig {
    RuntimeConfig {
        data_dir: home.to_path_buf(),
        config_dir: home.join(".concerto"),
        shutdown_grace: Duration::from_secs(5),
    }
}

/// Handle stored in Tauri state so the window-close path can shut Core
/// down. Present only when Core was booted in-process.
pub struct EmbeddedHandle {
    pub shutdown: CancellationToken,
}

/// Boot Core for the resolved mode. On success installs the client
/// socket override and spawns Core's run-until-shutdown loop on the
/// current Tokio runtime, returning a handle whose token triggers
/// teardown. Returns `None` for External mode, for the
/// `AlreadyRunning` fallback (a daemon already holds the PID lock — we
/// dial it instead), or on boot error.
pub async fn start(mode: Mode) -> Option<EmbeddedHandle> {
    use concerto_core::boot::{self, BootOutcome};

    let config = match &mode {
        Mode::External => return None,
        Mode::EmbeddedScratch { home } => scratch_config(home),
        Mode::EmbeddedReal => match RuntimeConfig::default_for_user() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "embedded: failed to resolve runtime config");
                return None;
            }
        },
    };

    // Core's persistence + PID lock require these dirs to exist.
    if let Err(e) = std::fs::create_dir_all(&config.data_dir) {
        tracing::error!(error = %e, dir = %config.data_dir.display(), "embedded: cannot create data dir");
        return None;
    }
    if let Err(e) = std::fs::create_dir_all(&config.config_dir) {
        tracing::error!(error = %e, dir = %config.config_dir.display(), "embedded: cannot create config dir");
        return None;
    }

    match boot::start(config).await {
        Ok(BootOutcome::Started(core)) => {
            crate::core_client::set_socket_override(core.socket_path().to_path_buf());
            let token = core.shutdown_token();
            tokio::spawn(async move {
                if let Err(e) = core.run_until_shutdown().await {
                    tracing::error!(error = %e, "embedded core shutdown error");
                }
            });
            tracing::info!("embedded core ready");
            Some(EmbeddedHandle { shutdown: token })
        }
        Ok(BootOutcome::AlreadyRunning { pid }) => {
            tracing::warn!(
                daemon_pid = pid,
                "daemon already running; dialing it instead of embedding"
            );
            None
        }
        Err(e) => {
            tracing::error!(error = %e, "embedded core failed to boot; falling back to external");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_when_flag_or_env_zero() {
        assert_eq!(
            resolve_mode(&["--external".into()], None, None),
            Mode::External
        );
        assert_eq!(resolve_mode(&[], Some("0"), None), Mode::External);
    }

    #[test]
    fn scratch_when_home_set() {
        let m = resolve_mode(&[], None, Some("/tmp/scratch"));
        assert_eq!(
            m,
            Mode::EmbeddedScratch {
                home: "/tmp/scratch".into()
            }
        );
    }

    #[test]
    fn real_by_default() {
        assert_eq!(resolve_mode(&[], None, None), Mode::EmbeddedReal);
        assert_eq!(resolve_mode(&[], Some("1"), None), Mode::EmbeddedReal);
    }

    #[test]
    fn scratch_flag_without_home_falls_through_to_real() {
        assert_eq!(
            resolve_mode(&["--embedded-scratch".into()], None, None),
            Mode::EmbeddedReal
        );
    }

    #[test]
    fn scratch_config_splits_home() {
        let c = scratch_config(std::path::Path::new("/tmp/s"));
        assert_eq!(c.data_dir, std::path::PathBuf::from("/tmp/s"));
        assert_eq!(c.config_dir, std::path::PathBuf::from("/tmp/s/.concerto"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn start_scratch_boots_and_shuts_down() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        let mode = Mode::EmbeddedScratch { home: home.clone() };

        let handle = super::start(mode)
            .await
            .expect("embedded scratch should boot");

        // We intentionally do NOT assert `default_socket_path()` equals the
        // scratch socket here. `set_socket_override` writes a process-global
        // `OnceLock` (set-once), and the Task 2 test in `core_client.rs`
        // (`default_socket_path_defaults_then_honors_override`) also writes
        // it. Under libtest's parallel runner, whichever test sets the cell
        // first wins, so asserting the exact override value would race and
        // flake. The real proof here is that `start` returned `Some` — Core
        // booted, the override was installed, and the run loop spawned.

        // Cancelling the token must actually tear Core down. Core's PID lock
        // (`<config_dir>/core.pid`) is removed when `Runtime::stop` drops the
        // `PidFile`, so its disappearance is a concrete teardown signal —
        // tighter than a blind sleep, which would pass even if the run loop
        // hung. The lock should exist while Core runs, then vanish.
        let pid_lock = home.join(".concerto").join("core.pid");
        assert!(
            pid_lock.exists(),
            "PID lock should exist while embedded Core runs"
        );
        handle.shutdown.cancel();
        let torn_down = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while pid_lock.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(
            torn_down.is_ok(),
            "embedded Core should release its PID lock after cancel"
        );
    }
}
