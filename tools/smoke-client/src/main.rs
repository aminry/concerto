//! `smoke-client` — the canonical end-to-end client used by
//! `scripts/smoke.sh`.
//!
//! Task 15 shipped a single-call client (`--socket <path>` →
//! `Runtime.GetServerCapabilities`). Task 27 promotes it to a clap
//! subcommand dispatcher covering the Phase 2 happy path:
//!
//!   * `caps` — `Runtime.GetServerCapabilities`.
//!   * `add-repo` — `Repositories.AddRepository` (global registry; no
//!     project scoping after the Project→Workspace collapse).
//!   * `clone` — `Repositories.Clone` (streaming, drained to EOS).
//!   * `new-workspace` — `Workspaces.CreateWorkspace`.
//!   * `new-workarea` — `Workareas.CreateWorkarea`.
//!   * `start-session` — `Sessions.CreateSession`.
//!   * `stream-session-io` — `Streams.Subscribe(session.io.<sid>)` +
//!     `Streams.Subscribe(session.events.<sid>)` raced with a
//!     `--timeout`. Stdout is the agent's stdout bytes.
//!   * `stop-session` — `Sessions.StopSession`.
//!
//! Task 52 (smoke gate v3) adds the Phase 3 surface:
//!
//!   * `set-perm-mode` — `Workareas.UpdateWorkareaPermissionMode`.
//!   * `create-loop` — `Schedules.CreateSchedule` (kind=loop).
//!   * `list-loops` — `Schedules.ListSchedules`.
//!   * `list-skills` — `Skills.RefreshMarketplaces` + `Skills.ListSkills`.
//!   * `list-mcp` — `Sessions.ListMcpServers`.
//!   * `list-audit` — reads `<data_dir>/audit/audit-<YYYY-MM-DD>.jsonl`
//!     directly (no gRPC channel; the JSONL writer is the only producer).
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
    /// Path to the Core's UDS socket. Required by every gRPC subcommand
    /// (all of them except `list-audit`, which reads the JSONL log file).
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
    /// Call `Repositories.AddRepository` and print the new repo id.
    /// Repositories are a global registry — no `--project-id`. `name` is
    /// globally unique; pass a distinct `--name` per repo.
    AddRepo {
        #[arg(long)]
        url: String,
        /// Globally-unique repo name (default `smoke-repo`).
        #[arg(long, default_value = "")]
        name: String,
        /// Task 301 clone strategy: `full | blobless | treeless` (empty →
        /// full). The `sparse-cone-clone` smoke check passes `blobless`.
        #[arg(long, default_value = "")]
        clone_strategy: String,
        /// Task 301: append `--sparse --no-checkout` so the worktree lands
        /// empty for Task 302's cone-set to populate.
        #[arg(long, default_value_t = false)]
        with_sparse: bool,
    },
    /// Call `Repositories.Clone` and drain the progress stream.
    Clone {
        #[arg(long)]
        repo_id: String,
    },
    /// Task 302: call `Repositories.SetCones` for a (workarea, repo) and
    /// print one applied cone path per line. Pass `--cone <path>` once per
    /// cone directory (repeatable); an empty set cones to top-level files.
    SetCones {
        #[arg(long)]
        workarea: String,
        #[arg(long)]
        repo: String,
        /// Repeatable cone path (repo-root-relative, forward-slash).
        #[arg(long = "cone")]
        cone: Vec<String>,
    },
    /// Call `Workspaces.CreateWorkspace` and print the workspace id.
    NewWorkspace {
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
    /// Call `Sessions.SendMessage`. The payload is `--text` plus a
    /// trailing newline, sent as raw UTF-8 bytes to the agent's stdin.
    SendMessage {
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        text: String,
    },
    /// Call `Sessions.StopSession`.
    StopSession {
        #[arg(long)]
        session_id: String,
    },
    /// Task 202: probe the `Streams.Subscribe` reconnect path
    /// (since_offset replay + AckOffset prune → GapDetected) over the
    /// live UDS Core. Self-contained: subscribes to `workspace.events`,
    /// creates two workspaces, reconnects with `since_offset`, then acks
    /// and reconnects again to force a gap. Prints `OK` on success.
    StreamsReplayProbe {
        #[arg(long)]
        repo_id: String,
    },
    /// Task 203: probe the `Files` service over the live UDS Core. Uploads
    /// a multi-chunk file into the workarea's `.context/` root, downloads
    /// it back (asserting byte-identical + matching BLAKE2b-256), stats it,
    /// and asserts an out-of-scope `../escape.txt` upload is rejected.
    /// Prints `files-transfer-probe: OK` on success.
    FilesTransferProbe {
        #[arg(long)]
        workarea_id: String,
    },
    /// Call `Workareas.UpdateWorkareaPermissionMode`. Prints the
    /// resulting mode's proto-enum string name.
    SetPermMode {
        #[arg(long)]
        workarea: String,
        /// One of `strict | normal | auto | yolo`.
        #[arg(long)]
        mode: String,
        /// Required acknowledgement string when `mode = yolo`
        /// (literal `"I understand"` per Task 32). Ignored otherwise.
        #[arg(long)]
        ack: Option<String>,
    },
    /// Call `Schedules.CreateSchedule` with `kind = "loop"`. Prints the
    /// new schedule id. The smoke gate does NOT wait for a fire — that
    /// is covered by `crates/core/tests/scheduler_loop.rs`.
    CreateLoop {
        #[arg(long)]
        workarea: String,
        /// Interval in seconds; must be in 30..=604800.
        #[arg(long)]
        interval: i64,
        #[arg(long)]
        prompt: String,
    },
    /// Call `Schedules.ListSchedules` for a workarea. Prints one
    /// schedule id per line.
    ListLoops {
        #[arg(long)]
        workarea: String,
    },
    /// Call `Skills.RefreshMarketplaces` then `Skills.ListSkills`.
    /// Prints one skill name per line.
    ListSkills {
        /// Optional scope filter: `personal | workspace | plugin | enterprise`.
        #[arg(long)]
        scope: Option<String>,
        /// Optional workspace id filter (only meaningful for workspace scope).
        #[arg(long)]
        workspace_id: Option<String>,
    },
    /// Call `Sessions.ListMcpServers`. Prints one server name per line.
    ListMcp {
        /// Optional scope filter: `personal | project | plugin | enterprise`.
        #[arg(long)]
        scope: Option<String>,
        /// Required when `scope = project`; the repository id whose
        /// `<local_path>/.mcp.json` to read.
        #[arg(long)]
        repository_id: Option<String>,
    },
    /// Read today's JSONL audit log under `<data-dir>/audit/`. Prints
    /// one `kind` per row; when `--kind` is set, only matching rows
    /// surface.
    ListAudit {
        /// Data directory (matches Core's `CONCERTO_DATA_DIR`).
        #[arg(long)]
        data_dir: std::path::PathBuf,
        /// Optional exact `kind` filter (e.g. `workspace_created`).
        #[arg(long)]
        kind: Option<String>,
    },
    /// Task 507: call `Notifications.GetInbox` and print one JSON object per
    /// returned notification (newest-first). The `notifications` smoke check
    /// greps the output for a seeded id to prove the live service round-trips.
    GetInbox {
        /// Only return unread notifications (default: all).
        #[arg(long, default_value_t = false)]
        unread_only: bool,
        /// Max rows to return; `0` ⇒ the Core default.
        #[arg(long, default_value_t = 0)]
        limit: u32,
    },
    /// Call `Maestro.SendToMaestro` — forward a chat turn (freeform,
    /// `@workarea`, or a slash directive). Prints `sent`.
    MaestroSend {
        #[arg(long)]
        text: String,
        /// Optional workspace scope hint (a bare `@composer` resolves here).
        #[arg(long)]
        workspace: Option<String>,
    },
    /// Call `Maestro.GetState` — print the 9-field read-model as JSON.
    MaestroState,
    /// Call `Maestro.GetHistory` — print one `role\ttext` line per turn.
    MaestroHistory,
    /// Call `Maestro.GetDigest` — print the digest text + chip count.
    MaestroDigest,
    /// Subscribe to `maestro.events` and print each decoded frame until
    /// the timeout fires. Used to assert replies / routing notices.
    MaestroWatch {
        /// Wall-clock budget, seconds.
        #[arg(long, default_value_t = 20)]
        timeout: u64,
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
/// `list-audit` is the lone exception (it reads the JSONL log file).
fn require_socket(socket: Option<PathBuf>) -> Result<PathBuf, String> {
    socket.ok_or_else(|| "missing --socket <path> (required for gRPC subcommands)".to_string())
}

async fn dispatch(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Caps => {
            let socket = require_socket(cli.socket)?;
            cmd::caps::run(&socket).await
        }
        Command::AddRepo {
            url,
            name,
            clone_strategy,
            with_sparse,
        } => {
            let socket = require_socket(cli.socket)?;
            cmd::add_repo::run(&socket, &url, &name, &clone_strategy, with_sparse).await
        }
        Command::Clone { repo_id } => {
            let socket = require_socket(cli.socket)?;
            cmd::clone::run(&socket, &repo_id).await
        }
        Command::SetCones {
            workarea,
            repo,
            cone,
        } => {
            let socket = require_socket(cli.socket)?;
            cmd::set_cones::run(&socket, &workarea, &repo, &cone).await
        }
        Command::NewWorkspace { name, repo_id } => {
            let socket = require_socket(cli.socket)?;
            cmd::new_workspace::run(&socket, &name, &repo_id).await
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
        Command::SendMessage { session_id, text } => {
            let socket = require_socket(cli.socket)?;
            cmd::send_message::run(&socket, &session_id, &text).await
        }
        Command::StopSession { session_id } => {
            let socket = require_socket(cli.socket)?;
            cmd::stop_session::run(&socket, &session_id).await
        }
        Command::StreamsReplayProbe { repo_id } => {
            let socket = require_socket(cli.socket)?;
            cmd::streams_replay_probe::run(&socket, &repo_id).await
        }
        Command::FilesTransferProbe { workarea_id } => {
            let socket = require_socket(cli.socket)?;
            cmd::files_transfer_probe::run(&socket, &workarea_id).await
        }
        Command::SetPermMode {
            workarea,
            mode,
            ack,
        } => {
            let socket = require_socket(cli.socket)?;
            cmd::set_perm_mode::run(&socket, &workarea, &mode, ack.as_deref()).await
        }
        Command::CreateLoop {
            workarea,
            interval,
            prompt,
        } => {
            let socket = require_socket(cli.socket)?;
            cmd::create_loop::run(&socket, &workarea, interval, &prompt).await
        }
        Command::ListLoops { workarea } => {
            let socket = require_socket(cli.socket)?;
            cmd::list_loops::run(&socket, &workarea).await
        }
        Command::ListSkills {
            scope,
            workspace_id,
        } => {
            let socket = require_socket(cli.socket)?;
            cmd::list_skills::run(&socket, scope.as_deref(), workspace_id.as_deref()).await
        }
        Command::ListMcp {
            scope,
            repository_id,
        } => {
            let socket = require_socket(cli.socket)?;
            cmd::list_mcp::run(&socket, scope.as_deref(), repository_id.as_deref()).await
        }
        Command::ListAudit { data_dir, kind } => {
            cmd::list_audit::run(&data_dir, kind.as_deref()).await
        }
        Command::GetInbox { unread_only, limit } => {
            let socket = require_socket(cli.socket)?;
            cmd::get_inbox::run(&socket, unread_only, limit).await
        }
        Command::MaestroSend { text, workspace } => {
            let socket = require_socket(cli.socket)?;
            cmd::maestro::send(&socket, &text, workspace).await
        }
        Command::MaestroState => {
            let socket = require_socket(cli.socket)?;
            cmd::maestro::state(&socket).await
        }
        Command::MaestroHistory => {
            let socket = require_socket(cli.socket)?;
            cmd::maestro::history(&socket).await
        }
        Command::MaestroDigest => {
            let socket = require_socket(cli.socket)?;
            cmd::maestro::digest(&socket).await
        }
        Command::MaestroWatch { timeout } => {
            let socket = require_socket(cli.socket)?;
            cmd::maestro::watch(&socket, timeout).await
        }
    }
}
