//! Embedded-Core mode: boot `concerto-core` inside the desktop process.
//!
//! Compiled only under the `embedded-core` feature. Picks a launch mode
//! from the environment, resolves a [`RuntimeConfig`], and boots Core on
//! a dedicated tokio runtime. Core's PID single-instance lock is the
//! coexistence guard: if a daemon already holds it, `boot::start` returns
//! `AlreadyRunning` and we fall back to dialing the live daemon.

// Mode, resolve_mode, and scratch_config are all consumed by Task 4's
// start() function; suppress dead_code until that wiring lands.
#![allow(dead_code)]

use std::path::PathBuf;
use std::time::Duration;

use concerto_core::runtime::RuntimeConfig;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_when_flag_or_env_zero() {
        assert_eq!(resolve_mode(&["--external".into()], None, None), Mode::External);
        assert_eq!(resolve_mode(&[], Some("0"), None), Mode::External);
    }

    #[test]
    fn scratch_when_home_set() {
        let m = resolve_mode(&[], None, Some("/tmp/scratch"));
        assert_eq!(m, Mode::EmbeddedScratch { home: "/tmp/scratch".into() });
    }

    #[test]
    fn real_by_default() {
        assert_eq!(resolve_mode(&[], None, None), Mode::EmbeddedReal);
        assert_eq!(resolve_mode(&[], Some("1"), None), Mode::EmbeddedReal);
    }

    #[test]
    fn scratch_config_splits_home() {
        let c = scratch_config(std::path::Path::new("/tmp/s"));
        assert_eq!(c.data_dir, std::path::PathBuf::from("/tmp/s"));
        assert_eq!(c.config_dir, std::path::PathBuf::from("/tmp/s/.concerto"));
    }
}
