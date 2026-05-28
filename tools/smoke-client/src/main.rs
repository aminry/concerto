//! `smoke-client` — the canonical end-to-end client used by
//! `scripts/smoke.sh`.
//!
//! Task 15 shipped a single-call client (`--socket <path>` →
//! `Runtime.GetServerCapabilities`). Task 27 promotes it to a clap
//! subcommand dispatcher covering the Phase 2 happy path:
//!
//!   * `caps` — `Runtime.GetServerCapabilities`.
//!   * `add-project` — direct sqlx insert (V0.1 has no Projects.Create
//!     RPC; see `cmd/add_project.rs`).
//!   * `add-repo` — `Repositories.AddRepository`.
//!   * `clone` — `Repositories.Clone` (streaming, drained to EOS).
//!   * `new-workspace` — `Workspaces.CreateWorkspace`.
//!   * `new-workarea` — `Workareas.CreateWorkarea`.
//!   * `start-session` — `Sessions.CreateSession`.
//!   * `stream-session-io` — `Streams.Subscribe(session.io.<sid>)` +
//!     `Streams.Subscribe(session.events.<sid>)` raced with a
//!     `--timeout`. Stdout is the agent's stdout bytes.
//!   * `stop-session` — `Sessions.StopSession`.
//!
//! Every gRPC call has a 30 s `tokio::time::timeout`. Output of each
//! id-producing subcommand is the resource UUID on stdout (no trailing
//! decoration). Errors → stderr with `smoke-client: <reason>`, exit 1.
//!
//! The connect pattern is shared via [`connect::connect_to_socket`];
//! see `tasks/13-grpc-uds-server.md` for the original lock.

mod cmd;
mod connect;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Subcommand-based smoke client used by `scripts/smoke.sh`.
#[derive(Debug, Parser)]
#[command(
    name = "smoke-client",
    version,
    about = "End-to-end RPC client for the Concerto Core smoke gate."
)]
struct Cli {
    /// Path to the Core's UDS socket. Required by every subcommand
    /// except `add-project` (which talks to SQLite directly).
    ///
    /// Default mirrors `RuntimeConfig::default_for_user` —
    /// `$CONCERTO_CONFIG_DIR/core.sock` or `~/.concerto/core.sock`.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Call `Runtime.GetServerCapabilities` and print one-line JSON.
    Caps,
    /// Insert a `projects` row directly via sqlx (V0.1 workaround —
    /// no `Projects.CreateProject` RPC exists yet). Prints the id.
    AddProject {
        /// Human-readable project name.
        #[arg(long)]
        name: String,
    },
    /// Call `Repositories.AddRepository` and print the new repo id.
    AddRepo {
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        url: String,
    },
    /// Call `Repositories.Clone` and drain the progress stream.
    Clone {
        #[arg(long)]
        repo_id: String,
    },
    /// Call `Workspaces.CreateWorkspace` and print the workspace id.
    NewWorkspace {
        #[arg(long)]
        project_id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        repo_id: String,
    },
    /// Call `Workareas.CreateWorkarea` and print the workarea id.
    NewWorkarea {
        #[arg(long)]
        workspace_id: String,
    },
    /// Call `Sessions.CreateSession` and print the session id.
    StartSession {
        #[arg(long)]
        workarea_id: String,
        #[arg(long)]
        agent_kind: String,
    },
    /// Subscribe to `session.io.<sid>` (+ events) and stream stdout
    /// bytes until `AgentExited` or the timeout fires.
    StreamSessionIo {
        #[arg(long)]
        session_id: String,
        /// Wall-clock budget, seconds.
        #[arg(long, default_value_t = 10)]
        timeout: u64,
    },
    /// Call `Sessions.StopSession`.
    StopSession {
        #[arg(long)]
        session_id: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("smoke-client: failed to build tokio runtime: {e}");
            return ExitCode::from(1);
        }
    };

    let result = rt.block_on(dispatch(cli));

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("smoke-client: {e}");
            ExitCode::from(1)
        }
    }
}

/// Resolve `--socket` for subcommands that need a Core connection.
/// `add-project` is the lone exception (it touches SQLite directly).
fn require_socket(socket: Option<PathBuf>) -> Result<PathBuf, String> {
    socket.ok_or_else(|| "missing --socket <path> (required for gRPC subcommands)".to_string())
}

async fn dispatch(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Caps => {
            let socket = require_socket(cli.socket)?;
            cmd::caps::run(&socket).await
        }
        Command::AddProject { name } => cmd::add_project::run(&name).await,
        Command::AddRepo { project_id, url } => {
            let socket = require_socket(cli.socket)?;
            cmd::add_repo::run(&socket, &project_id, &url).await
        }
        Command::Clone { repo_id } => {
            let socket = require_socket(cli.socket)?;
            cmd::clone::run(&socket, &repo_id).await
        }
        Command::NewWorkspace {
            project_id,
            name,
            repo_id,
        } => {
            let socket = require_socket(cli.socket)?;
            cmd::new_workspace::run(&socket, &project_id, &name, &repo_id).await
        }
        Command::NewWorkarea { workspace_id } => {
            let socket = require_socket(cli.socket)?;
            cmd::new_workarea::run(&socket, &workspace_id).await
        }
        Command::StartSession {
            workarea_id,
            agent_kind,
        } => {
            let socket = require_socket(cli.socket)?;
            cmd::start_session::run(&socket, &workarea_id, &agent_kind).await
        }
        Command::StreamSessionIo {
            session_id,
            timeout,
        } => {
            let socket = require_socket(cli.socket)?;
            cmd::stream_session_io::run(&socket, &session_id, timeout).await
        }
        Command::StopSession { session_id } => {
            let socket = require_socket(cli.socket)?;
            cmd::stop_session::run(&socket, &session_id).await
        }
    }
}
