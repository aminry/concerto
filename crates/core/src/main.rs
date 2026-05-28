//! `concerto-core` binary entry point.
//!
//! Real runtime supervision is filled in by Task 11. Today this binary:
//!   1. Initializes logging (Task 04).
//!   2. Opens the SQLite persistence layer (Task 08), creating the file +
//!      WAL/foreign-keys pragmas + running embedded migrations.
//!   3. Logs that it reached steady state.
//!   4. Waits for SIGTERM / Ctrl-C and shuts down cleanly.

use std::path::PathBuf;

use concerto_core::logging;
use concerto_error::{Error, Result};
use concerto_persist::{Persistence, PersistenceConfig};

fn main() -> std::process::ExitCode {
    // Logging is sync; install it before we hand control to tokio so the
    // runtime's own setup messages land in the log.
    let _log_guard = match logging::init() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("failed to initialize logging: {e}");
            return std::process::ExitCode::from(1);
        }
    };

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(error = %e, "failed to build tokio runtime");
            return std::process::ExitCode::from(1);
        }
    };

    match rt.block_on(run()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "concerto-core exited with error");
            std::process::ExitCode::from(1)
        }
    }
}

/// Async entry point. Errors are returned to `main`, which logs and exits
/// with a non-zero code — no `panic!`s on the startup path.
async fn run() -> Result<()> {
    tracing::info!("concerto-core starting");

    let config = persistence_config()?;
    tracing::info!(
        db_path = %config.db_path.display(),
        max_readers = config.max_readers,
        "opening persistence"
    );

    let persist = Persistence::open(config).await.map_err(|e| {
        tracing::error!(error = %e, "failed to open persistence");
        e
    })?;
    tracing::info!("persistence ready");

    wait_for_shutdown_signal().await?;
    tracing::info!("shutdown signal received; closing persistence");

    persist.shutdown().await?;
    tracing::info!("concerto-core stopped");
    Ok(())
}

/// Resolve the [`PersistenceConfig`] from environment + defaults.
///
/// `CONCERTO_DB_PATH`, if set and non-empty, overrides `db_path`. This is
/// the seam tests and smoke gate use to avoid touching `$HOME/concerto/`.
fn persistence_config() -> Result<PersistenceConfig> {
    let mut config = PersistenceConfig::default_for_user()?;
    if let Ok(p) = std::env::var("CONCERTO_DB_PATH") {
        if !p.is_empty() {
            config.db_path = PathBuf::from(p);
        }
    }
    Ok(config)
}

/// Wait for SIGTERM (Unix) or Ctrl-C, whichever arrives first.
///
/// Returns `Ok(())` when a signal arrives, or an error if the signal
/// installer itself fails.
async fn wait_for_shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())
            .map_err(|e| Error::Internal(format!("install SIGTERM handler: {e}")))?;
        let mut sigint = signal(SignalKind::interrupt())
            .map_err(|e| Error::Internal(format!("install SIGINT handler: {e}")))?;
        tokio::select! {
            _ = sigterm.recv() => Ok(()),
            _ = sigint.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .map_err(|e| Error::Internal(format!("ctrl_c handler: {e}")))
    }
}
