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
pub mod audit;
pub mod boot;
pub mod conn_transport;
pub mod connect_bridge;
pub mod error_map;
pub mod handlers;
pub mod llm;
pub mod log_fields;
pub mod log_filter;
pub mod logging;
// Task 401: the Maestro Agent subsystem (cluster-M root) — the in-process
// `concerto-maestro-mcp` MCP server + the FROZEN 16-tool registry. `cfg(unix)`
// because it sits over the `cfg(unix)` agent supervisor (mirrors
// `agent_supervisor`/`scheduler`/`suggestions`); the Windows lane omits it.
#[cfg(unix)]
pub mod maestro;
pub mod notifications;
pub mod pid_file;
pub mod repo_manager;
pub mod runtime;
#[cfg(unix)]
pub mod scheduler;
pub mod security;
pub mod settings;
pub mod signals;
pub mod skills;
#[cfg(unix)]
pub mod suggestions;
pub mod supervisor;
pub mod vcs;
pub mod workspace_manager;
