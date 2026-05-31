//! `concerto` — the command-line client for a local Concerto Core.
//!
//! Wraps the Core's gRPC API over its Unix-domain socket. V1.0 Task 109
//! ships the read-only skeleton:
//!
//!   * `concerto status`              — `Runtime.GetServerCapabilities` + `GetStatus`.
//!   * `concerto workspace ls`        — `Workspaces.ListWorkspaces`.
//!   * `concerto session ls`          — `Sessions.ListSessions`.
//!
//! Global flags:
//!
//!   * `--socket <path>` / `$CONCERTO_SOCKET` — point at a non-default Core
//!     (mirrors the desktop's socket-override convention). Precedence:
//!     `--socket` > `$CONCERTO_SOCKET` > `<HOME>/.concerto/core.sock`.
//!   * `--json` — emit a single machine-readable JSON document instead of a
//!     human table.
//!
//! The UDS dial + default-socket derivation live in the self-contained
//! [`client`] module so the later `concerto pair` (Task 713) and
//! `concerto backup` (Task 111) subcommands reuse them within this crate.
//! Output rendering is separated from the RPC calls in [`commands`] so
//! `--json` is a thin switch.

mod client;
mod commands;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use commands::{CommandError, OutputFormat};

/// Top-level `concerto` CLI.
#[derive(Debug, Parser)]
#[command(
    name = "concerto",
    version,
    about = "Command-line client for a local Concerto Core (over UDS gRPC).",
    long_about = None,
)]
struct Cli {
    /// Path to the Core's UDS socket. Overrides `$CONCERTO_SOCKET`. When
    /// unset, falls back to `$CONCERTO_SOCKET` then `~/.concerto/core.sock`.
    #[arg(long, global = true, value_name = "PATH")]
    socket: Option<PathBuf>,

    /// Emit machine-readable JSON instead of a human-readable table.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show the Core's version, uptime, transport, and advertised services.
    Status,
    /// Workspace commands.
    #[command(subcommand)]
    Workspace(WorkspaceCommand),
    /// Session commands.
    #[command(subcommand)]
    Session(SessionCommand),
}

#[derive(Debug, Subcommand)]
enum WorkspaceCommand {
    /// List workspaces (optionally filtered to a single project).
    Ls {
        /// Limit to one project id. When omitted, lists across all projects.
        #[arg(long, value_name = "ID")]
        project: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum SessionCommand {
    /// List sessions (optionally filtered to a single workarea).
    Ls {
        /// Limit to one workarea id. When omitted, lists across all workareas.
        #[arg(long, value_name = "ID")]
        workarea: Option<String>,
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
            eprintln!("concerto: failed to build async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    match rt.block_on(dispatch(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("concerto: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn dispatch(cli: Cli) -> Result<(), CommandError> {
    let format = OutputFormat::from_json_flag(cli.json);
    let socket = client::resolve_socket_path(cli.socket)?;

    match cli.command {
        Command::Status => commands::status::run(&socket, format).await,
        Command::Workspace(WorkspaceCommand::Ls { project }) => {
            commands::workspace::run(&socket, project, format).await
        }
        Command::Session(SessionCommand::Ls { workarea }) => {
            commands::session::run(&socket, workarea, format).await
        }
    }
}
