//! Task 37: cold resume from agent JSONL.
//!
//! When the `concerto-agent-host` is gone too (machine reboot, host
//! OOM-killed), the Core no longer has a UDS to reconnect to and Task
//! 36's `adopt_orphans` will have marked the session `crashed`. This
//! module's [`cold_resume_session`] re-spawns the host with
//! `--resume-jsonl <external_session_id>` so the wrapped agent CLI
//! (Claude / Codex) loads its own on-disk conversation transcript and
//! picks up where it left off.
//!
//! The cold-resume path REUSES the original `sessions` row: the
//! row's `host_pid`, `host_socket`, `pty_cookie`, `status`, and
//! `last_acked_seq` columns are rewritten in place so the session id
//! stays stable across the cold cycle. `external_session_id` is
//! preserved.
//!
//! ## Auto-resume gating
//!
//! Cold resume is OPT-IN per project. Each `projects` row carries a
//! `settings_json` blob; setting `{"auto_resume_agents": true}` makes
//! the cold sweep in `adopt_orphans` call [`maybe_auto_resume`] for
//! every crashed session belonging to that project. The default is
//! `false` so a hostile or stale `external_session_id` doesn't get
//! relaunched without explicit user consent.
//!
//! ## Failure modes
//!
//! - **No `external_session_id`**: returns [`concerto_error::Error::NotFound`]
//!   tagged with the wire code `session.no_external_id`. The caller (the
//!   gRPC handler, the auto-resume sweep) can surface this distinctly
//!   from "session does not exist".
//! - **Workarea archived / missing**: surfaces as the normal
//!   `start_session` `NotFound` / `Validation` error.
//! - **Spawn / handshake failure**: surfaces the underlying error; the
//!   session row is left at `'crashed'` by
//!   [`AgentSupervisorHandle::cold_resume_existing`]'s error paths.

#![cfg(unix)]

use std::path::PathBuf;

use concerto_error::{Error, Result};
use concerto_persist::SessionId;

use crate::agent_supervisor::actor::AgentSupervisorHandle;

/// Cold-resume a single session by spawning a fresh `concerto-agent-host`
/// with `--resume-jsonl <external_session_id>`. Returns the input
/// [`SessionId`] (cold resume reuses the row).
///
/// The row must already carry an `external_session_id`. Sessions whose
/// parser never extracted one error with `NotFound` and the wire code
/// `session.no_external_id`; the caller is expected to either start a
/// fresh session or wait for the parser to catch up.
pub async fn cold_resume_session(
    handle: &AgentSupervisorHandle,
    session_id: &SessionId,
) -> Result<SessionId> {
    let persistence = handle.persistence();

    // Look up the row.
    let row = concerto_persist::sessions::get(persistence.readers(), session_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("session {session_id} not found")))?;
    let token = row.external_session_id.clone().ok_or_else(|| {
        Error::NotFound(format!(
            "session.no_external_id: session {session_id} has no external_session_id; \
             cannot cold-resume (parser never extracted a Claude/Codex token)"
        ))
    })?;

    // Resolve the workarea's worktree root for the new host's cwd. The
    // supervisor doesn't have a `WorkareaManager` plumbed through; read
    // the row directly. The workarea is the same one that owned the
    // original session, so its `worktree_root` is the canonical cwd.
    let workarea = concerto_persist::workareas::get(persistence.readers(), &row.workarea_id)
        .await?
        .ok_or_else(|| {
            Error::NotFound(format!(
                "workarea {} not found (cold-resume target)",
                row.workarea_id
            ))
        })?;
    if workarea.archived_at.is_some() {
        return Err(Error::Validation(format!(
            "workarea.archived: workarea {} is archived; cannot cold-resume",
            row.workarea_id
        )));
    }
    let cwd = PathBuf::from(&workarea.worktree_root);

    handle.cold_resume_existing(session_id, cwd, &token).await
}

/// Cold-resume `session_id` IF the project setting
/// `auto_resume_agents` is true. Returns `Ok(true)` when the resume
/// was attempted (and `start_session` succeeded), `Ok(false)` when the
/// setting is off (or unreadable) and the session is left in
/// `crashed`, and `Err` on infrastructure failures the caller should
/// surface.
///
/// Called by the cold-path branch of `adopt_orphans` after the host's
/// UDS has been determined to be unreachable.
pub async fn maybe_auto_resume(
    handle: &AgentSupervisorHandle,
    session_id: &SessionId,
) -> Result<bool> {
    let persistence = handle.persistence();
    let enabled = read_auto_resume_for_session(handle, session_id).await;
    if !enabled {
        tracing::debug!(
            session = %session_id,
            "maybe_auto_resume: project setting auto_resume_agents=false; leaving crashed"
        );
        return Ok(false);
    }
    // Only meaningful when the session has an external_session_id —
    // otherwise cold_resume_session will error with
    // `session.no_external_id` and we'd spam the log on every boot.
    let row = match concerto_persist::sessions::get(persistence.readers(), session_id).await? {
        Some(r) => r,
        None => return Ok(false),
    };
    if row.external_session_id.is_none() {
        tracing::info!(
            session = %session_id,
            "maybe_auto_resume: no external_session_id; cannot auto-resume"
        );
        return Ok(false);
    }
    match cold_resume_session(handle, session_id).await {
        Ok(_) => {
            tracing::info!(
                session = %session_id,
                "maybe_auto_resume: auto-resumed crashed session"
            );
            Ok(true)
        }
        Err(e) => {
            tracing::warn!(
                session = %session_id,
                error = %e,
                "maybe_auto_resume: cold_resume_session failed"
            );
            Ok(false)
        }
    }
}

/// Read `workspaces.settings_json.auto_resume_agents` for the workspace
/// that owns `session_id`. Returns `false` on any error or missing
/// key — the cold path falls back to "leave crashed" on doubt.
async fn read_auto_resume_for_session(
    handle: &AgentSupervisorHandle,
    session_id: &SessionId,
) -> bool {
    let persistence = handle.persistence();
    let pool = persistence.readers();
    let row = match sqlx::query_as::<_, (String,)>(
        "SELECT ws.settings_json
         FROM sessions s
         JOIN workareas wa  ON wa.id = s.workarea_id
         JOIN workspaces ws ON ws.id = wa.workspace_id
         WHERE s.id = ?",
    )
    .bind(&session_id.0)
    .fetch_optional(pool)
    .await
    {
        Ok(Some((j,))) => j,
        _ => return false,
    };
    serde_json::from_str::<serde_json::Value>(&row)
        .ok()
        .and_then(|v| {
            v.as_object()
                .and_then(|m| m.get("auto_resume_agents"))
                .and_then(|x| x.as_bool())
        })
        .unwrap_or(false)
}
