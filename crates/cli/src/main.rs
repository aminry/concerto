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
    /// Capture a portable backup of the local Concerto state.
    ///
    /// Unlike the other subcommands, `backup` operates on the local DB file
    /// directly (a hot-consistent `VACUUM INTO` snapshot) and does NOT dial
    /// the Core, so it works even when no Core is running. It writes a frozen
    /// `<out>/` layout: `concerto.db`, optional `worktrees.tar`, optional
    /// `audit.jsonl`, and a `manifest.json`.
    Backup {
        /// Output directory for the backup artifacts. Created if missing.
        /// Defaults to `./concerto-backup`.
        #[arg(long, value_name = "DIR", default_value = "concerto-backup")]
        out: PathBuf,
        /// Also tar the worktree directory tree into `<out>/worktrees.tar`.
        #[arg(long)]
        with_worktrees: bool,
        /// Inclusive lower bound (ISO-8601, e.g. `2026-05-01`) on audit
        /// records to export into `<out>/audit.jsonl`. Setting either
        /// `--audit-from` or `--audit-to` enables the audit export.
        #[arg(long, value_name = "TS")]
        audit_from: Option<String>,
        /// Inclusive upper bound (ISO-8601) on audit records to export.
        #[arg(long, value_name = "TS")]
        audit_to: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum WorkspaceCommand {
    /// List workspaces (the global registry; there is no project filter).
    Ls {
        /// Include archived workspaces in the listing.
        #[arg(long)]
        include_archived: bool,
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

    match cli.command {
        // `backup` is file-level (local DB + worktrees + audit JSONL) and
        // never dials the Core, so it must NOT resolve/require the UDS socket
        // — that would force a running Core and pull in the Unix-only client
        // path. It resolves its own paths from the environment.
        Command::Backup {
            out,
            with_worktrees,
            audit_from,
            audit_to,
        } => {
            let args = commands::backup::BackupArgs {
                out,
                with_worktrees,
                audit_from,
                audit_to,
            };
            commands::backup::run(args, format).await
        }
        Command::Status => {
            let socket = client::resolve_socket_path(cli.socket)?;
            commands::status::run(&socket, format).await
        }
        Command::Workspace(WorkspaceCommand::Ls { include_archived }) => {
            let socket = client::resolve_socket_path(cli.socket)?;
            commands::workspace::run(&socket, include_archived, format).await
        }
        Command::Session(SessionCommand::Ls { workarea }) => {
            let socket = client::resolve_socket_path(cli.socket)?;
            commands::session::run(&socket, workarea, format).await
        }
    }
}
