//! gRPC `Sessions` service handler (Task 23).
//!
//! Translates `concerto.v1.Sessions` requests into calls against
//! [`crate::agent_supervisor::AgentSupervisorHandle`]. V0.1 surface:
//!
//! - `CreateSession` — validates the `agent_kind` string (`echo|claude`
//!   in V0.1; `codex|gemini` error `INVALID_ARGUMENT` until the Phase 3
//!   parser packs land); delegates to the Agent Supervisor; reads the
//!   freshly-inserted row back from persistence and returns it.
//! - `GetSession` — pure read against `sessions` via persistence.
//! - `ListSessions` — pure read against `sessions` scoped by workarea.
//! - `SendMessage` — forwards `payload` bytes through
//!   [`AgentSupervisorHandle::send_input`].
//! - `StopSession` — delegates to
//!   [`AgentSupervisorHandle::stop_session`]; `reason` is logged.
//!
//! Mapping back to proto:
//!
//! - `Session.status` is the raw lowercase string from the DB
//!   (`starting|running|awaiting|finished|crashed`).
//! - `Session.agent_kind` is the DB-stored string (Task 22 maps the
//!   in-process `AgentKind::Echo` to `"claude"` in the DB, so the
//!   proto always sees one of the CHECK-set values).
//! - `Session.permission_mode` is the proto enum derived from the
//!   lowercase DB string.

use async_trait::async_trait;
use concerto_persist::{
    Persistence, SessionId as PersistSessionId, WorkareaId as PersistWorkareaId,
};
use concerto_proto::v1::sessions_server::Sessions as SessionsService;
use concerto_proto::v1::{
    CreateSessionRequest, ListSessionsRequest, ListSessionsResponse, PermissionMode,
    SendMessageRequest, Session as ProtoSession, SessionId as ProtoSessionId, StopSessionRequest,
    UpdateSessionPermissionModeRequest,
};
use std::sync::Arc;
use tonic::{Request, Response, Status};

use crate::agent_supervisor::{AgentKind, AgentSupervisorHandle, StartSessionRequest};
use crate::error_map::error_to_status;
use crate::workspace_manager::WorkareaManager;

/// Implements the generated `Sessions` service trait.
#[derive(Clone)]
pub struct SessionsHandler {
    supervisor: AgentSupervisorHandle,
    persistence: Arc<Persistence>,
    /// Used to look up the workarea's worktree root so the agent host
    /// gets a real cwd. The Agent Supervisor's `start_session` itself
    /// only validates that the workarea exists; the handler is
    /// responsible for resolving the cwd because V0.1's `cwd` is the
    /// worktree root.
    workareas: WorkareaManager,
}

impl SessionsHandler {
    pub fn new(
        supervisor: AgentSupervisorHandle,
        persistence: Arc<Persistence>,
        workareas: WorkareaManager,
    ) -> Self {
        Self {
            supervisor,
            persistence,
            workareas,
        }
    }
}

#[async_trait]
impl SessionsService for SessionsHandler {
    #[tracing::instrument(skip_all, name = "Sessions::CreateSession")]
    async fn create_session(
        &self,
        request: Request<CreateSessionRequest>,
    ) -> Result<Response<ProtoSession>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        let kind = parse_agent_kind(&req.agent_kind)?;
        let permission_mode = req
            .permission_mode
            .map(permission_mode_from_i32)
            .transpose()?;

        // Resolve the workarea's worktree root for the agent's cwd.
        let wa_id = PersistWorkareaId(req.workarea_id.clone());
        let workarea = self
            .workareas
            .get(&wa_id)
            .await
            .map_err(error_to_status)?
            .ok_or_else(|| Status::not_found(format!("workarea {} not found", req.workarea_id)))?;
        let cwd = std::path::PathBuf::from(&workarea.worktree_root);

        let session_id = self
            .supervisor
            .start_session(StartSessionRequest {
                workarea_id: wa_id,
                agent_kind: kind,
                echo_text: None,
                cwd,
                permission_mode,
            })
            .await
            .map_err(error_to_status)?;

        // Read the just-inserted row back so the wire shape exactly
        // mirrors persistence.
        let row = concerto_persist::sessions::get(self.persistence.readers(), &session_id)
            .await
            .map_err(error_to_status)?
            .ok_or_else(|| Status::internal("session row missing after create"))?;
        Ok(Response::new(session_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Sessions::GetSession")]
    async fn get_session(
        &self,
        request: Request<ProtoSessionId>,
    ) -> Result<Response<ProtoSession>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("session id is required"));
        }
        let id = PersistSessionId(req.value);
        let row = concerto_persist::sessions::get(self.persistence.readers(), &id)
            .await
            .map_err(error_to_status)?
            .ok_or_else(|| Status::not_found(format!("session {id} not found")))?;
        Ok(Response::new(session_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Sessions::ListSessions")]
    async fn list_sessions(
        &self,
        request: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        let wa_id = PersistWorkareaId(req.workarea_id);
        let rows = concerto_persist::sessions::list_by_workarea(self.persistence.readers(), &wa_id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(ListSessionsResponse {
            sessions: rows.into_iter().map(session_to_proto).collect(),
        }))
    }

    #[tracing::instrument(skip_all, name = "Sessions::SendMessage")]
    async fn send_message(
        &self,
        request: Request<SendMessageRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.session_id.is_empty() {
            return Err(Status::invalid_argument("session_id is required"));
        }
        let id = PersistSessionId(req.session_id);
        self.supervisor
            .send_input(&id, req.payload)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }

    #[tracing::instrument(skip_all, name = "Sessions::UpdateSessionPermissionMode")]
    async fn update_session_permission_mode(
        &self,
        request: Request<UpdateSessionPermissionModeRequest>,
    ) -> Result<Response<ProtoSession>, Status> {
        let req = request.into_inner();
        if req.session_id.is_empty() {
            return Err(Status::invalid_argument("session_id is required"));
        }
        let mode = permission_mode_from_i32(req.permission_mode)?;
        let id = PersistSessionId(req.session_id);
        self.supervisor
            .update_session_permission_mode(&id, &mode, &req.acknowledgement)
            .await
            .map_err(error_to_status)?;
        // Reload the row so the wire shape mirrors persistence.
        let row = concerto_persist::sessions::get(self.persistence.readers(), &id)
            .await
            .map_err(error_to_status)?
            .ok_or_else(|| Status::not_found(format!("session {id} not found")))?;
        Ok(Response::new(session_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Sessions::StopSession")]
    async fn stop_session(
        &self,
        request: Request<StopSessionRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.session_id.is_empty() {
            return Err(Status::invalid_argument("session_id is required"));
        }
        let id = PersistSessionId(req.session_id);
        let reason = if req.reason.is_empty() {
            None
        } else {
            Some(req.reason)
        };
        self.supervisor
            .stop_session(&id, reason)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }
}

/// Parse the wire `agent_kind` string into the in-process enum. V0.1
/// accepts `echo` and `claude`; `codex` / `gemini` are reserved but
/// return `INVALID_ARGUMENT` until Phase 3 parser packs ship.
#[allow(clippy::result_large_err)]
fn parse_agent_kind(s: &str) -> Result<AgentKind, Status> {
    match s {
        "echo" => Ok(AgentKind::Echo),
        "claude" => Ok(AgentKind::Claude),
        "codex" | "gemini" => Err(Status::invalid_argument(format!(
            "agent.unsupported: agent_kind {s:?} is not implemented in V0.1"
        ))),
        other => Err(Status::invalid_argument(format!(
            "agent.unsupported: agent_kind {other:?} must be one of echo|claude"
        ))),
    }
}

fn session_to_proto(row: concerto_persist::Session) -> ProtoSession {
    ProtoSession {
        id: row.id.to_string(),
        workarea_id: row.workarea_id.to_string(),
        chat_id: row.chat_id,
        agent_kind: row.agent_kind,
        agent_version: row.agent_version,
        model: row.model,
        status: row.status,
        permission_mode: permission_mode_to_i32(&row.permission_mode),
        started_at: Some(epoch_ms_to_ts(row.started_at)),
        ended_at: row.ended_at.map(epoch_ms_to_ts),
    }
}

fn epoch_ms_to_ts(ms: i64) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: ms.div_euclid(1000),
        nanos: (ms.rem_euclid(1000) * 1_000_000) as i32,
    }
}

#[allow(clippy::result_large_err)]
fn permission_mode_from_i32(v: i32) -> Result<String, Status> {
    let pm = PermissionMode::try_from(v).map_err(|_| {
        Status::invalid_argument(format!("permission_mode {v} is not a known enum value"))
    })?;
    match pm {
        PermissionMode::Unspecified => Err(Status::invalid_argument(
            "permission_mode must be one of STRICT|NORMAL|AUTO|YOLO",
        )),
        PermissionMode::Strict => Ok("strict".to_string()),
        PermissionMode::Normal => Ok("normal".to_string()),
        PermissionMode::Auto => Ok("auto".to_string()),
        PermissionMode::Yolo => Ok("yolo".to_string()),
    }
}

fn permission_mode_to_i32(s: &str) -> i32 {
    match s {
        "strict" => PermissionMode::Strict as i32,
        "normal" => PermissionMode::Normal as i32,
        "auto" => PermissionMode::Auto as i32,
        "yolo" => PermissionMode::Yolo as i32,
        _ => PermissionMode::Unspecified as i32,
    }
}
