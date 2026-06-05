//! Subcommand modules for `smoke-client`.
//!
//! Each module exposes a single `pub async fn run(...)` that the
//! top-level dispatcher in `main.rs` calls. Subcommands print the
//! resource id (UUID string) — or, for `stream-session-io`, the
//! agent's stdout bytes — to stdout and exit 0 on success. Errors are
//! reported as `smoke-client: <reason>` on stderr with exit code 1.
//!
//! Every gRPC call is wrapped in a 30 s `tokio::time::timeout` so a
//! misbehaving Core surfaces in the smoke gate quickly. The streaming
//! `stream-session-io` subcommand takes its timeout from the caller.

pub mod add_project;
pub mod add_repo;
pub mod caps;
pub mod clone;
pub mod create_loop;
pub mod files_transfer_probe;
pub mod list_audit;
pub mod list_loops;
pub mod list_mcp;
pub mod list_skills;
pub mod new_workarea;
pub mod new_workspace;
pub mod send_message;
pub mod set_cones;
pub mod set_perm_mode;
pub mod start_session;
pub mod stop_session;
pub mod stream_session_io;
pub mod streams_replay_probe;

use std::time::Duration;

/// Default per-RPC deadline (matches the task spec's "30s" budget).
pub const RPC_TIMEOUT: Duration = Duration::from_secs(30);
