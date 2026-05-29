//! Task 36: hot reconnect — adopt orphan `concerto-agent-host` processes
//! across a Core restart.
//!
//! After a Core restart (clean or crashed), `concerto-agent-host`
//! processes detached via `setsid()` keep running, owning the PTYs and
//! buffering output in their 1 MiB ring buffers. This module is the
//! Core-side counterpart that, at boot, finds those orphans and
//! re-attaches the bridge.
//!
//! ## Strategy
//!
//! 1. Scan `<data_dir>/runtime/agents/*.sock` for every UDS the host
//!    layer leaves behind on a live session.
//! 2. For each socket, look up the matching `sessions` row by
//!    `host_socket` and read `pty_cookie` + `last_acked_seq`.
//! 3. Connect to the UDS; send `HostFrame::Hello { cookie,
//!    last_seq = last_acked_seq }`.
//! 4. On `Ready { last_seq, .. }`: persist `host_pid` (if available),
//!    set status back to `'running'`, register a fresh `SessionEntry`
//!    in the in-memory map, and re-spawn the bridge read-pump. The
//!    host's replay flows through the same parser pack the freshly-
//!    started path would have used.
//! 5. On `CookieMismatch` / `AlreadyConnected` / decode error / no
//!    DB row: log + mark the session `'crashed'` and remove the
//!    stale socket file. (A crashed host is Task 37's cold-resume
//!    territory; here we only handle hosts that *survived*.)
//!
//! `adopt_orphans` runs *after* the supervisor actor has been spawned
//! into the runtime supervision tree, but *before* the gRPC server
//! accepts traffic (`design/01 §6.3`, `design/04 §6.4`). This ordering
//! is the responsibility of `main.rs`.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use concerto_error::Result;
use concerto_persist::SessionId;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

use crate::agent_supervisor::actor::{adopt_resume_session, AgentSupervisorHandle};
use crate::agent_supervisor::bridge::{build_hello, read_frame, write_frame, HostFrame};

/// Adopt every surviving `concerto-agent-host` whose UDS still lives
/// under `<data_dir>/runtime/agents/`. Returns the number of sessions
/// successfully re-attached.
///
/// On any per-socket failure the function logs and continues; only
/// catastrophic errors (e.g. failing to read the runtime directory at
/// all) surface as `Err`.
pub async fn adopt_orphans(handle: &AgentSupervisorHandle) -> Result<usize> {
    let runtime_dir = handle.data_dir().join("runtime").join("agents");
    if !tokio::fs::try_exists(&runtime_dir).await.unwrap_or(false) {
        tracing::debug!(
            runtime_dir = %runtime_dir.display(),
            "adopt_orphans: runtime dir absent; nothing to adopt"
        );
        return Ok(0);
    }

    let mut read_dir = match tokio::fs::read_dir(&runtime_dir).await {
        Ok(rd) => rd,
        Err(e) => {
            tracing::warn!(
                runtime_dir = %runtime_dir.display(),
                error = %e,
                "adopt_orphans: failed to read runtime dir"
            );
            return Ok(0);
        }
    };

    let mut sockets: Vec<PathBuf> = Vec::new();
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("sock") {
            sockets.push(path);
        }
    }

    let mut adopted = 0usize;
    for socket in sockets {
        match try_adopt_one(handle, &socket).await {
            Ok(true) => {
                adopted += 1;
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(
                    socket = %socket.display(),
                    error = %e,
                    "adopt_orphans: unexpected error during adoption attempt"
                );
            }
        }
    }
    tracing::info!(adopted, "adopt_orphans: sweep complete");
    Ok(adopted)
}

/// Try to adopt a single socket. Returns `Ok(true)` on success,
/// `Ok(false)` when the socket was deliberately rejected (cookie
/// mismatch, no DB row, etc.); `Err` only on infrastructure failures
/// the caller should surface.
async fn try_adopt_one(handle: &AgentSupervisorHandle, socket: &std::path::Path) -> Result<bool> {
    let socket_str = socket.to_string_lossy().into_owned();
    // Look up the session by host_socket. Use the readers pool — the
    // adoption sweep runs before any gRPC traffic so no contention.
    let row = match find_session_by_socket(handle, &socket_str).await? {
        Some(r) => r,
        None => {
            tracing::info!(
                socket = %socket.display(),
                "adopt_orphans: no session row matches socket; removing stale file"
            );
            let _ = tokio::fs::remove_file(socket).await;
            return Ok(false);
        }
    };

    let session_id = row.id.clone();
    let cookie_vec = match row.pty_cookie.clone() {
        Some(c) if c.len() == 32 => c,
        _ => {
            tracing::warn!(
                session = %session_id,
                socket = %socket.display(),
                "adopt_orphans: session row missing/malformed cookie; marking crashed"
            );
            mark_crashed(handle, &session_id).await;
            let _ = tokio::fs::remove_file(socket).await;
            return Ok(false);
        }
    };
    let mut cookie = [0u8; 32];
    cookie.copy_from_slice(&cookie_vec);
    let last_acked_seq = row.last_acked_seq.max(0) as u64;

    // Connect + send Hello.
    let stream = match UnixStream::connect(socket).await {
        Ok(s) => s,
        Err(e) => {
            tracing::info!(
                session = %session_id,
                socket = %socket.display(),
                error = %e,
                "adopt_orphans: host UDS not connectable; marking crashed (host died)"
            );
            mark_crashed(handle, &session_id).await;
            let _ = tokio::fs::remove_file(socket).await;
            return Ok(false);
        }
    };
    let (mut read_half, mut write_half) = stream.into_split();
    let hello = build_hello(env!("CARGO_PKG_VERSION"), cookie, last_acked_seq);
    if let Err(e) = write_frame(&mut write_half, &hello).await {
        tracing::warn!(
            session = %session_id,
            socket = %socket.display(),
            error = %e,
            "adopt_orphans: Hello write failed; marking crashed"
        );
        let _ = write_half.shutdown().await;
        mark_crashed(handle, &session_id).await;
        return Ok(false);
    }

    let ready = match read_frame(&mut read_half).await {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(
                session = %session_id,
                socket = %socket.display(),
                error = %e,
                "adopt_orphans: Ready read failed; marking crashed"
            );
            mark_crashed(handle, &session_id).await;
            return Ok(false);
        }
    };
    match ready {
        HostFrame::Ready { .. } => {}
        HostFrame::CookieMismatch => {
            tracing::warn!(
                session = %session_id,
                socket = %socket.display(),
                "adopt_orphans: host reported CookieMismatch; marking crashed"
            );
            mark_crashed(handle, &session_id).await;
            return Ok(false);
        }
        HostFrame::AlreadyConnected => {
            tracing::warn!(
                session = %session_id,
                socket = %socket.display(),
                "adopt_orphans: host reports another Core is connected; skipping"
            );
            return Ok(false);
        }
        other => {
            tracing::warn!(
                session = %session_id,
                socket = %socket.display(),
                ?other,
                "adopt_orphans: unexpected handshake frame; marking crashed"
            );
            mark_crashed(handle, &session_id).await;
            return Ok(false);
        }
    }

    // Re-register the session in the supervisor's in-memory map and
    // spawn a fresh read pump. The helper lives in `actor.rs` so it
    // can construct `SessionEntry` without exposing its private
    // fields.
    if let Err(e) =
        adopt_resume_session(handle, &row, cookie, read_half, write_half, last_acked_seq).await
    {
        tracing::warn!(
            session = %session_id,
            error = %e,
            "adopt_orphans: failed to re-attach bridge; marking crashed"
        );
        mark_crashed(handle, &session_id).await;
        return Ok(false);
    }

    // Best-effort: bump status back to running. start_session already
    // sets it on first connect; on adoption the row may say `running`
    // (clean restart) or `starting` (we died mid-handshake last
    // boot). Either way push it to running explicitly.
    let persistence = handle.persistence();
    let mut w = persistence.writer().await;
    let _ = concerto_persist::sessions::update_status(&mut w, &session_id, "running").await;
    drop(w);

    tracing::info!(
        session = %session_id,
        socket = %socket.display(),
        last_acked_seq,
        "adopt_orphans: session re-attached"
    );
    Ok(true)
}

/// Look up the session row whose `host_socket` matches `socket_str`,
/// among rows whose `ended_at IS NULL` (active sessions only —
/// `mark_ended` runs at clean shutdown so a finished session has no
/// host process to adopt). Returns the row if exactly one matches.
async fn find_session_by_socket(
    handle: &AgentSupervisorHandle,
    socket_str: &str,
) -> Result<Option<concerto_persist::Session>> {
    let persistence = handle.persistence();
    let pool = persistence.readers();
    let row = sqlx::query_as::<_, RawSession>(
        "SELECT id, workarea_id, chat_id, agent_kind, agent_version, model, mode,
                host_pid, host_socket, pty_cookie, external_session_id,
                permission_mode, bypass_destructive_guard,
                started_at, ended_at, last_heartbeat, status, last_acked_seq
         FROM sessions
         WHERE host_socket = ? AND ended_at IS NULL
         LIMIT 1",
    )
    .bind(socket_str)
    .fetch_optional(pool)
    .await
    .map_err(|e| concerto_error::Error::Sqlx(Box::new(e)))?;
    Ok(row.map(Into::into))
}

/// Mirror of the `sessions` row shape so `sqlx::query_as` can hydrate
/// without depending on the row mapping function being `pub`.
#[derive(sqlx::FromRow)]
struct RawSession {
    id: String,
    workarea_id: String,
    chat_id: String,
    agent_kind: String,
    agent_version: Option<String>,
    model: Option<String>,
    mode: Option<String>,
    host_pid: Option<i64>,
    host_socket: Option<String>,
    pty_cookie: Option<Vec<u8>>,
    external_session_id: Option<String>,
    permission_mode: String,
    bypass_destructive_guard: i64,
    started_at: i64,
    ended_at: Option<i64>,
    last_heartbeat: Option<i64>,
    status: String,
    last_acked_seq: i64,
}

impl From<RawSession> for concerto_persist::Session {
    fn from(r: RawSession) -> Self {
        concerto_persist::Session {
            id: concerto_persist::SessionId(r.id),
            workarea_id: concerto_persist::WorkareaId(r.workarea_id),
            chat_id: r.chat_id,
            agent_kind: r.agent_kind,
            agent_version: r.agent_version,
            model: r.model,
            mode: r.mode,
            host_pid: r.host_pid,
            host_socket: r.host_socket,
            pty_cookie: r.pty_cookie,
            external_session_id: r.external_session_id,
            permission_mode: r.permission_mode,
            bypass_destructive_guard: r.bypass_destructive_guard != 0,
            started_at: r.started_at,
            ended_at: r.ended_at,
            last_heartbeat: r.last_heartbeat,
            status: r.status,
            last_acked_seq: r.last_acked_seq,
        }
    }
}

async fn mark_crashed(handle: &AgentSupervisorHandle, id: &SessionId) {
    let persistence = handle.persistence();
    let mut w = persistence.writer().await;
    if let Err(e) = concerto_persist::sessions::update_status(&mut w, id, "crashed").await {
        tracing::warn!(
            session = %id,
            error = %e,
            "adopt_orphans: failed to mark session crashed"
        );
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let _ = concerto_persist::sessions::mark_ended(&mut w, id, now_ms).await;
}

// Silence unused-import lint when only the re-export is used at the
// module root.
#[allow(dead_code)]
fn _hint_arc(a: &Arc<()>) -> &Arc<()> {
    a
}
