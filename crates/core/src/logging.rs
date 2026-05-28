//! Concerto Core logging setup.
//!
//! Per design/00 §6.1 / §7.4: `tracing` + `tracing-subscriber` write a
//! human-readable log to a daily-rotating file at
//! `~/concerto/logs/core.<YYYY-MM-DD>.log` plus stderr. Filters are
//! configured via `RUST_LOG` (default `info,concerto=debug`).
//!
//! The binary holds the [`tracing::dispatcher::DefaultGuard`] returned by
//! [`init`] for the lifetime of the process; dropping it would silence
//! every subsequent event.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::str::FromStr;

use concerto_error::{Error, Result};
use tracing::dispatcher::DefaultGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Initialize the runtime tracing subscriber.
///
/// Returns a guard that the caller MUST hold for the lifetime of the
/// program. Dropping it tears down the default dispatcher.
pub fn init() -> Result<DefaultGuard> {
    let log_dir = log_dir()?;
    std::fs::create_dir_all(&log_dir)?;

    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("core")
        .filename_suffix("log")
        .build(&log_dir)
        .map_err(|e| Error::Internal(format!("rolling file appender: {e}")))?;

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file_appender)
        .with_ansi(false);

    let console_layer = tracing_subscriber::fmt::layer()
        .compact()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal());

    let filter = build_filter()?;

    let guard = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(console_layer)
        .set_default();

    Ok(guard)
}

/// Initialize a no-op subscriber for tests.
///
/// Safe to call from many tests in parallel — uses `try_init`, so the
/// second-and-later callers see a benign error which is ignored.
pub fn init_for_tests() {
    let _ = tracing_subscriber::fmt::try_init();
}

fn log_dir() -> Result<PathBuf> {
    let home =
        home::home_dir().ok_or_else(|| Error::Internal("home::home_dir() returned None".into()))?;
    Ok(home.join("concerto").join("logs"))
}

/// Parse `RUST_LOG` into a [`Targets`] filter.
///
/// The grammar is the same shape as `EnvFilter` understands but
/// intentionally simpler: comma-separated entries of either `<level>` (the
/// default) or `<target>=<level>`. Unknown levels produce a typed error
/// instead of being silently ignored.
fn build_filter() -> Result<Targets> {
    let raw = std::env::var("RUST_LOG").unwrap_or_else(|_| "info,concerto=debug".to_string());
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
        // SAFETY: tests in the same process share env; remove RUST_LOG to
        // exercise the default branch. The narrow scope means we don't
        // race with concurrent threads in this binary.
        let prev = std::env::var("RUST_LOG").ok();
        std::env::remove_var("RUST_LOG");
        let f = build_filter().expect("default filter parses");
        // Targets has no public introspection; just assert it constructs.
        drop(f);
        if let Some(p) = prev {
            std::env::set_var("RUST_LOG", p);
        }
    }

    #[test]
    fn parses_target_overrides() {
        std::env::set_var("RUST_LOG", "warn,concerto=trace,foo::bar=error");
        let f = build_filter().expect("parses target overrides");
        drop(f);
        std::env::remove_var("RUST_LOG");
    }

    #[test]
    fn rejects_invalid_level() {
        std::env::set_var("RUST_LOG", "notalevel");
        let err = build_filter().expect_err("invalid level is an error");
        assert_eq!(err.wire_code(), "internal");
        std::env::remove_var("RUST_LOG");
    }
}
