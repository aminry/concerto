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
use concerto_persist::RepositoryId;
use concerto_persist::{
    Persistence, SessionId as PersistSessionId, WorkareaId as PersistWorkareaId,
};
use concerto_proto::v1::sessions_server::Sessions as SessionsService;
use concerto_proto::v1::{
    ApprovalDecision, CreateSessionRequest, ListMcpResponse, ListSessionsRequest,
    ListSessionsResponse, McpScopeRequest, McpServer as ProtoMcpServer, PermissionMode,
    ResizeSessionRequest, ResolveApprovalRequest, RevertRequest, SendMessageRequest,
    Session as ProtoSession, SessionId as ProtoSessionId, StopSessionRequest,
    UpdateSessionPermissionModeRequest, UpsertProjectMcpRequest,
};
use std::sync::Arc;
use tonic::{Request, Response, Status};

use crate::agent_supervisor::mcp::{self, McpScope, McpScopeFilter, McpServer};
use crate::agent_supervisor::{AgentKind, AgentSupervisorHandle, StartSessionRequest};
use crate::error_map::error_to_status;
use crate::security::Decision;
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
                resume_session_id: None,
                chat_id: None,
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

    #[tracing::instrument(skip_all, name = "Sessions::ResolveApproval")]
    async fn resolve_approval(
        &self,
        request: Request<ResolveApprovalRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.session_id.is_empty() {
            return Err(Status::invalid_argument("session_id is required"));
        }
        if req.approval_id.is_empty() {
            return Err(Status::invalid_argument("approval_id is required"));
        }
        let decision = approval_decision_from_i32(req.decision)?;
        let id = PersistSessionId(req.session_id);
        self.supervisor
            .resolve_approval(&id, &req.approval_id, decision, None)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }

    #[tracing::instrument(skip_all, name = "Sessions::RevertToCheckpoint")]
    async fn revert_to_checkpoint(
        &self,
        request: Request<RevertRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.checkpoint_id.is_empty() {
            return Err(Status::invalid_argument("checkpoint_id is required"));
        }
        if req.session_id.is_empty() {
            return Err(Status::invalid_argument("session_id is required"));
        }
        let id = PersistSessionId(req.session_id);
        self.supervisor
            .revert_to_checkpoint(&req.checkpoint_id, &id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
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

    #[tracing::instrument(skip_all, name = "Sessions::DeleteSession")]
    async fn delete_session(
        &self,
        request: Request<ProtoSessionId>,
    ) -> Result<Response<()>, Status> {
        let id = request.into_inner().value;
        if id.is_empty() {
            return Err(Status::invalid_argument(
                "session_id (value) must not be empty",
            ));
        }
        self.supervisor
            .delete_session(&PersistSessionId(id), None)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }

    #[tracing::instrument(skip_all, name = "Sessions::ResizeSession")]
    async fn resize_session(
        &self,
        request: Request<ResizeSessionRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.session_id.is_empty() {
            return Err(Status::invalid_argument("session_id is required"));
        }
        // Clamp to u16 + sane minimums; a 0-sized PTY makes TUIs misbehave.
        let rows = req.rows.clamp(1, u16::MAX as u32) as u16;
        let cols = req.cols.clamp(1, u16::MAX as u32) as u16;
        let id = PersistSessionId(req.session_id);
        self.supervisor
            .resize_session(&id, rows, cols)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }

    #[tracing::instrument(skip_all, name = "Sessions::ListMcpServers")]
    async fn list_mcp_servers(
        &self,
        request: Request<McpScopeRequest>,
    ) -> Result<Response<ListMcpResponse>, Status> {
        let req = request.into_inner();
        let filter = parse_mcp_filter(req.scope.as_deref(), req.repository_id.as_deref())?;
        // Production callers read the developer's real home directory;
        // the test harness uses `mcp::list_mcp_servers` directly with an
        // explicit override. See `crates/core/tests/mcp_listing.rs`.
        let servers = mcp::list_mcp_servers(&self.persistence, filter, None)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(ListMcpResponse {
            servers: servers.into_iter().map(mcp_server_to_proto).collect(),
        }))
    }

    #[tracing::instrument(skip_all, name = "Sessions::UpsertProjectMcp")]
    async fn upsert_project_mcp(
        &self,
        _request: Request<UpsertProjectMcpRequest>,
    ) -> Result<Response<()>, Status> {
        // V0.1 is read-only — writing `.mcp.json` is V1.0. The RPC is
        // declared so the wire surface is locked; the handler responds
        // with `UNIMPLEMENTED` to give clients a clean failure path
        // until the writer lands.
        Err(Status::unimplemented(
            "mcp.upsert: writing project-level .mcp.json is V1.0",
        ))
    }

    #[tracing::instrument(skip_all, name = "Sessions::ColdResumeSession")]
    async fn cold_resume_session(
        &self,
        request: Request<ProtoSessionId>,
    ) -> Result<Response<ProtoSession>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("session id is required"));
        }
        let id = PersistSessionId(req.value);
        crate::agent_supervisor::cold_resume::cold_resume_session(&self.supervisor, &id)
            .await
            .map_err(error_to_status)?;
        let row = concerto_persist::sessions::get(self.persistence.readers(), &id)
            .await
            .map_err(error_to_status)?
            .ok_or_else(|| Status::not_found(format!("session {id} not found")))?;
        Ok(Response::new(session_to_proto(row)))
    }
}

/// Translate the wire `McpScopeRequest` into the typed filter. Rules:
///
/// - Both fields absent → `All`.
/// - `scope = "personal" | "plugin" | "enterprise"` → matching variant.
/// - `scope = "project"` → `repository_id` MUST be present;
///   `INVALID_ARGUMENT` otherwise.
/// - `repository_id` present without `scope` → treat as `Project(id)`
///   (the field is only ever meaningful in project context).
/// - Any other `scope` string → `INVALID_ARGUMENT`.
#[allow(clippy::result_large_err)]
fn parse_mcp_filter(scope: Option<&str>, repo_id: Option<&str>) -> Result<McpScopeFilter, Status> {
    match (scope, repo_id) {
        (None, None) => Ok(McpScopeFilter::All),
        (None, Some(id)) => Ok(McpScopeFilter::Project(RepositoryId(id.to_string()))),
        (Some("personal"), _) => Ok(McpScopeFilter::Personal),
        (Some("plugin"), _) => Ok(McpScopeFilter::Plugin),
        (Some("enterprise"), _) => Ok(McpScopeFilter::Enterprise),
        (Some("project"), Some(id)) if !id.is_empty() => {
            Ok(McpScopeFilter::Project(RepositoryId(id.to_string())))
        }
        (Some("project"), _) => Err(Status::invalid_argument(
            "mcp.scope: scope=\"project\" requires a non-empty repository_id",
        )),
        (Some("all"), _) => Ok(McpScopeFilter::All),
        (Some(other), _) => Err(Status::invalid_argument(format!(
            "mcp.scope: unknown scope {other:?}; expected one of personal|project|plugin|enterprise"
        ))),
    }
}

fn mcp_server_to_proto(s: McpServer) -> ProtoMcpServer {
    let scope = match &s.scope {
        McpScope::Personal => "personal",
        McpScope::Project(_) => "project",
        McpScope::Plugin => "plugin",
        McpScope::Enterprise => "enterprise",
    };
    ProtoMcpServer {
        name: s.name,
        scope: scope.to_string(),
        command: s.command,
        args: s.args,
        env: s.env.into_iter().collect(),
        source_path: s.source_path.to_string_lossy().into_owned(),
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
        // Task 402: a wire `agent_kind="maestro"` round-trips to the new kind
        // (the boot-time lifecycle spawn is 414's wiring; this keeps the parser
        // honest + round-trippable with `as_db_kind`).
        "maestro" => Ok(AgentKind::Maestro),
        "codex" | "gemini" => Err(Status::invalid_argument(format!(
            "agent.unsupported: agent_kind {s:?} is not implemented in V0.1"
        ))),
        other => Err(Status::invalid_argument(format!(
            "agent.unsupported: agent_kind {other:?} must be one of echo|claude|maestro"
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

/// Map the wire `ApprovalDecision` enum onto the in-process
/// [`Decision`] enum. `UNSPECIFIED` is rejected; users cannot send
/// `AutoDeny` (auto verdicts are server-side).
#[allow(clippy::result_large_err)]
fn approval_decision_from_i32(v: i32) -> Result<Decision, Status> {
    let ad = ApprovalDecision::try_from(v)
        .map_err(|_| Status::invalid_argument(format!("decision {v} is not a known enum value")))?;
    match ad {
        ApprovalDecision::Unspecified => Err(Status::invalid_argument(
            "decision must be one of APPROVE|APPROVE_ONCE|DENY",
        )),
        ApprovalDecision::Approve => Ok(Decision::AutoApprove),
        ApprovalDecision::ApproveOnce => Ok(Decision::AutoApproveOnce),
        ApprovalDecision::Deny => Ok(Decision::AutoDeny),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Task 402: a wire `agent_kind="maestro"` parses to `AgentKind::Maestro`
    /// (round-trips with `as_db_kind`/`from_db_kind`); `echo`/`claude` are
    /// unchanged; `codex`/`gemini` + garbage still reject.
    #[test]
    fn parse_agent_kind_accepts_maestro() {
        assert_eq!(parse_agent_kind("maestro").unwrap(), AgentKind::Maestro);
        assert_eq!(parse_agent_kind("echo").unwrap(), AgentKind::Echo);
        assert_eq!(parse_agent_kind("claude").unwrap(), AgentKind::Claude);
        assert!(parse_agent_kind("codex").is_err());
        assert!(parse_agent_kind("gemini").is_err());
        assert!(parse_agent_kind("bogus").is_err());
    }
}
