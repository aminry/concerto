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
use concerto_persist::{WorkareaId as PersistWorkareaId, WorkspaceId as PersistWorkspaceId};
use concerto_proto::v1::workareas_server::Workareas as WorkareasService;
use concerto_proto::v1::{
    CreateWorkareaRequest, ListWorkareasRequest, ListWorkareasResponse, PermissionMode,
    Workarea as ProtoWorkarea, WorkareaId as ProtoWorkareaId,
};
use tonic::{Request, Response, Status};

use crate::error_map::error_to_status;
use crate::workspace_manager::WorkareaManager;

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

fn permission_mode_to_i32(s: &str) -> i32 {
    match s {
        "strict" => PermissionMode::Strict as i32,
        "normal" => PermissionMode::Normal as i32,
        "auto" => PermissionMode::Auto as i32,
        "yolo" => PermissionMode::Yolo as i32,
        _ => PermissionMode::Unspecified as i32,
    }
}
