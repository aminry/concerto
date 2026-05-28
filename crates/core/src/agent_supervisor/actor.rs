//! `AgentSupervisorActor` + cloneable `AgentSupervisorHandle` (Task 22).
//!
//! The actor pattern matches the existing managers
//! (`RepoManagerActor`, `WorkspaceManagerActor`, `WorkareaManagerActor`):
//! the actor's `run` parks on shutdown; all meaningful work flows
//! through the handle.
//!
//! ## Locked surface (Task 22)
//!
//! `AgentSupervisorHandle::{start_session, send_input, stop_session,
//! subscribe_events}` — signatures frozen.
//!
//! ## V0.1 session state machine
//!
//! ```text
//!  start_session  ─────────►  starting  ─── Hello/Ready ──►  running
//!                                                              │
//!                                                              ▼
//!                  stop_session ───────────────────────────► finished
//!                                                              │
//!                                                              ▼
//!                            host AgentExited                finished
//! ```
//!
//! On any error inside `start_session` (host fails to bind, handshake
//! fails, timeout) the session row is removed (rollback) and the host
//! process is killed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use concerto_error::{Error, Result};
use concerto_persist::{NewChat, NewSession, Persistence, SessionId, WorkareaId};
use sqlx::Connection;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::process::Child;
use tokio::sync::{broadcast, Mutex};

use crate::agent_supervisor::approval::{user_decision_string, PendingApprovals};
use crate::agent_supervisor::bridge::{
    build_hello, read_frame, write_frame, FrameError, HostFrame,
};
use crate::agent_supervisor::events::{AgentEvent, MessageRole};
use crate::agent_supervisor::parsers::{
    claude_code::ClaudeCodePack, echo::EchoPack, MsgRole, ParseEvent, ParserPack,
};
use crate::agent_supervisor::spawn::{spawn_host, wait_for_socket, SOCKET_POLL_BUDGET};
use crate::security::{Decision, PermissionResolver};
use crate::supervisor::{Actor, ActorContext};

/// Channel capacity for the in-process per-session broadcast of
/// [`AgentEvent`]s. Sized to match the Workarea/Workspace managers.
const EVENTS_CAPACITY: usize = 256;

/// V0.1 supported agent kinds. `Echo` is a test-only spawn mode that
/// wraps `/usr/bin/echo`; `Claude` is the real CLI. `Codex` and `Gemini`
/// are accepted at the type layer but cause `start_session` to error
/// with `NOT_IMPLEMENTED` (parser-pack work, Task 33).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentKind {
    Echo,
    Claude,
    Codex,
    Gemini,
}

impl AgentKind {
    /// V0.1 mapping from the in-process kind to the SQL `agent_kind`
    /// CHECK-set value persisted on the `sessions` row.
    ///
    /// `Echo` reuses `'claude'` because migration 0001's CHECK constraint
    /// does not include an `'echo'` value (the CHECK set is frozen). The
    /// schema kind is decoupled from the spawn-time binary — `start_session`
    /// reads the [`AgentKind`] enum to decide what to exec; the DB row
    /// is just record-keeping for the UI.
    pub fn as_db_kind(&self) -> &'static str {
        match self {
            AgentKind::Echo => "claude",
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Gemini => "gemini",
        }
    }
}

/// Caller-supplied parameters for [`AgentSupervisorHandle::start_session`].
#[derive(Clone, Debug)]
pub struct StartSessionRequest {
    /// Workarea this session belongs to.
    pub workarea_id: WorkareaId,
    /// Which agent CLI to spawn (Task 22 V0.1 supports Echo and Claude).
    pub agent_kind: AgentKind,
    /// For `AgentKind::Echo`, the payload to echo. Ignored for other
    /// kinds.
    pub echo_text: Option<String>,
    /// Working directory for the agent CLI (typically the workarea's
    /// worktree root).
    pub cwd: PathBuf,
    /// Initial permission mode persisted on the session row. Defaults
    /// to `"normal"` when `None`.
    pub permission_mode: Option<String>,
}

/// Config for the actor's `run` loop. V0.1 has no knobs — the actor
/// parks on shutdown.
#[derive(Clone, Debug, Default)]
pub struct AgentSupervisorConfig;

/// Raw I/O chunk surfaced by [`AgentSupervisorHandle::subscribe_session_io`].
///
/// The `events` broadcast carries the parsed `AgentEvent` view of an
/// agent's output (Task 22). Task 23's `Streams` service additionally
/// exposes the raw bytes via the `session.io.<sid>` subject; this struct
/// is the wire shape for that subject. V0.1 only ever publishes
/// `stream = "stdout"` because `portable-pty` merges the child's stderr
/// into the master, but the field is here so V1.0 stderr-aware parsers
/// don't need a wire-format change.
#[derive(Clone, Debug)]
pub struct SessionIoChunk {
    pub session_id: SessionId,
    /// `"stdout"` or `"stderr"`.
    pub stream: &'static str,
    pub data: Vec<u8>,
}

/// Maximum number of [`AgentEvent`]s the supervisor replays to a new
/// subscriber that attaches mid-session. V0.1's per-session ring buffer
/// (`design/10 §3.3`) is V1.0 work; this small replay buffer is the
/// minimum needed to keep the `Streams.Subscribe(session.events.<sid>)`
/// surface honest for fast-finishing sessions (echo agent, smoke
/// gate). The cap is deliberately tight so the buffer's memory cost is
/// bounded by `MAX_REPLAY_EVENTS × sizeof(AgentEvent)` per session.
const MAX_REPLAY_EVENTS: usize = 64;

/// Maximum number of [`SessionIoChunk`]s the supervisor replays. Same
/// rationale as [`MAX_REPLAY_EVENTS`].
const MAX_REPLAY_IO: usize = 64;

/// Per-session in-process state held by the supervisor.
struct SessionEntry {
    workarea_id: WorkareaId,
    /// `sessions.chat_id` — cached so the read-pump's checkpoint /
    /// revert paths (Task 34) don't have to round-trip the DB to
    /// resolve the per-session chat thread.
    chat_id: String,
    /// 32-byte cookie issued at spawn time. Held in process only — the
    /// schema does not include a slot for it on `sessions` in V0.1, so
    /// the supervisor uses this map for `send_input` /  cookie-aware
    /// reconnect work that lands in Task 36.
    #[allow(dead_code)]
    cookie: [u8; 32],
    /// Effective permission mode at start time (Task 32). Mirrors the
    /// row column but kept in process so the supervisor doesn't have to
    /// re-read the DB on every send_input / approval check. Updated by
    /// [`AgentSupervisorHandle::update_session_permission_mode`].
    permission_mode: crate::security::PermissionMode,
    /// Per-session UDS path.
    socket_path: PathBuf,
    /// Broadcast sender — subscribers receive [`AgentEvent`].
    events: broadcast::Sender<AgentEvent>,
    /// Replay buffer for [`AgentEvent`]s. New subscribers receive these
    /// events before any live broadcast traffic. Bounded to
    /// [`MAX_REPLAY_EVENTS`] entries (oldest dropped).
    events_replay: Arc<Mutex<Vec<AgentEvent>>>,
    /// Broadcast sender for raw I/O chunks. Task 23 surfaces this via
    /// `Streams.Subscribe(subject="session.io.<sid>")`.
    io: broadcast::Sender<SessionIoChunk>,
    /// Replay buffer for [`SessionIoChunk`]s.
    io_replay: Arc<Mutex<Vec<SessionIoChunk>>>,
    /// Writer half of the bridge connection; held under a mutex so
    /// `send_input` can serialize stdin writes.
    writer: Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>,
    /// Child handle. Held under a mutex so `stop_session` can `kill`
    /// without racing the read-loop task.
    child: Arc<Mutex<Option<Child>>>,
    /// Set once the agent has exited or been stopped. Subscribers
    /// attaching after this stays `true` get the replay buffer but no
    /// live events.
    finished: Arc<std::sync::atomic::AtomicBool>,
    /// Task 33: per-CLI parser pack. Constructed at `start_session`
    /// based on `agent_kind`; the read-pump invokes
    /// `parse_chunk` on every `StdoutBytes` frame. Held on the entry
    /// so future tasks (e.g. checkpoint restore) can build a fresh
    /// pump against the original pack without re-detecting the CLI.
    #[allow(dead_code)]
    parser: Arc<dyn ParserPack>,
    /// Task 33: pending approvals awaiting a `Sessions.ResolveApproval`
    /// call. Keyed by `tool_approvals.id`.
    pending_approvals: Arc<Mutex<PendingApprovals>>,
}

/// Cloneable, shareable handle to the Agent Supervisor's state.
///
/// All meaningful work flows through this struct. The actor's `run`
/// merely parks on shutdown so future watchdog hooks have somewhere to
/// land.
#[derive(Clone)]
pub struct AgentSupervisorHandle {
    persistence: Arc<Persistence>,
    /// `<data_dir>` — the per-session socket path is
    /// `<data_dir>/runtime/agents/<sid>.sock` (locked layout).
    data_dir: Arc<PathBuf>,
    /// `<config_dir>` — used by Task 32's resolver to read
    /// `managed.json`.
    config_dir: Arc<PathBuf>,
    /// Resolved path to `concerto-agent-host`. Tests inject a path
    /// resolved by `assert_cmd::cargo::cargo_bin`; production resolves
    /// via `current_exe().parent().join(...)` at start_session time.
    host_bin: Arc<PathBuf>,
    sessions: Arc<Mutex<HashMap<SessionId, SessionEntry>>>,
}

impl AgentSupervisorHandle {
    /// Build a fresh handle. Normally callers go through
    /// [`AgentSupervisorActor::new`]; this is `pub` so tests can
    /// construct one without the supervisor.
    pub fn new(
        persistence: Arc<Persistence>,
        data_dir: Arc<PathBuf>,
        config_dir: Arc<PathBuf>,
        host_bin: PathBuf,
    ) -> Self {
        Self {
            persistence,
            data_dir,
            config_dir,
            host_bin: Arc::new(host_bin),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Borrow the shared persistence handle. Used by the gRPC
    /// `Sessions` handler (Task 23) to read `sessions` rows directly for
    /// `Get` / `List` without an extra `Arc<Persistence>` plumbed
    /// through `api_server`.
    pub fn persistence(&self) -> Arc<Persistence> {
        Arc::clone(&self.persistence)
    }

    /// Subscribe to the per-session [`AgentEvent`] broadcast. Returns
    /// `None` if the session is unknown (e.g. never created — entries
    /// are kept alive after exit specifically so Task 23's `Streams`
    /// service can replay recent events).
    ///
    /// The returned [`broadcast::Receiver`] only receives events
    /// produced after the call; for replay coverage of fast-finishing
    /// sessions, see [`AgentSupervisorHandle::subscribe_events_with_replay`].
    pub async fn subscribe_events(
        &self,
        session_id: &SessionId,
    ) -> Option<broadcast::Receiver<AgentEvent>> {
        let map = self.sessions.lock().await;
        map.get(session_id).map(|e| e.events.subscribe())
    }

    /// Subscribe to the per-session [`AgentEvent`] broadcast WITH a
    /// snapshot of recent buffered events (up to [`MAX_REPLAY_EVENTS`]).
    /// Returns `(replay, receiver)`. Callers (the `Streams` handler in
    /// Task 23) emit the replay first, then forward live frames from
    /// the receiver. This is the V0.1 substitute for `since_offset` on
    /// the `Streams.Subscribe` wire: it lets the gRPC client see the
    /// first burst of events even when its subscribe races a
    /// fast-finishing agent (e.g. the echo path).
    pub async fn subscribe_events_with_replay(
        &self,
        session_id: &SessionId,
    ) -> Option<(Vec<AgentEvent>, broadcast::Receiver<AgentEvent>)> {
        let map = self.sessions.lock().await;
        let entry = map.get(session_id)?;
        let replay = entry.events_replay.lock().await.clone();
        Some((replay, entry.events.subscribe()))
    }

    /// Subscribe to the per-session raw I/O broadcast (Task 23). Returns
    /// `None` if the session is unknown. The `Streams` service uses this
    /// for the `session.io.<sid>` subject; the `AgentEvent` view (used
    /// for `session.events.<sid>`) lives on a separate channel.
    pub async fn subscribe_session_io(
        &self,
        session_id: &SessionId,
    ) -> Option<broadcast::Receiver<SessionIoChunk>> {
        let map = self.sessions.lock().await;
        map.get(session_id).map(|e| e.io.subscribe())
    }

    /// `subscribe_session_io` with replay; see
    /// [`AgentSupervisorHandle::subscribe_events_with_replay`].
    pub async fn subscribe_session_io_with_replay(
        &self,
        session_id: &SessionId,
    ) -> Option<(Vec<SessionIoChunk>, broadcast::Receiver<SessionIoChunk>)> {
        let map = self.sessions.lock().await;
        let entry = map.get(session_id)?;
        let replay = entry.io_replay.lock().await.clone();
        Some((replay, entry.io.subscribe()))
    }

    /// Start a new agent session. See module docs for the state machine.
    pub async fn start_session(&self, req: StartSessionRequest) -> Result<SessionId> {
        match req.agent_kind {
            AgentKind::Codex | AgentKind::Gemini => {
                return Err(Error::Validation(format!(
                    "agent.not_implemented: agent_kind {:?} is not supported in V0.1 \
                     (parser pack arrives in Phase 3)",
                    req.agent_kind
                )));
            }
            AgentKind::Echo | AgentKind::Claude => {}
        }

        // Validate the workarea exists. The Workarea Manager is
        // responsible for keeping `status` valid; the supervisor only
        // refuses workareas the caller invented.
        let workarea =
            concerto_persist::workareas::get(self.persistence.readers(), &req.workarea_id)
                .await?
                .ok_or_else(|| {
                    Error::NotFound(format!("workarea {} not found", req.workarea_id))
                })?;
        if workarea.archived_at.is_some() {
            return Err(Error::Validation(format!(
                "workarea.archived: workarea {} is archived",
                req.workarea_id
            )));
        }

        // 1. Generate cookie + session id + paths.
        let mut cookie = [0u8; 32];
        getrandom::getrandom(&mut cookie)
            .map_err(|e| Error::Internal(format!("getrandom: {e}")))?;
        let session_id = SessionId(uuid::Uuid::now_v7().to_string());
        // The socket path is constrained to ~104 chars on macOS
        // (`SUN_LEN`/`sockaddr_un.sun_path`). The locked layout
        // `<data_dir>/runtime/agents/<sid>.sock` keeps the suffix short,
        // but the data_dir prefix can already be deep (e.g. macOS
        // tempdirs nest under `/var/folders/.../T/.tmpXXXX/data/`). When
        // the locked path would overflow, fall back to placing the
        // socket directly under `$TMPDIR` keyed by a short session id
        // prefix — every other artefact (logs, final-info) still uses
        // the canonical layout under `data_dir` so on-disk
        // observability is unaffected.
        let runtime_dir = self.data_dir.join("runtime").join("agents");
        tokio::fs::create_dir_all(&runtime_dir).await?;
        let canonical_socket = runtime_dir.join(format!("{}.sock", session_id.0));
        let socket_path = if canonical_socket.to_string_lossy().len() < 100 {
            canonical_socket
        } else {
            // Truncate the UUID to 8 chars; this is enough collision
            // resistance for the in-process map and the socket is removed
            // on session end.
            let short = &session_id.0[..8.min(session_id.0.len())];
            let tmp = std::env::temp_dir().join(format!("ccs-{short}.sock"));
            tmp
        };
        let log_dir = self.data_dir.join("agents").join(&session_id.0);
        tokio::fs::create_dir_all(&log_dir).await?;
        let stdout_log = log_dir.join("stdout.log");
        let final_info = log_dir.join("final-info.json");

        // 2. Persist `chats` + `sessions` rows in a single transaction.
        //
        // `sessions.chat_id` is `NOT NULL REFERENCES chats(id)` and
        // `chats.session_id` is `REFERENCES sessions(id)` with a CHECK
        // that requires `session_id IS NOT NULL` for `kind = 'session'`
        // — i.e. the two rows reference each other and neither can be
        // inserted first under default (immediate) FK enforcement.
        // SQLite's `PRAGMA defer_foreign_keys = ON` defers FK checking
        // to commit time for the duration of the current transaction
        // only; both rows are visible by then and the cycle resolves.
        // (`design/09 §6.2` calls out the cyclical chats↔sessions FKs
        // as expected schema behaviour.)
        let now_ms = now_unix_ms();
        // Task 32: if the caller specified a mode, validate + cap it;
        // otherwise walk the workarea → workspace → project →
        // managed → default chain and use the resolved value. We
        // resolve BEFORE inserting the session row so the row carries
        // the effective mode from row 1; the in-memory cache below
        // mirrors it.
        let permission_mode = match req.permission_mode.clone() {
            Some(s) => {
                let parsed = crate::security::parse_permission_mode(&s)?;
                let managed = crate::security::load_managed_policy(&self.config_dir);
                let _capped = crate::security::permission::enforce_managed_cap(parsed, &managed)?;
                parsed.as_str().to_string()
            }
            None => {
                // Inherit-from-workarea: walk workarea → workspace →
                // project → managed → default WITHOUT a session row
                // (the row doesn't exist yet). Helper mirrors
                // `resolve_effective_mode` but takes a workarea id.
                let resolved =
                    resolve_for_new_session(&self.persistence, &self.config_dir, &req.workarea_id)
                        .await?;
                resolved.mode.as_str().to_string()
            }
        };
        let permission_mode_enum = crate::security::parse_permission_mode(&permission_mode)?;
        let chat_id = uuid::Uuid::now_v7().to_string();
        {
            let mut writer = self.persistence.writer().await;
            let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
            sqlx::query("PRAGMA defer_foreign_keys = ON")
                .execute(&mut *tx)
                .await
                .map_err(|e| Error::Sqlx(Box::new(e)))?;
            concerto_persist::sessions::insert_chat(
                &mut tx,
                NewChat {
                    id: chat_id.clone(),
                    session_id: Some(session_id.0.clone()),
                    kind: "session".to_string(),
                    created_at: now_ms,
                },
            )
            .await?;
            concerto_persist::sessions::insert(
                &mut tx,
                NewSession {
                    id: session_id.clone(),
                    workarea_id: req.workarea_id.clone(),
                    chat_id: chat_id.clone(),
                    agent_kind: req.agent_kind.as_db_kind().to_string(),
                    agent_version: None,
                    model: None,
                    mode: None,
                    host_pid: None,
                    host_socket: Some(socket_path.to_string_lossy().into_owned()),
                    pty_cookie: Some(cookie.to_vec()),
                    external_session_id: None,
                    permission_mode: permission_mode.clone(),
                    bypass_destructive_guard: false,
                    started_at: now_ms,
                    status: "starting".to_string(),
                },
            )
            .await?;
            tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        }

        // 3. Spawn the host process.
        let (agent_bin, agent_args) = resolve_agent_bin(&req)?;
        let cookie_hex = hex::encode(cookie);
        let mut child = spawn_host(
            &self.host_bin,
            &agent_bin,
            &agent_args,
            &req.cwd,
            &socket_path,
            &cookie_hex,
            &final_info,
        )
        .map_err(|e| Error::Internal(format!("spawn agent-host: {e}")))?;
        let host_pid = child.id().map(|p| p as i64).unwrap_or(-1);

        // 4. Wait for the host's UDS to appear, then connect + handshake.
        let socket_ready = wait_for_socket(&socket_path, SOCKET_POLL_BUDGET).await;
        if let Err(e) = socket_ready {
            // Best-effort cleanup: kill the host, drop the row.
            let _ = child.kill().await;
            self.mark_failed(&session_id).await;
            return Err(e);
        }
        let stream = match UnixStream::connect(&socket_path).await {
            Ok(s) => s,
            Err(e) => {
                let _ = child.kill().await;
                self.mark_failed(&session_id).await;
                return Err(Error::Io(e));
            }
        };
        let (mut read_half, mut write_half) = stream.into_split();
        let hello = build_hello(env!("CARGO_PKG_VERSION"), cookie);
        if let Err(e) = write_frame(&mut write_half, &hello).await {
            let _ = child.kill().await;
            self.mark_failed(&session_id).await;
            return Err(Error::Internal(format!("write Hello: {e}")));
        }
        // Expect Ready.
        let ready = match read_frame(&mut read_half).await {
            Ok(f) => f,
            Err(e) => {
                let _ = child.kill().await;
                self.mark_failed(&session_id).await;
                return Err(Error::Internal(format!("read Ready: {e}")));
            }
        };
        match ready {
            HostFrame::Ready { .. } => {}
            HostFrame::CookieMismatch => {
                let _ = child.kill().await;
                self.mark_failed(&session_id).await;
                return Err(Error::Internal(
                    "agent-host rejected cookie (mismatch)".to_string(),
                ));
            }
            HostFrame::AlreadyConnected => {
                let _ = child.kill().await;
                self.mark_failed(&session_id).await;
                return Err(Error::Internal(
                    "agent-host reports another Core is connected".to_string(),
                ));
            }
            other => {
                let _ = child.kill().await;
                self.mark_failed(&session_id).await;
                return Err(Error::Internal(format!(
                    "unexpected handshake frame {:?}",
                    other
                )));
            }
        }

        // 5. Mark `running` in the DB.
        {
            let mut writer = self.persistence.writer().await;
            concerto_persist::sessions::update_host(
                &mut writer,
                &session_id,
                host_pid,
                &socket_path.to_string_lossy(),
                "running",
            )
            .await?;
        }

        // 6. Register in-process state + start the read-pump task.
        let (events, _) = broadcast::channel(EVENTS_CAPACITY);
        let (io, _) = broadcast::channel(EVENTS_CAPACITY);
        let events_replay = Arc::new(Mutex::new(Vec::<AgentEvent>::new()));
        let io_replay = Arc::new(Mutex::new(Vec::<SessionIoChunk>::new()));
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer_arc = Arc::new(Mutex::new(write_half));
        let child_arc = Arc::new(Mutex::new(Some(child)));
        // Task 33: construct the per-CLI parser pack based on the
        // requested agent kind. Echo gets the trivial pass-through;
        // Claude gets the V0.1 regex pack. Codex/Gemini error earlier
        // in this function, so they never reach this branch.
        let parser: Arc<dyn ParserPack> = match req.agent_kind {
            AgentKind::Echo => Arc::new(EchoPack::new()),
            AgentKind::Claude => Arc::new(ClaudeCodePack::new()),
            AgentKind::Codex | AgentKind::Gemini => unreachable!("rejected above"),
        };
        let pending_approvals: Arc<Mutex<PendingApprovals>> =
            Arc::new(Mutex::new(PendingApprovals::new()));
        {
            let mut map = self.sessions.lock().await;
            map.insert(
                session_id.clone(),
                SessionEntry {
                    workarea_id: req.workarea_id.clone(),
                    chat_id: chat_id.clone(),
                    cookie,
                    permission_mode: permission_mode_enum,
                    socket_path: socket_path.clone(),
                    events: events.clone(),
                    events_replay: Arc::clone(&events_replay),
                    io: io.clone(),
                    io_replay: Arc::clone(&io_replay),
                    writer: writer_arc.clone(),
                    child: child_arc.clone(),
                    finished: Arc::clone(&finished),
                    parser: Arc::clone(&parser),
                    pending_approvals: Arc::clone(&pending_approvals),
                },
            );
        }

        // Started event, before launching the read pump so any
        // subscriber registered after `start_session` returns sees an
        // already-running session.
        let started = AgentEvent::Started {
            session_id: session_id.clone(),
        };
        push_replay(&events_replay, started.clone()).await;
        let _ = events.send(started);

        let pump_persistence = Arc::clone(&self.persistence);
        let pump_session = session_id.clone();
        let pump_workarea = req.workarea_id.clone();
        let pump_chat = chat_id.clone();
        let pump_events = events.clone();
        let pump_events_replay = Arc::clone(&events_replay);
        let pump_io = io.clone();
        let pump_io_replay = Arc::clone(&io_replay);
        let pump_sessions = Arc::clone(&self.sessions);
        let pump_log = stdout_log.clone();
        let pump_finished = Arc::clone(&finished);
        let pump_parser = Arc::clone(&parser);
        let pump_pending = Arc::clone(&pending_approvals);
        let pump_writer = Arc::clone(&writer_arc);
        // Resolver constructed once per session; its mode field is
        // refreshed in `update_session_permission_mode`. V0.1 reads the
        // workarea bypass off the just-resolved row.
        let bypass = bypass_for_session(&self.persistence, &session_id).await;
        let pump_resolver = PermissionResolver::new(permission_mode_enum, bypass);
        tokio::spawn(async move {
            run_read_pump(
                read_half,
                pump_session,
                pump_workarea,
                pump_chat,
                pump_events,
                pump_events_replay,
                pump_io,
                pump_io_replay,
                pump_persistence,
                pump_sessions,
                pump_log,
                pump_finished,
                pump_parser,
                pump_pending,
                pump_writer,
                pump_resolver,
            )
            .await;
        });

        Ok(session_id)
    }

    /// Task 33: resolve a pending tool-approval gate. Sends the user's
    /// decision through the matching `oneshot::Sender`, persists the
    /// row, and lets the waiter task spawned in the read pump inject
    /// the bytes back into the agent's stdin.
    ///
    /// First-write-wins via the `tool_approvals` row's UPDATE guard:
    /// a second call against an already-decided row returns
    /// [`Error::Validation`] with the `tool_approval.already_resolved`
    /// wire code.
    pub async fn resolve_approval(
        &self,
        session_id: &SessionId,
        approval_id: &str,
        decision: Decision,
        decided_by_device_id: Option<&str>,
    ) -> Result<()> {
        let sender = {
            let map = self.sessions.lock().await;
            let entry = map
                .get(session_id)
                .ok_or_else(|| Error::NotFound(format!("session {session_id} not running")))?;
            let mut pending = entry.pending_approvals.lock().await;
            pending.remove(approval_id)
        };
        let sender = sender.ok_or_else(|| {
            Error::Validation(format!(
                "tool_approval.already_resolved: approval {approval_id} not pending"
            ))
        })?;

        // Persist the user decision FIRST so a crash between the
        // oneshot send and the DB write would still leave the row
        // accurate. The waiter task is responsible only for injection.
        let row_string = user_decision_string(decision);
        let now_ms = now_unix_ms();
        let rows_affected = {
            let mut writer = self.persistence.writer().await;
            let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
            let rows = concerto_persist::tool_approvals::update_decision(
                &mut tx,
                approval_id,
                row_string,
                now_ms,
                decided_by_device_id,
            )
            .await?;
            tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
            rows
        };
        if rows_affected == 0 {
            return Err(Error::Validation(format!(
                "tool_approval.already_resolved: row {approval_id} already had a decision"
            )));
        }
        // Wake the waiter; if it has died (session ended), drop is
        // benign.
        let _ = sender.send(decision);
        Ok(())
    }

    /// Task 34: revert a workarea to a checkpoint.
    ///
    /// Looks up the checkpoint row → workarea id + chat_message_id,
    /// stops every live session on the workarea, hard-resets each
    /// repo's worktree to the checkpoint's `git_ref`, and soft-deletes
    /// chat messages in the supplied `session_id`'s chat that postdate
    /// the checkpoint by overwriting `superseded_by` to the
    /// checkpoint's `chat_message_id`. V0.1 does NOT auto-restart the
    /// session — the user clicks "Start session" again per
    /// `tasks/34 §Scope — in`.
    pub async fn revert_to_checkpoint(
        &self,
        checkpoint_id: &str,
        session_id: &SessionId,
    ) -> Result<()> {
        // Look up the checkpoint to find the workarea.
        let cp = concerto_persist::checkpoints::get(self.persistence.readers(), checkpoint_id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("checkpoint {checkpoint_id} not found")))?;

        // Stop every live session on the workarea (Task 22's stop
        // semantics: kill the host, mark finished). The supervisor's
        // entry map already mirrors `sessions.ended_at IS NULL`, so we
        // enumerate the in-process map instead of the DB. The map is
        // the source of truth for "currently running".
        let live_sessions: Vec<SessionId> = {
            let map = self.sessions.lock().await;
            map.iter()
                .filter_map(|(sid, entry)| {
                    if entry.workarea_id == cp.workarea_id
                        && !entry.finished.load(std::sync::atomic::Ordering::SeqCst)
                    {
                        Some(sid.clone())
                    } else {
                        None
                    }
                })
                .collect()
        };
        for sid in &live_sessions {
            // Stop is idempotent; ignore NotFound — the session may
            // have exited between our enumeration and the stop call.
            match self.stop_session(sid, Some("revert".to_string())).await {
                Ok(_) | Err(Error::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }

        // Resolve the chat_id for the caller's session_id. The
        // soft-delete must scope to *some* chat; the design picks the
        // initiating session's chat thread. When the session is
        // unknown, fall back to the head checkpoint's chat: read it
        // back via `chat_messages`'s chat_id column would require an
        // extra helper — V0.1's revert path always has a valid
        // session_id from the gRPC caller, so the unknown-session
        // branch surfaces as NotFound.
        let chat_id =
            crate::agent_supervisor::checkpoint::chat_id_for_session(&self.persistence, session_id)
                .await?
                .ok_or_else(|| Error::NotFound(format!("session {session_id} not found")))?;

        // Hard-reset every repo in the sibling set + soft-delete the
        // post-checkpoint messages.
        let n_reset = crate::agent_supervisor::checkpoint::revert_workarea_to_checkpoint(
            &self.persistence,
            checkpoint_id,
            &chat_id,
        )
        .await?;

        tracing::info!(
            audit.kind = "revert_to_checkpoint",
            audit.session_id = %session_id,
            audit.checkpoint_id = %checkpoint_id,
            audit.workarea_id = %cp.workarea_id,
            audit.n_repos_reset = n_reset,
            audit.n_sessions_stopped = live_sessions.len(),
            "workarea reverted to checkpoint"
        );
        Ok(())
    }

    /// Test-only entry point: drive the read pump as if a parser pack
    /// had emitted [`crate::agent_supervisor::parsers::ParseEvent::TurnComplete`]
    /// for `session_id`. The supervisor takes the slow path through the
    /// same `dispatch_parse_event` branch a real turn-complete would
    /// hit, so the checkpoint plumbing is exercised end-to-end without
    /// requiring a real agent CLI that surfaces the boundary.
    ///
    /// Documented as "test-only" but exposed `pub` because gating it
    /// on `#[cfg(test)]` would block the in-process integration test
    /// (which lives in `crates/core/tests/`, not in the lib's own
    /// `cfg(test)`). Production paths never call this — there is no
    /// supported way to trigger a checkpoint without going through a
    /// `ParseEvent::TurnComplete` from a real parser pack.
    pub async fn synthesize_turn_complete(&self, session_id: &SessionId) -> Result<()> {
        let (workarea_id, chat_id) = {
            let map = self.sessions.lock().await;
            let entry = map
                .get(session_id)
                .ok_or_else(|| Error::NotFound(format!("session {session_id} not running")))?;
            (entry.workarea_id.clone(), entry.chat_id.clone())
        };
        // Inline the same logic as the TurnComplete branch in
        // `dispatch_parse_event` so the test exercises the production
        // code path (DB insert → checkpoint create → event emit).
        let persistence = Arc::clone(&self.persistence);
        let session_id = session_id.clone();
        let events_sender = {
            let map = self.sessions.lock().await;
            map.get(&session_id).map(|e| e.events.clone())
        };
        let events_replay = {
            let map = self.sessions.lock().await;
            map.get(&session_id).map(|e| Arc::clone(&e.events_replay))
        };
        let events_sender = events_sender
            .ok_or_else(|| Error::NotFound(format!("session {session_id} not running")))?;
        let events_replay = events_replay
            .ok_or_else(|| Error::NotFound(format!("session {session_id} not running")))?;

        let chat_message_id =
            crate::agent_supervisor::checkpoint::insert_turn_message(&persistence, &chat_id)
                .await?;
        let records = crate::agent_supervisor::checkpoint::create_checkpoint_for_workarea(
            &persistence,
            &workarea_id,
            &chat_message_id,
            &session_id,
        )
        .await?;
        // Mirror the TurnComplete + per-record CheckpointCreated event
        // emission so subscribers see the same wire shape.
        let turn = AgentEvent::TurnComplete {
            session_id: session_id.clone(),
        };
        push_replay(&events_replay, turn.clone()).await;
        let _ = events_sender.send(turn);
        for rec in records {
            let ev = AgentEvent::CheckpointCreated {
                session_id: session_id.clone(),
                checkpoint_id: rec.checkpoint_id,
                git_ref: rec.git_ref,
            };
            push_replay(&events_replay, ev.clone()).await;
            let _ = events_sender.send(ev);
        }
        Ok(())
    }

    /// Send a chunk of bytes as the agent's stdin (via the host's
    /// `StdinBytes` frame).
    pub async fn send_input(&self, session_id: &SessionId, data: Vec<u8>) -> Result<()> {
        let writer_arc = {
            let map = self.sessions.lock().await;
            let entry = map
                .get(session_id)
                .ok_or_else(|| Error::NotFound(format!("session {} not running", session_id)))?;
            entry.writer.clone()
        };
        let mut w = writer_arc.lock().await;
        write_frame(&mut *w, &HostFrame::StdinBytes { data })
            .await
            .map_err(|e| Error::Internal(format!("write StdinBytes: {e}")))
    }

    /// Stop a running session. Kills the host process, marks the DB row
    /// `finished`, and removes the in-process state.
    ///
    /// Idempotent: stopping a session whose host already exited
    /// (entry kept around for replay) returns `Ok(())` after evicting
    /// the entry.
    pub async fn stop_session(
        &self,
        session_id: &SessionId,
        _reason: Option<String>,
    ) -> Result<()> {
        let entry = {
            let mut map = self.sessions.lock().await;
            map.remove(session_id)
        };
        let entry =
            entry.ok_or_else(|| Error::NotFound(format!("session {} not running", session_id)))?;
        // Kill the child; the read-pump task observes EOF and exits.
        {
            let mut child_guard = entry.child.lock().await;
            if let Some(mut child) = child_guard.take() {
                let _ = child.kill().await;
            }
        }
        // Best-effort socket cleanup (the host removes it on its own
        // exit path too).
        let _ = tokio::fs::remove_file(&entry.socket_path).await;

        // Mark finished in the DB.
        let now_ms = now_unix_ms();
        let mut writer = self.persistence.writer().await;
        concerto_persist::sessions::mark_ended(&mut writer, session_id, now_ms).await?;
        entry
            .finished
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let exited = AgentEvent::Exited {
            session_id: session_id.clone(),
            exit_code: None,
            signal: None,
        };
        push_replay(&entry.events_replay, exited.clone()).await;
        let _ = entry.events.send(exited);
        // Reference the workarea_id so the field is observed (helps
        // future audit-log integration locate the workspace).
        tracing::debug!(
            session = %session_id,
            workarea = %entry.workarea_id,
            "session stopped"
        );
        Ok(())
    }

    /// Task 32: change `sessions.permission_mode` for a live session.
    ///
    /// `mode` must be one of `strict|normal|auto|yolo`. The
    /// acknowledgement string is required for `yolo`; the managed.json
    /// cap is enforced. Updates the in-memory entry's cached mode (if
    /// present) so downstream tool-approval checks (Task 33) see the
    /// new value without a DB round-trip.
    pub async fn update_session_permission_mode(
        &self,
        id: &SessionId,
        mode: &str,
        acknowledgement: &str,
    ) -> Result<()> {
        let parsed = crate::security::parse_permission_mode(mode)?;
        if parsed == crate::security::PermissionMode::Yolo
            && !crate::security::ack_for_yolo(acknowledgement)
        {
            return Err(Error::Policy(format!(
                "policy.acknowledgement_required: setting permission_mode={} requires acknowledgement={:?}",
                parsed.as_str(),
                crate::security::ACK_YOLO
            )));
        }
        let managed = crate::security::load_managed_policy(&self.config_dir);
        let _capped = crate::security::permission::enforce_managed_cap(parsed, &managed)?;

        let existing = concerto_persist::sessions::get(self.persistence.readers(), id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("session {id} not found")))?;
        let from = existing.permission_mode.clone();

        let mut writer = self.persistence.writer().await;
        let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        concerto_persist::sessions::set_permission_mode(&mut tx, id, parsed.as_str()).await?;
        tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        drop(writer);

        // Best-effort: refresh the in-memory cache so the next
        // tool-approval check sees the new value.
        {
            let mut map = self.sessions.lock().await;
            if let Some(entry) = map.get_mut(id) {
                entry.permission_mode = parsed;
            }
        }

        tracing::info!(
            audit.kind = "permission_mode_changed",
            audit.scope = "session",
            audit.session_id = %id,
            audit.from = %from,
            audit.to = %parsed.as_str(),
            audit.acknowledgement_provided = !acknowledgement.is_empty(),
            "session permission_mode changed"
        );
        Ok(())
    }

    /// Mark the session as `crashed` in the DB; used on the error path
    /// inside `start_session` when the handshake fails.
    async fn mark_failed(&self, id: &SessionId) {
        let mut writer = self.persistence.writer().await;
        let _ = concerto_persist::sessions::update_status(&mut writer, id, "crashed").await;
    }
}

/// Look up the bypass_destructive_guard flag for the session that was
/// just inserted. The walk mirrors the inheritance chain but only the
/// effective value is needed here; a follow-on Task 41/42/43 will
/// switch to calling [`crate::security::resolve_effective_mode`]
/// directly. V0.1 reads the workarea + workspace bypass directly so
/// the resolver matches what's persisted.
async fn bypass_for_session(persistence: &Persistence, session_id: &SessionId) -> bool {
    let pool = persistence.readers();
    let row = sqlx::query(
        "SELECT
            s.bypass_destructive_guard  AS s_bypass,
            wa.bypass_destructive_guard AS wa_bypass,
            ws.bypass_destructive_guard AS ws_bypass
         FROM sessions s
         JOIN workareas wa ON wa.id = s.workarea_id
         JOIN workspaces ws ON ws.id = wa.workspace_id
         WHERE s.id = ?",
    )
    .bind(&session_id.0)
    .fetch_optional(pool)
    .await;
    use sqlx::Row;
    match row {
        Ok(Some(r)) => {
            let s: i64 = r.get("s_bypass");
            if s != 0 {
                return true;
            }
            let wa: Option<i64> = r.get("wa_bypass");
            if let Some(v) = wa {
                if v != 0 {
                    return true;
                }
            }
            let ws: Option<i64> = r.get("ws_bypass");
            if let Some(v) = ws {
                if v != 0 {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// Long-running task that drains the bridge connection's read half and
/// emits `AgentEvent`s. Returns when the connection closes (`Eof`) or
/// when the host signals `AgentExited`.
#[allow(clippy::too_many_arguments)]
async fn run_read_pump(
    mut read_half: tokio::net::unix::OwnedReadHalf,
    session_id: SessionId,
    workarea_id: WorkareaId,
    chat_id: String,
    events: broadcast::Sender<AgentEvent>,
    events_replay: Arc<Mutex<Vec<AgentEvent>>>,
    io: broadcast::Sender<SessionIoChunk>,
    io_replay: Arc<Mutex<Vec<SessionIoChunk>>>,
    persistence: Arc<Persistence>,
    sessions: Arc<Mutex<HashMap<SessionId, SessionEntry>>>,
    stdout_log: PathBuf,
    finished: Arc<std::sync::atomic::AtomicBool>,
    parser: Arc<dyn ParserPack>,
    pending_approvals: Arc<Mutex<PendingApprovals>>,
    writer: Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>,
    resolver: PermissionResolver,
) {
    // Open the per-session stdout log file for append.
    let log_file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stdout_log)
        .await
        .ok();
    let log_file = log_file.map(|f| Arc::new(Mutex::new(f)));

    // Parser's accumulating buffer — V0.1 echoes the chunk back into
    // the buf and the pack drains it, but the buf is owned by the
    // pump so V1.0's partial-line accumulating packs work too.
    let mut parser_buf: Vec<u8> = Vec::new();

    loop {
        let frame = match read_frame(&mut read_half).await {
            Ok(f) => f,
            Err(FrameError::Eof) => break,
            Err(e) => {
                tracing::warn!(
                    session = %session_id,
                    error = %e,
                    "bridge read failed; ending read pump"
                );
                break;
            }
        };
        match frame {
            HostFrame::StdoutBytes { data, .. } => {
                if let Some(lf) = &log_file {
                    let mut f = lf.lock().await;
                    let _ = f.write_all(&data).await;
                    let _ = f.flush().await;
                }
                // Feed the parser. Per Task 33, the parser surfaces
                // `Bytes` (raw passthrough), `Message`, `ToolCall`,
                // `AwaitingApproval`, and `TurnComplete` events.
                parser_buf.extend_from_slice(&data);
                let parse_events = parser.parse_chunk(&mut parser_buf);
                for ev in parse_events {
                    dispatch_parse_event(
                        ev,
                        &session_id,
                        &workarea_id,
                        &chat_id,
                        &events,
                        &events_replay,
                        &io,
                        &io_replay,
                        &persistence,
                        &resolver,
                        &parser,
                        &pending_approvals,
                        &writer,
                    )
                    .await;
                }
            }
            HostFrame::StderrBytes { data, .. } => {
                // V0.1 surfaces stderr-as-assistant-message too; the
                // host never emits this frame in V0.1 (portable-pty
                // merges stderr into stdout) but the code path is
                // sound for V1.0.
                let chunk = SessionIoChunk {
                    session_id: session_id.clone(),
                    stream: "stderr",
                    data: data.clone(),
                };
                push_replay_io(&io_replay, chunk.clone()).await;
                let _ = io.send(chunk);
                let content = String::from_utf8_lossy(&data).into_owned();
                let msg = AgentEvent::Message {
                    session_id: session_id.clone(),
                    role: MessageRole::Assistant,
                    content,
                };
                push_replay(&events_replay, msg.clone()).await;
                let _ = events.send(msg);
            }
            HostFrame::AgentExited { exit_code, signal } => {
                let exited = AgentEvent::Exited {
                    session_id: session_id.clone(),
                    exit_code,
                    signal,
                };
                push_replay(&events_replay, exited.clone()).await;
                let _ = events.send(exited);
                // Mark finished in DB. Keep the in-memory entry around
                // so late subscribers can still attach (and read the
                // replay buffer) — the session is logically over, but
                // the gRPC `Streams` service may need to see the
                // recently-emitted events. The entry is dropped when
                // `stop_session` is called explicitly OR on Core
                // shutdown.
                finished.store(true, std::sync::atomic::Ordering::SeqCst);
                let now_ms = now_unix_ms();
                let mut writer = persistence.writer().await;
                let _ =
                    concerto_persist::sessions::mark_ended(&mut writer, &session_id, now_ms).await;
                drop(writer);
                let map = sessions.lock().await;
                if let Some(entry) = map.get(&session_id) {
                    // Best-effort reap the child + remove the socket
                    // file (the host removes its own socket on exit too,
                    // but be defensive). The entry stays in the map so
                    // late `Streams.Subscribe` callers can still drain
                    // the replay buffer; explicit `stop_session` is the
                    // hook that finally evicts the entry.
                    let mut child_guard = entry.child.lock().await;
                    if let Some(mut c) = child_guard.take() {
                        let _ = c.wait().await;
                    }
                    let _ = tokio::fs::remove_file(&entry.socket_path).await;
                }
                break;
            }
            HostFrame::Pong | HostFrame::Ready { .. } => {
                // Ready was consumed during handshake; a duplicate is a
                // protocol violation but we tolerate it. Pong is the
                // expected heartbeat reply once Ping is wired (Task 36).
            }
            other => {
                tracing::debug!(?other, "ignoring unexpected frame from host");
            }
        }
    }
}

/// V0.1 agent-binary resolution:
///
/// - [`AgentKind::Echo`] → spawn `/bin/echo`, with the configured
///   `echo_text` (default `"hello"`) as the agent argument.
/// - [`AgentKind::Claude`] → spawn `claude`; relies on `$PATH`.
fn resolve_agent_bin(req: &StartSessionRequest) -> Result<(String, Vec<String>)> {
    match req.agent_kind {
        AgentKind::Echo => {
            // Wrap in `sh -c` with a trailing `sleep 0.1` so the PTY
            // master has time to drain `echo`'s output before the child
            // exits. Linux's pty subsystem can race on a fast-exiting
            // child and drop the unread bytes; macOS happens to keep
            // them around. The smoke gate (Task 27) depends on this
            // output being delivered.
            // Linux's PTY subsystem can race when the child exits before
            // its output buffer has been drained by the master side —
            // the reader may see EOF before the buffered bytes. Sleep
            // 1s after the echo to give the PTY master time to read
            // everything. macOS doesn't need this but pays a 1s tax in
            // exchange for cross-OS reliability of the smoke gate.
            let payload = req.echo_text.clone().unwrap_or_else(|| "hello".to_string());
            let script = format!("echo {}; sleep 1", shell_escape_single_quoted(&payload));
            Ok(("/bin/sh".to_string(), vec!["-c".to_string(), script]))
        }
        AgentKind::Claude => Ok(("claude".to_string(), Vec::new())),
        AgentKind::Codex | AgentKind::Gemini => Err(Error::Validation(
            "agent.not_implemented: codex/gemini deferred to Phase 3".to_string(),
        )),
    }
}

/// Wrap `s` in single quotes for `/bin/sh -c`, escaping any embedded
/// quotes via `'\''`. Used by the V0.1 echo agent path so user-supplied
/// `echo_text` cannot break out of the wrapping `sh -c` script.
fn shell_escape_single_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Resolve the effective permission mode for a session about to be
/// inserted on `workarea_id` (no `sessions` row exists yet). Walks
/// workarea → workspace → project → managed → default, identical to
/// [`crate::security::resolve_effective_mode`] but starting one level
/// up the chain.
async fn resolve_for_new_session(
    persistence: &Persistence,
    config_dir: &std::path::Path,
    workarea_id: &WorkareaId,
) -> Result<crate::security::EffectiveMode> {
    use crate::security::{
        load_managed_policy, parse_permission_mode, EffectiveMode, ModeSource, PermissionMode,
    };
    let pool = persistence.readers();
    let row = sqlx::query(
        "SELECT
            wa.permission_mode         AS workarea_mode,
            wa.bypass_destructive_guard AS workarea_bypass,
            ws.permission_mode         AS workspace_mode,
            ws.bypass_destructive_guard AS workspace_bypass,
            p.settings_json            AS project_settings_json
         FROM workareas wa
         JOIN workspaces ws ON ws.id = wa.workspace_id
         JOIN projects p    ON p.id  = ws.project_id
         WHERE wa.id = ?",
    )
    .bind(&workarea_id.0)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?
    .ok_or_else(|| Error::NotFound(format!("workarea {workarea_id} not found")))?;

    use sqlx::Row;
    let workarea_mode: Option<String> = row.get("workarea_mode");
    let workspace_mode: Option<String> = row.get("workspace_mode");
    let project_settings_json: String = row.get("project_settings_json");

    let (mut mode, mut source) = if let Some(m) = workarea_mode.as_deref() {
        (parse_permission_mode(m)?, ModeSource::Workarea)
    } else if let Some(m) = workspace_mode.as_deref() {
        (parse_permission_mode(m)?, ModeSource::Workspace)
    } else {
        // Inline of `project_default_from_settings` (private to
        // security::permission). Forgive malformed JSON by falling
        // through to default.
        let project_default: Option<PermissionMode> =
            match serde_json::from_str::<serde_json::Value>(&project_settings_json) {
                Ok(v) => v
                    .as_object()
                    .and_then(|m| m.get("default_permission_mode"))
                    .and_then(|x| x.as_str())
                    .map(parse_permission_mode)
                    .transpose()?,
                Err(_) => None,
            };
        match project_default {
            Some(m) => (m, ModeSource::Project),
            None => (PermissionMode::Normal, ModeSource::Default),
        }
    };

    let workarea_bypass: Option<i64> = row.get("workarea_bypass");
    let workspace_bypass: Option<i64> = row.get("workspace_bypass");
    let mut bypass = if let Some(b) = workarea_bypass {
        b != 0
    } else if let Some(b) = workspace_bypass {
        b != 0
    } else {
        false
    };

    let managed = load_managed_policy(config_dir);
    if let Some(cap) = managed.max_permission_mode {
        if mode.rank() > cap.rank() {
            mode = cap;
            source = ModeSource::Managed;
        }
    }
    if !managed.allow_yolo && mode == PermissionMode::Yolo {
        mode = PermissionMode::Auto;
        source = ModeSource::Managed;
    }
    if !managed.allow_bypass_destructive_guard && bypass {
        bypass = false;
    }

    Ok(EffectiveMode {
        mode,
        bypass_destructive_guard: bypass,
        source,
    })
}

/// Task 33: route a single [`ParseEvent`] coming off the parser pack to
/// the right sink (broadcasts, persistence, approval wait-loop).
///
/// All arguments are short-lived borrows so the function can be called
/// from inside `run_read_pump`'s per-frame loop without bumping
/// reference counts.
#[allow(clippy::too_many_arguments)]
async fn dispatch_parse_event(
    ev: ParseEvent,
    session_id: &SessionId,
    workarea_id: &WorkareaId,
    chat_id: &str,
    events: &broadcast::Sender<AgentEvent>,
    events_replay: &Arc<Mutex<Vec<AgentEvent>>>,
    io: &broadcast::Sender<SessionIoChunk>,
    io_replay: &Arc<Mutex<Vec<SessionIoChunk>>>,
    persistence: &Arc<Persistence>,
    resolver: &PermissionResolver,
    parser: &Arc<dyn ParserPack>,
    pending_approvals: &Arc<Mutex<PendingApprovals>>,
    writer: &Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>,
) {
    match ev {
        ParseEvent::Bytes(data) => {
            let chunk = SessionIoChunk {
                session_id: session_id.clone(),
                stream: "stdout",
                data,
            };
            push_replay_io(io_replay, chunk.clone()).await;
            let _ = io.send(chunk);
        }
        ParseEvent::Message { role, content } => {
            let msg = AgentEvent::Message {
                session_id: session_id.clone(),
                role: map_msg_role(role),
                content,
            };
            push_replay(events_replay, msg.clone()).await;
            let _ = events.send(msg);
        }
        ParseEvent::ToolCall {
            name,
            args,
            call_id,
        } => {
            let ev = AgentEvent::ToolCall {
                session_id: session_id.clone(),
                call_id,
                name,
                args_json: args.to_string(),
            };
            push_replay(events_replay, ev.clone()).await;
            let _ = events.send(ev);
        }
        ParseEvent::TurnComplete => {
            let ev = AgentEvent::TurnComplete {
                session_id: session_id.clone(),
            };
            push_replay(events_replay, ev.clone()).await;
            let _ = events.send(ev);
            // Task 34: at every turn boundary, snapshot the worktree
            // into a per-repo checkpoint ref + DB row, then emit one
            // AgentEvent::CheckpointCreated per ref. The checkpoint
            // creation runs on a spawned task so a slow git operation
            // doesn't block the read pump from draining the next
            // frame; failures are logged at WARN and the read pump
            // keeps going (best-effort per `tasks/34 §Scope — in`).
            let persistence = Arc::clone(persistence);
            let workarea_id = workarea_id.clone();
            let chat_id = chat_id.to_string();
            let session_id = session_id.clone();
            let events_for_checkpoint = events.clone();
            let events_replay_for_checkpoint = Arc::clone(events_replay);
            tokio::spawn(async move {
                let chat_message_id =
                    match crate::agent_supervisor::checkpoint::insert_turn_message(
                        &persistence,
                        &chat_id,
                    )
                    .await
                    {
                        Ok(id) => id,
                        Err(e) => {
                            tracing::warn!(
                                session = %session_id,
                                error = %e,
                                "checkpoint: failed to insert turn marker"
                            );
                            return;
                        }
                    };
                let records =
                    match crate::agent_supervisor::checkpoint::create_checkpoint_for_workarea(
                        &persistence,
                        &workarea_id,
                        &chat_message_id,
                        &session_id,
                    )
                    .await
                    {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!(
                                session = %session_id,
                                workarea = %workarea_id,
                                error = %e,
                                "checkpoint: create_checkpoint_for_workarea failed"
                            );
                            return;
                        }
                    };
                for rec in records {
                    let ev = AgentEvent::CheckpointCreated {
                        session_id: session_id.clone(),
                        checkpoint_id: rec.checkpoint_id,
                        git_ref: rec.git_ref,
                    };
                    push_replay(&events_replay_for_checkpoint, ev.clone()).await;
                    let _ = events_for_checkpoint.send(ev);
                }
            });
        }
        ParseEvent::AwaitingApproval {
            tool,
            summary,
            payload,
        } => {
            // Resolve the verdict from the cached permission mode.
            let decision = resolver.decide(&tool);
            let approval_id = uuid::Uuid::now_v7().to_string();
            let now_ms = now_unix_ms();
            let payload_json = payload.to_string();

            match decision {
                Decision::AutoApprove | Decision::AutoApproveOnce | Decision::AutoDeny => {
                    // Persist the auto-row up front + inject the bytes
                    // right back into the agent's stdin.
                    let auto_string = resolver.auto_decision_string().to_string();
                    let row = concerto_persist::tool_approvals::NewToolApproval {
                        id: approval_id.clone(),
                        session_id: session_id.clone(),
                        tool_name: tool.clone(),
                        payload_json: payload_json.clone(),
                        requested_at: now_ms,
                        decision: Some(auto_string.clone()),
                        decided_at: Some(now_ms),
                        decided_by_device_id: None,
                    };
                    if let Ok(mut w) = persistence.writer().await.begin().await {
                        let _ = concerto_persist::tool_approvals::insert(&mut w, row).await;
                        let _ = w.commit().await;
                    }
                    let bytes = parser.inject_approval(decision);
                    if !bytes.is_empty() {
                        let mut w = writer.lock().await;
                        let _ = write_frame(&mut *w, &HostFrame::StdinBytes { data: bytes }).await;
                    }
                    let resolved = AgentEvent::ApprovalResolved {
                        session_id: session_id.clone(),
                        approval_id,
                        tool,
                        decision: auto_string,
                    };
                    push_replay(events_replay, resolved.clone()).await;
                    let _ = events.send(resolved);
                }
                Decision::MustAsk => {
                    // Persist the pending row + create the oneshot
                    // pairing, then emit AwaitingApproval and park a
                    // waiter task that injects the bytes once the
                    // user resolves.
                    let row = concerto_persist::tool_approvals::NewToolApproval {
                        id: approval_id.clone(),
                        session_id: session_id.clone(),
                        tool_name: tool.clone(),
                        payload_json: payload_json.clone(),
                        requested_at: now_ms,
                        decision: None,
                        decided_at: None,
                        decided_by_device_id: None,
                    };
                    if let Ok(mut w) = persistence.writer().await.begin().await {
                        let _ = concerto_persist::tool_approvals::insert(&mut w, row).await;
                        let _ = w.commit().await;
                    }
                    let (tx, rx) = tokio::sync::oneshot::channel::<Decision>();
                    {
                        let mut pending = pending_approvals.lock().await;
                        pending.insert(approval_id.clone(), tx);
                    }
                    let awaiting = AgentEvent::AwaitingApproval {
                        session_id: session_id.clone(),
                        approval_id: approval_id.clone(),
                        tool: tool.clone(),
                        summary,
                        payload_json,
                    };
                    push_replay(events_replay, awaiting.clone()).await;
                    let _ = events.send(awaiting);

                    // Park the waiter on a spawned task — when the
                    // client resolves, write the matching injection
                    // bytes and emit ApprovalResolved.
                    let parser = Arc::clone(parser);
                    let writer = Arc::clone(writer);
                    let events_for_waiter = events.clone();
                    let events_replay_for_waiter = Arc::clone(events_replay);
                    let session_id_for_waiter = session_id.clone();
                    let tool_for_waiter = tool;
                    tokio::spawn(async move {
                        match rx.await {
                            Ok(d) => {
                                let bytes = parser.inject_approval(d);
                                if !bytes.is_empty() {
                                    let mut w = writer.lock().await;
                                    let _ = write_frame(
                                        &mut *w,
                                        &HostFrame::StdinBytes { data: bytes },
                                    )
                                    .await;
                                }
                                let row_str = user_decision_string(d);
                                let resolved = AgentEvent::ApprovalResolved {
                                    session_id: session_id_for_waiter,
                                    approval_id,
                                    tool: tool_for_waiter,
                                    decision: row_str.to_string(),
                                };
                                push_replay(&events_replay_for_waiter, resolved.clone()).await;
                                let _ = events_for_waiter.send(resolved);
                            }
                            Err(_) => {
                                // Sender dropped — session ended.
                            }
                        }
                    });
                }
            }
        }
    }
}

fn map_msg_role(r: MsgRole) -> MessageRole {
    match r {
        MsgRole::Assistant => MessageRole::Assistant,
        MsgRole::User => MessageRole::User,
        MsgRole::System => MessageRole::System,
        MsgRole::Tool => MessageRole::Tool,
    }
}

/// Append `ev` to the per-session replay buffer, evicting the oldest
/// entry when the buffer is at capacity. The buffer is short by design
/// (see [`MAX_REPLAY_EVENTS`]) — V1.0 grows this into the per-subject
/// ring buffer + `since_offset` semantics from `design/10 §3.3`.
async fn push_replay(buf: &Arc<Mutex<Vec<AgentEvent>>>, ev: AgentEvent) {
    let mut v = buf.lock().await;
    if v.len() >= MAX_REPLAY_EVENTS {
        v.remove(0);
    }
    v.push(ev);
}

async fn push_replay_io(buf: &Arc<Mutex<Vec<SessionIoChunk>>>, chunk: SessionIoChunk) {
    let mut v = buf.lock().await;
    if v.len() >= MAX_REPLAY_IO {
        v.remove(0);
    }
    v.push(chunk);
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Supervised actor wrapper. Mirrors the other manager actors: the
/// `run` future parks on shutdown; the meaningful surface is the
/// cheap-to-clone [`AgentSupervisorHandle`].
pub struct AgentSupervisorActor {
    handle: AgentSupervisorHandle,
}

impl AgentSupervisorActor {
    pub fn new(
        persistence: Arc<Persistence>,
        data_dir: Arc<PathBuf>,
        config_dir: Arc<PathBuf>,
        host_bin: PathBuf,
    ) -> Self {
        Self {
            handle: AgentSupervisorHandle::new(persistence, data_dir, config_dir, host_bin),
        }
    }

    /// Cheap clone of the shared handle.
    pub fn handle(&self) -> AgentSupervisorHandle {
        self.handle.clone()
    }
}

#[async_trait]
impl Actor for AgentSupervisorActor {
    const NAME: &'static str = "agent-supervisor";
    type Config = AgentSupervisorConfig;

    async fn run(self, ctx: ActorContext<Self::Config>) -> Result<()> {
        tracing::info!("AgentSupervisor ready");
        ctx.shutdown.cancelled().await;
        tracing::debug!("AgentSupervisor actor shutting down");
        Ok(())
    }
}

// Silence the dead-field warning for the test-only socket field stash —
// keeping `_` around the read on the type so it does not become public
// surface.
#[allow(dead_code)]
fn _hint_path_field(p: &Path) -> &Path {
    p
}
