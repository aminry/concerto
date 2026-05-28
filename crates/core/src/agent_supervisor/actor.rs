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

use crate::agent_supervisor::bridge::{
    build_hello, read_frame, write_frame, FrameError, HostFrame,
};
use crate::agent_supervisor::events::{AgentEvent, MessageRole};
use crate::agent_supervisor::spawn::{spawn_host, wait_for_socket, SOCKET_POLL_BUDGET};
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

/// Per-session in-process state held by the supervisor.
struct SessionEntry {
    workarea_id: WorkareaId,
    /// 32-byte cookie issued at spawn time. Held in process only — the
    /// schema does not include a slot for it on `sessions` in V0.1, so
    /// the supervisor uses this map for `send_input` /  cookie-aware
    /// reconnect work that lands in Task 36.
    #[allow(dead_code)]
    cookie: [u8; 32],
    /// Per-session UDS path.
    socket_path: PathBuf,
    /// Broadcast sender — subscribers receive [`AgentEvent`].
    events: broadcast::Sender<AgentEvent>,
    /// Writer half of the bridge connection; held under a mutex so
    /// `send_input` can serialize stdin writes.
    writer: Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>,
    /// Child handle. Held under a mutex so `stop_session` can `kill`
    /// without racing the read-loop task.
    child: Arc<Mutex<Option<Child>>>,
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
    pub fn new(persistence: Arc<Persistence>, data_dir: Arc<PathBuf>, host_bin: PathBuf) -> Self {
        Self {
            persistence,
            data_dir,
            host_bin: Arc::new(host_bin),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Subscribe to the per-session [`AgentEvent`] broadcast. Returns
    /// `None` if the session is unknown (e.g. already stopped).
    pub async fn subscribe_events(
        &self,
        session_id: &SessionId,
    ) -> Option<broadcast::Receiver<AgentEvent>> {
        let map = self.sessions.lock().await;
        map.get(session_id).map(|e| e.events.subscribe())
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
        let permission_mode = req
            .permission_mode
            .clone()
            .unwrap_or_else(|| "normal".to_string());
        validate_permission_mode(&permission_mode)?;
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
        let writer_arc = Arc::new(Mutex::new(write_half));
        let child_arc = Arc::new(Mutex::new(Some(child)));
        {
            let mut map = self.sessions.lock().await;
            map.insert(
                session_id.clone(),
                SessionEntry {
                    workarea_id: req.workarea_id.clone(),
                    cookie,
                    socket_path: socket_path.clone(),
                    events: events.clone(),
                    writer: writer_arc.clone(),
                    child: child_arc.clone(),
                },
            );
        }

        // Started event, before launching the read pump so any
        // subscriber registered after `start_session` returns sees an
        // already-running session.
        let _ = events.send(AgentEvent::Started {
            session_id: session_id.clone(),
        });

        let pump_persistence = Arc::clone(&self.persistence);
        let pump_session = session_id.clone();
        let pump_events = events.clone();
        let pump_sessions = Arc::clone(&self.sessions);
        let pump_log = stdout_log.clone();
        tokio::spawn(async move {
            run_read_pump(
                read_half,
                pump_session,
                pump_events,
                pump_persistence,
                pump_sessions,
                pump_log,
            )
            .await;
        });

        Ok(session_id)
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
        let _ = entry.events.send(AgentEvent::Exited {
            session_id: session_id.clone(),
            exit_code: None,
            signal: None,
        });
        // Reference the workarea_id so the field is observed (helps
        // future audit-log integration locate the workspace).
        tracing::debug!(
            session = %session_id,
            workarea = %entry.workarea_id,
            "session stopped"
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

/// Long-running task that drains the bridge connection's read half and
/// emits `AgentEvent`s. Returns when the connection closes (`Eof`) or
/// when the host signals `AgentExited`.
async fn run_read_pump(
    mut read_half: tokio::net::unix::OwnedReadHalf,
    session_id: SessionId,
    events: broadcast::Sender<AgentEvent>,
    persistence: Arc<Persistence>,
    sessions: Arc<Mutex<HashMap<SessionId, SessionEntry>>>,
    stdout_log: PathBuf,
) {
    // Open the per-session stdout log file for append.
    let log_file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stdout_log)
        .await
        .ok();
    let log_file = log_file.map(|f| Arc::new(Mutex::new(f)));

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
                let content = String::from_utf8_lossy(&data).into_owned();
                let _ = events.send(AgentEvent::Message {
                    session_id: session_id.clone(),
                    role: MessageRole::Assistant,
                    content,
                });
            }
            HostFrame::StderrBytes { data, .. } => {
                // V0.1 surfaces stderr-as-assistant-message too; the
                // host never emits this frame in V0.1 (portable-pty
                // merges stderr into stdout) but the code path is
                // sound for V1.0.
                let content = String::from_utf8_lossy(&data).into_owned();
                let _ = events.send(AgentEvent::Message {
                    session_id: session_id.clone(),
                    role: MessageRole::Assistant,
                    content,
                });
            }
            HostFrame::AgentExited { exit_code, signal } => {
                let _ = events.send(AgentEvent::Exited {
                    session_id: session_id.clone(),
                    exit_code,
                    signal,
                });
                // Mark finished in DB + remove from map.
                let now_ms = now_unix_ms();
                let mut writer = persistence.writer().await;
                let _ =
                    concerto_persist::sessions::mark_ended(&mut writer, &session_id, now_ms).await;
                drop(writer);
                let mut map = sessions.lock().await;
                if let Some(entry) = map.remove(&session_id) {
                    let _ = tokio::fs::remove_file(&entry.socket_path).await;
                    let mut child_guard = entry.child.lock().await;
                    if let Some(mut c) = child_guard.take() {
                        let _ = c.wait().await;
                    }
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
            let payload = req.echo_text.clone().unwrap_or_else(|| "hello".to_string());
            Ok(("/bin/echo".to_string(), vec![payload]))
        }
        AgentKind::Claude => Ok(("claude".to_string(), Vec::new())),
        AgentKind::Codex | AgentKind::Gemini => Err(Error::Validation(
            "agent.not_implemented: codex/gemini deferred to Phase 3".to_string(),
        )),
    }
}

fn validate_permission_mode(mode: &str) -> Result<()> {
    match mode {
        "strict" | "normal" | "auto" | "yolo" => Ok(()),
        other => Err(Error::Validation(format!(
            "permission_mode {other:?} must be one of strict|normal|auto|yolo"
        ))),
    }
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
    pub fn new(persistence: Arc<Persistence>, data_dir: Arc<PathBuf>, host_bin: PathBuf) -> Self {
        Self {
            handle: AgentSupervisorHandle::new(persistence, data_dir, host_bin),
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
