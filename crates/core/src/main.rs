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

use std::sync::Arc;

use concerto_core::api_server::{ApiServerActor, ApiServerConfig};
use concerto_core::logging;
use concerto_core::repo_manager::{RepoManagerActor, RepoManagerConfig};
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

    let socket_path = config.config_dir.join("core.sock");
    let repos_root = config.data_dir.join("repos");
    let mut runtime = match Runtime::start(config).await? {
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

    // Task 18: spawn the Repository Manager first so its handle can be
    // captured by the gRPC server's factory closure below. The actor's
    // `run` loop just idles on shutdown; the handle is the meaningful
    // surface and lives in `RepoManagerActor::new`.
    let persistence = runtime
        .supervisor()
        .expect("supervisor present at boot")
        .persistence();
    let repo_actor = RepoManagerActor::new(Arc::clone(&persistence), repos_root.clone());
    let repo_handle = repo_actor.handle();
    // The actor instance built above is consumed by the factory; the
    // handle clone above is what the gRPC service holds.
    drop(repo_actor);
    let repo_factory_persistence = Arc::clone(&persistence);
    let repo_factory_root = repos_root.clone();
    runtime
        .supervisor_mut()
        .expect("supervisor present at boot")
        .spawn::<RepoManagerActor, _>(
            move || {
                RepoManagerActor::new(
                    Arc::clone(&repo_factory_persistence),
                    repo_factory_root.clone(),
                )
            },
            RepoManagerConfig {
                repos_root: repos_root.clone(),
            },
        )
        .await?;

    // Task 13: spawn the gRPC server as the next supervised actor.
    // Handles captured by the factory closure are cheap `Arc::clone`s
    // (plus a single `RepoManager::clone` for the repositories service),
    // so a restart constructs a fresh `ApiServerActor` without re-reading
    // the wall clock or rebuilding the supervisor view.
    let started_at = runtime.started_at();
    let supervisor_view = runtime
        .supervisor()
        .expect("supervisor present at boot")
        .view();
    let factory_started_at = Arc::clone(&started_at);
    let factory_view = supervisor_view.clone();
    let factory_repo_handle = repo_handle.clone();
    runtime
        .supervisor_mut()
        .expect("supervisor present at boot")
        .spawn::<ApiServerActor, _>(
            move || {
                ApiServerActor::with_repo_manager(
                    Arc::clone(&factory_started_at),
                    factory_view.clone(),
                    factory_repo_handle.clone(),
                )
            },
            ApiServerConfig { socket_path },
        )
        .await?;

    tracing::info!("concerto-core ready");

    runtime.wait_for_shutdown().await?;
    tracing::info!("shutdown signal observed");

    runtime.stop().await?;
    tracing::info!("concerto-core stopped");
    Ok(())
}
