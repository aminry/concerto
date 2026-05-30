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
//!
//! The boot orchestration itself now lives in [`concerto_core::boot`] so
//! both this daemon binary and the embedded desktop path can share it.

use concerto_core::boot::{self, BootOutcome};
use concerto_core::logging;
use concerto_core::runtime::RuntimeConfig;
use concerto_error::Result;

fn main() -> std::process::ExitCode {
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

async fn run() -> Result<()> {
    let config = RuntimeConfig::default_for_user()?;
    match boot::start(config).await? {
        BootOutcome::Started(core) => core.run_until_shutdown().await,
        // Per design/01 §3.3: exit 0 so launchd doesn't restart us.
        BootOutcome::AlreadyRunning { .. } => Ok(()),
    }
}
