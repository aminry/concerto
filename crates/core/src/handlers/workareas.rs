//! gRPC `Workareas` service handler (Task 20).
//!
//! Translates `concerto.v1.Workareas` requests into calls against
//! [`crate::workspace_manager::WorkareaManager`]. V0.1 surface:
//!
//! - `CreateWorkarea` — allocates composer name, sets up worktree +
//!   `.context/`, persists rows, transitions `created → active`.
//! - `GetWorkarea` — returns the persisted row.
//! - `ListWorkareas` — scoped by workspace; `include_archived` knob.
//! - `ArchiveWorkarea` — idempotent UPDATE (status → archived, sets
//!   `archived_at`).

use async_trait::async_trait;
use concerto_persist::{
    RepositoryId as PersistRepositoryId, WorkareaId as PersistWorkareaId,
    WorkspaceId as PersistWorkspaceId,
};
use concerto_proto::v1::workareas_server::Workareas as WorkareasService;
use concerto_proto::v1::{
    ArchiveWorkareaRequest, CreateWorkareaRequest, DiffHunk as ProtoDiffHunk,
    DiffKind as ProtoDiffKind, DiffPayload as ProtoDiffPayload, FileDiff as ProtoFileDiff,
    GetDiffRequest, GetWorkareaPrSetResponse, ListWorkareasRequest, ListWorkareasResponse,
    PermissionMode, PullRequest as ProtoPullRequest, SetWorkareaBypassDestructiveGuardRequest,
    UpdateWorkareaPermissionModeRequest, Workarea as ProtoWorkarea, WorkareaId as ProtoWorkareaId,
};
use tonic::{Request, Response, Status};

use crate::error_map::error_to_status;
use crate::workspace_manager::{ArchiveOpts, WorkareaManager};

/// Implements the generated `Workareas` service trait.
#[derive(Clone)]
pub struct WorkareasHandler {
    workarea_manager: WorkareaManager,
}

impl WorkareasHandler {
    pub fn new(workarea_manager: WorkareaManager) -> Self {
        Self { workarea_manager }
    }
}

#[async_trait]
impl WorkareasService for WorkareasHandler {
    #[tracing::instrument(skip_all, name = "Workareas::CreateWorkarea")]
    async fn create_workarea(
        &self,
        request: Request<CreateWorkareaRequest>,
    ) -> Result<Response<ProtoWorkarea>, Status> {
        let req = request.into_inner();
        let permission_mode = req
            .permission_mode
            .map(permission_mode_from_i32)
            .transpose()?;
        let row = self
            .workarea_manager
            .create_workarea(&req.workspace_id, permission_mode)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(workarea_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Workareas::GetWorkarea")]
    async fn get_workarea(
        &self,
        request: Request<ProtoWorkareaId>,
    ) -> Result<Response<ProtoWorkarea>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("workarea id is required"));
        }
        let id = PersistWorkareaId(req.value);
        match self
            .workarea_manager
            .get(&id)
            .await
            .map_err(error_to_status)?
        {
            Some(wa) => Ok(Response::new(workarea_to_proto(wa))),
            None => Err(Status::not_found(format!("workarea {id} not found"))),
        }
    }

    #[tracing::instrument(skip_all, name = "Workareas::ListWorkareas")]
    async fn list_workareas(
        &self,
        request: Request<ListWorkareasRequest>,
    ) -> Result<Response<ListWorkareasResponse>, Status> {
        let req = request.into_inner();
        if req.workspace_id.is_empty() {
            return Err(Status::invalid_argument("workspace_id is required"));
        }
        let ws_id = PersistWorkspaceId(req.workspace_id);
        let rows = self
            .workarea_manager
            .list_by_workspace(&ws_id, req.include_archived)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(ListWorkareasResponse {
            workareas: rows.into_iter().map(workarea_to_proto).collect(),
        }))
    }

    #[tracing::instrument(skip_all, name = "Workareas::GetWorkareaRepoDiff")]
    async fn get_workarea_repo_diff(
        &self,
        request: Request<GetDiffRequest>,
    ) -> Result<Response<ProtoDiffPayload>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        if req.repository_id.is_empty() {
            return Err(Status::invalid_argument("repository_id is required"));
        }
        let wa_id = PersistWorkareaId(req.workarea_id);
        let repo_id = PersistRepositoryId(req.repository_id);
        let payload = self
            .workarea_manager
            .get_repo_diff(&wa_id, &repo_id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(diff_payload_to_proto(payload)))
    }

    #[tracing::instrument(skip_all, name = "Workareas::ArchiveWorkarea")]
    async fn archive_workarea(
        &self,
        request: Request<ProtoWorkareaId>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("workarea id is required"));
        }
        let id = PersistWorkareaId(req.value);
        self.workarea_manager
            .archive(&id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }

    #[tracing::instrument(skip_all, name = "Workareas::ArchiveWorkareaWithOpts")]
    async fn archive_workarea_with_opts(
        &self,
        request: Request<ArchiveWorkareaRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        let id = PersistWorkareaId(req.workarea_id);
        let opts = ArchiveOpts {
            remove_worktree: req.remove_worktree,
        };
        self.workarea_manager
            .archive_workarea(&id, opts)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }

    #[tracing::instrument(skip_all, name = "Workareas::RestoreWorkarea")]
    async fn restore_workarea(
        &self,
        request: Request<ProtoWorkareaId>,
    ) -> Result<Response<ProtoWorkarea>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("workarea id is required"));
        }
        let id = PersistWorkareaId(req.value);
        let row = self
            .workarea_manager
            .restore_workarea(&id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(workarea_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Workareas::UpdateWorkareaPermissionMode")]
    async fn update_workarea_permission_mode(
        &self,
        request: Request<UpdateWorkareaPermissionModeRequest>,
    ) -> Result<Response<ProtoWorkarea>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        // V0.1: `PERMISSION_MODE_UNSPECIFIED` clears the override
        // (inherit-from-workspace) — same convention as the workarea
        // settings_json field shape. The proto wire is a non-optional
        // enum so we map UNSPECIFIED → None.
        let mode: Option<String> = match PermissionMode::try_from(req.permission_mode) {
            Ok(PermissionMode::Unspecified) => None,
            Ok(PermissionMode::Strict) => Some("strict".to_string()),
            Ok(PermissionMode::Normal) => Some("normal".to_string()),
            Ok(PermissionMode::Auto) => Some("auto".to_string()),
            Ok(PermissionMode::Yolo) => Some("yolo".to_string()),
            Err(_) => {
                return Err(Status::invalid_argument(format!(
                    "permission_mode {} is not a known enum value",
                    req.permission_mode
                )));
            }
        };
        let id = PersistWorkareaId(req.workarea_id);
        let row = self
            .workarea_manager
            .update_workarea_permission_mode(&id, mode.as_deref(), &req.acknowledgement)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(workarea_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Workareas::GetWorkareaPrSet")]
    async fn get_workarea_pr_set(
        &self,
        request: Request<ProtoWorkareaId>,
    ) -> Result<Response<GetWorkareaPrSetResponse>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("workarea id is required"));
        }
        let id = PersistWorkareaId(req.value);
        let rows = self
            .workarea_manager
            .list_pr_set(&id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(GetWorkareaPrSetResponse {
            pull_requests: rows.into_iter().map(pull_request_to_proto).collect(),
        }))
    }

    #[tracing::instrument(skip_all, name = "Workareas::SetWorkareaBypassDestructiveGuard")]
    async fn set_workarea_bypass_destructive_guard(
        &self,
        request: Request<SetWorkareaBypassDestructiveGuardRequest>,
    ) -> Result<Response<ProtoWorkarea>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        let id = PersistWorkareaId(req.workarea_id);
        let row = self
            .workarea_manager
            .set_workarea_bypass_destructive_guard(&id, req.enable, &req.acknowledgement)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(workarea_to_proto(row)))
    }
}

fn workarea_to_proto(row: concerto_persist::Workarea) -> ProtoWorkarea {
    ProtoWorkarea {
        id: row.id.to_string(),
        workspace_id: row.workspace_id.to_string(),
        composer_name: row.composer_name,
        branch_name: row.branch_name,
        worktree_root: row.worktree_root,
        status: row.status,
        permission_mode: row.permission_mode.as_deref().map(permission_mode_to_i32),
        created_at: Some(epoch_ms_to_ts(row.created_at)),
        last_activity_at: row.last_activity_at.map(epoch_ms_to_ts),
        archived_at: row.archived_at.map(epoch_ms_to_ts),
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

/// Convert the Rust [`concerto_gix_wrap::DiffPayload`] into its proto
/// equivalent. The two types intentionally mirror each other one-to-one;
/// the conversion is a flat field-by-field copy plus enum mapping.
fn diff_payload_to_proto(p: concerto_gix_wrap::DiffPayload) -> ProtoDiffPayload {
    ProtoDiffPayload {
        files: p.files.into_iter().map(file_diff_to_proto).collect(),
    }
}

fn file_diff_to_proto(f: concerto_gix_wrap::FileDiff) -> ProtoFileDiff {
    let path = path_to_string(&f.path);
    let old_path = f.old_path.as_deref().map(path_to_string);
    ProtoFileDiff {
        path,
        kind: diff_kind_to_i32(&f.kind),
        old_path,
        hunks: f.hunks.into_iter().map(diff_hunk_to_proto).collect(),
    }
}

fn diff_hunk_to_proto(h: concerto_gix_wrap::DiffHunk) -> ProtoDiffHunk {
    ProtoDiffHunk {
        old_start: h.old_start,
        old_lines: h.old_lines,
        new_start: h.new_start,
        new_lines: h.new_lines,
        body: h.body,
    }
}

fn diff_kind_to_i32(k: &concerto_gix_wrap::DiffKind) -> i32 {
    match k {
        concerto_gix_wrap::DiffKind::Added => ProtoDiffKind::Added as i32,
        concerto_gix_wrap::DiffKind::Deleted => ProtoDiffKind::Deleted as i32,
        concerto_gix_wrap::DiffKind::Modified => ProtoDiffKind::Modified as i32,
        concerto_gix_wrap::DiffKind::Renamed => ProtoDiffKind::Renamed as i32,
    }
}

fn path_to_string(p: &std::path::Path) -> String {
    p.to_string_lossy().into_owned()
}

fn pull_request_to_proto(row: concerto_persist::PullRequest) -> ProtoPullRequest {
    ProtoPullRequest {
        id: row.id.to_string(),
        workarea_id: row.workarea_id.to_string(),
        repository_id: row.repository_id.to_string(),
        provider: row.provider,
        pr_number: row.pr_number,
        base_ref: row.base_ref,
        head_ref: row.head_ref,
        state: row.state,
        title: row.title,
        body: row.body,
        url: row.url,
        head_sha: row.head_sha,
        created_at: row.created_at,
        updated_at: row.updated_at,
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
