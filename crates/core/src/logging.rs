//! Concerto Core logging setup.
//!
//! Per design/00 §6.1 / §7.4: `tracing` + `tracing-subscriber` write a
//! human-readable log to stderr and a JSON log to a daily-rotating file
//! at `$CONCERTO_DATA_DIR/logs/core.<YYYY-MM-DD>.log` (14-day retention).
//! Filters are configured via `RUST_LOG` (default `info,concerto=debug`).
//!
//! ## Span field convention
//!
//! Every public function that takes an ID parameter MUST wrap its body
//! in the corresponding span via the [`workspace_span`](crate::workspace_span!),
//! [`workarea_span`](crate::workarea_span!), [`session_span`](crate::session_span!),
//! or [`device_span`](crate::device_span!) macro. The JSON file layer
//! captures span fields automatically, so downstream log readers can
//! filter events by `workspace_id` / `session_id` without parsing the
//! human format. Rust has no real lint for this; reviewers enforce it.
//!
//! ## Secrets redaction
//!
//! The file layer is wrapped by [`crate::log_filter::SecretsFilter`],
//! which replaces field values whose names appear in
//! [`crate::log_filter::REDACTED_FIELDS`] with `"<redacted>"` before
//! they are serialized. Adding a name to that list is a one-line change;
//! removing a name is forbidden.
//!
//! ## Lifetime
//!
//! The binary holds the [`LogGuard`] returned by [`init`] for the
//! lifetime of the process. Dropping it tears down both the default
//! dispatcher and the non-blocking writer worker — every subsequent
//! event is lost.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use concerto_error::{Error, Result};
use tracing::dispatcher::DefaultGuard;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::log_filter::SecretsFilter;

/// Retention: keep this many rotated log files. Older files are deleted
/// by `tracing-appender` automatically on rotation.
const MAX_LOG_FILES: usize = 14;

/// Returned by [`init`]. Holds both the tracing dispatcher guard and
/// the `non_blocking` writer's worker guard; dropping either silences
/// all further output.
///
/// Bind in `main()` and never re-assign.
pub struct LogGuard {
    _default: DefaultGuard,
    _worker: WorkerGuard,
}

/// Initialize the runtime tracing subscriber.
///
/// Resolves the log directory from `$CONCERTO_DATA_DIR/logs` (falling
/// back to `~/concerto/logs`), then delegates to
/// [`init_with_log_dir`]. Returns a guard the caller MUST hold for the
/// lifetime of the program.
pub fn init() -> Result<LogGuard> {
    let log_dir = log_dir()?;
    init_with_log_dir(&log_dir)
}

/// Initialize tracing with an explicit log directory. Used by [`init`]
/// in production and by integration tests that need a tempdir.
pub fn init_with_log_dir(log_dir: &Path) -> Result<LogGuard> {
    std::fs::create_dir_all(log_dir)?;

    // Build the rolling daily appender. Filename schema is
    // `core.YYYY-MM-DD.log` — tracing-appender always inserts a `.`
    // between prefix, date, and suffix; rotation = daily; the latest
    // 14 files are retained on disk per design/00 §7.4.
    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("core")
        .filename_suffix("log")
        .max_log_files(MAX_LOG_FILES)
        .build(log_dir)
        .map_err(|e| Error::Internal(format!("rolling file appender: {e}")))?;

    // Wrap in non_blocking so the application thread never stalls on
    // disk I/O. The returned WorkerGuard MUST be held for the program's
    // lifetime; we bundle it into [`LogGuard`] so callers can't drop
    // it accidentally.
    let (non_blocking, worker_guard) = tracing_appender::non_blocking(file_appender);

    // File layer = JSON, with secrets redaction applied per design/00
    // §7.4. SecretsFilter implements `Layer` directly so it both
    // formats and scrubs in one pass — see crate::log_filter for the
    // rationale.
    let file_layer = SecretsFilter::json(non_blocking);

    // Console layer = compact human format, routed through
    // SecretsFilter so blocklisted field names never reach stderr
    // either (launchd, journald, etc. capture stderr in production).
    // ANSI iff stderr is a TTY.
    let ansi = std::io::stderr().is_terminal();
    let console_layer = SecretsFilter::compact_human(std::io::stderr(), ansi);

    let filter = build_filter(std::env::var("RUST_LOG").ok())?;

    let guard = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(console_layer)
        .set_default();

    Ok(LogGuard {
        _default: guard,
        _worker: worker_guard,
    })
}

/// Initialize a no-op subscriber for tests.
///
/// Safe to call from many tests in parallel — uses `try_init`, so the
/// second-and-later callers see a benign error which is ignored.
pub fn init_for_tests() {
    let _ = tracing_subscriber::fmt::try_init();
}

/// Resolve the on-disk log directory.
///
/// Honours `CONCERTO_DATA_DIR` (set by `RuntimeConfig` and the smoke
/// gate) and falls back to `~/concerto/`.
fn log_dir() -> Result<PathBuf> {
    let data_dir = std::env::var("CONCERTO_DATA_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let base = match data_dir {
        Some(p) => p,
        None => home::home_dir()
            .ok_or_else(|| Error::Internal("home::home_dir() returned None".into()))?
            .join("concerto"),
    };
    Ok(base.join("logs"))
}

/// Parse a `RUST_LOG`-style spec into a [`Targets`] filter.
///
/// Takes the raw env-var value as a parameter so unit tests don't race
/// each other on the global `RUST_LOG` (which the previous Task 05
/// implementation did, intermittently). `None` selects the default
/// `info,concerto=debug`.
///
/// The grammar is the same shape as `EnvFilter` understands but
/// intentionally simpler: comma-separated entries of either `<level>`
/// (the default) or `<target>=<level>`. Unknown levels produce a typed
/// error instead of being silently ignored.
fn build_filter(raw: Option<String>) -> Result<Targets> {
    let raw = raw.unwrap_or_else(|| "info,concerto=debug".to_string());
    let mut targets = Targets::new();
    let mut default_seen = false;
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if let Some((target, level)) = entry.split_once('=') {
            let target = target.trim();
            let level = level.trim();
            let lvl = LevelFilter::from_str(level).map_err(|e| {
                Error::Internal(format!(
                    "RUST_LOG: bad level '{level}' for target '{target}': {e}"
                ))
            })?;
            targets = targets.with_target(target.to_string(), lvl);
        } else {
            let lvl = LevelFilter::from_str(entry)
                .map_err(|e| Error::Internal(format!("RUST_LOG: bad level '{entry}': {e}")))?;
            targets = targets.with_default(lvl);
            default_seen = true;
        }
    }
    if !default_seen {
        targets = targets.with_default(LevelFilter::INFO);
    }
    Ok(targets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_filter_when_env_unset() {
        let f = build_filter(None).expect("default filter parses");
        drop(f);
    }

    #[test]
    fn parses_target_overrides() {
        let f = build_filter(Some("warn,concerto=trace,foo::bar=error".into()))
            .expect("parses target overrides");
        drop(f);
    }

    #[test]
    fn rejects_invalid_level() {
        let err = build_filter(Some("notalevel".into())).expect_err("invalid level is an error");
        assert_eq!(err.wire_code(), "internal");
    }

    #[test]
    fn empty_entries_ignored() {
        // Commas without values shouldn't break parsing.
        let f = build_filter(Some(",,info,,".into())).expect("trims empty entries");
        drop(f);
    }
}
