//! `concerto-core` binary entry point.
//!
//! As of Task 11 this binary:
//!   1. Initializes logging (Task 05).
//!   2. Resolves a [`RuntimeConfig`] (data_dir + config_dir, env-overridable).
//!   3. Calls [`Runtime::start`], which acquires the single-instance lock,
//!      opens persistence (Task 08), and installs signal handlers.
//!   4. If another instance was already running, logs and exits 0.
//!   5. Otherwise blocks on [`Runtime::wait_for_shutdown`] until a signal
//!      fires (SIGTERM/SIGINT on Unix; Ctrl-C on Windows).
//!   6. Calls [`Runtime::stop`], which shuts down persistence and releases
//!      the lock.

use concerto_core::logging;
use concerto_core::runtime::{Runtime, RuntimeConfig, StartOutcome};
use concerto_error::Result;

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

    let config = RuntimeConfig::default_for_user()?;
    tracing::info!(
        data_dir = %config.data_dir.display(),
        config_dir = %config.config_dir.display(),
        "resolved runtime config"
    );

    let runtime = match Runtime::start(config).await? {
        StartOutcome::Started(r) => r,
        StartOutcome::AlreadyRunning { pid } => {
            // Per design/01 §3.3: exit 0 so launchd doesn't restart us.
            // The "another instance running" log line is already emitted
            // by Runtime::start; the higher level just acknowledges.
            tracing::info!(
                other_pid = pid,
                "exiting cleanly — another instance is live"
            );
            return Ok(());
        }
    };

    tracing::info!("concerto-core ready");

    runtime.wait_for_shutdown().await?;
    tracing::info!("shutdown signal observed");

    runtime.stop().await?;
    tracing::info!("concerto-core stopped");
    Ok(())
}
