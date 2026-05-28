//! Concerto Core daemon library.
//!
//! Hosts the long-lived runtime. Subsystems hang off of it as separate
//! modules.
//!
//! As of Task 11 the runtime skeleton owns:
//!
//! - [`runtime::Runtime`] — the boot/shutdown orchestrator and
//!   single-instance guard.
//! - [`pid_file::PidFile`] — the RAII handle for the advisory lock at
//!   `<config_dir>/core.pid`.
//! - [`signals`] — SIGTERM/SIGINT/SIGHUP plumbing (Unix);
//!   `tokio::signal::ctrl_c` on Windows.
//!
//! Actor supervision (the typed tokio-task hierarchy from `design/01
//! §3.2`) arrives in Task 12.

#[cfg(unix)]
pub mod agent_supervisor;
pub mod api_server;
pub mod error_map;
pub mod handlers;
pub mod log_fields;
pub mod log_filter;
pub mod logging;
pub mod pid_file;
pub mod repo_manager;
pub mod runtime;
pub mod security;
pub mod signals;
pub mod supervisor;
pub mod workspace_manager;
