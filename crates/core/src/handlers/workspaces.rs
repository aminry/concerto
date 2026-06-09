//! gRPC `Workspaces` service handler (Task 19).
//!
//! Translates `concerto.v1.Workspaces` requests into calls against
//! [`crate::workspace_manager::WorkspaceManager`]. V0.1 surface:
//!
//! - `CreateWorkspace` — accepts 1..N repos (Task 306) + slug
//!   derivation; rejects an empty set (`workspace.no_repos`), a repeated
//!   id (`workspace.duplicate_repo`), and foreign/unknown repos, all as
//!   `INVALID_ARGUMENT` / `NOT_FOUND`. The V0.1 single-repo rejection is
//!   retired.
//! - `GetWorkspace` — returns the persisted row.
//! - `ListWorkspaces` — scoped by project.
//! - `ArchiveWorkspace` — idempotent UPDATE.

use async_trait::async_trait;
use concerto_persist::WorkspaceId as PersistWorkspaceId;
use concerto_proto::v1::workspaces_server::Workspaces as WorkspacesService;
use concerto_proto::v1::{
    CreateWorkspaceRequest, ListWorkspaceReposResponse, ListWorkspacesRequest,
    ListWorkspacesResponse, PermissionMode, UpdateWorkspaceRequest, UpdateWorkspaceSettingsRequest,
    Workspace as ProtoWorkspace, WorkspaceId as ProtoWorkspaceId, WorkspaceRepoEntry,
};
use tonic::{Request, Response, Status};

use crate::error_map::error_to_status;
use crate::workspace_manager::{WorkspaceManager, WorkspaceRepoSpec};

/// Implements the generated `Workspaces` service trait.
#[derive(Clone)]
pub struct WorkspacesHandler {
    workspace_manager: WorkspaceManager,
}

impl WorkspacesHandler {
    pub fn new(workspace_manager: WorkspaceManager) -> Self {
        Self { workspace_manager }
    }
}

#[async_trait]
impl WorkspacesService for WorkspacesHandler {
    #[tracing::instrument(skip_all, name = "Workspaces::CreateWorkspace")]
    async fn create_workspace(
        &self,
        request: Request<CreateWorkspaceRequest>,
    ) -> Result<Response<ProtoWorkspace>, Status> {
        let req = request.into_inner();
        let permission_mode = req
            .permission_mode
            .map(permission_mode_from_i32)
            .transpose()?;
        let repos: Vec<WorkspaceRepoSpec> = req
            .repos
            .into_iter()
            .map(|r| WorkspaceRepoSpec {
                repository_id: concerto_persist::RepositoryId(r.repository_id),
                sparse_cones: r.sparse_cones,
            })
            .collect();
        let row = self
            .workspace_manager
            .create_workspace(
                &req.name,
                &repos,
                permission_mode,
                req.description,
                req.icon,
            )
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(workspace_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Workspaces::GetWorkspace")]
    async fn get_workspace(
        &self,
        request: Request<ProtoWorkspaceId>,
    ) -> Result<Response<ProtoWorkspace>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("workspace id is required"));
        }
        let id = PersistWorkspaceId(req.value);
        match self
            .workspace_manager
            .get(&id)
            .await
            .map_err(error_to_status)?
        {
            Some(ws) => Ok(Response::new(workspace_to_proto(ws))),
            None => Err(Status::not_found(format!("workspace {id} not found"))),
        }
    }

    #[tracing::instrument(skip_all, name = "Workspaces::ListWorkspaces")]
    async fn list_workspaces(
        &self,
        request: Request<ListWorkspacesRequest>,
    ) -> Result<Response<ListWorkspacesResponse>, Status> {
        let req = request.into_inner();
        let mut rows = self
            .workspace_manager
            .list_all()
            .await
            .map_err(error_to_status)?;
        // `include_archived = false` (default) hides archived workspaces.
        if !req.include_archived {
            rows.retain(|w| w.archived_at.is_none());
        }
        Ok(Response::new(ListWorkspacesResponse {
            workspaces: rows.into_iter().map(workspace_to_proto).collect(),
        }))
    }

    #[tracing::instrument(skip_all, name = "Workspaces::ArchiveWorkspace")]
    async fn archive_workspace(
        &self,
        request: Request<ProtoWorkspaceId>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("workspace id is required"));
        }
        let id = PersistWorkspaceId(req.value);
        self.workspace_manager
            .archive(&id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }

    #[tracing::instrument(skip_all, name = "Workspaces::RestoreWorkspace")]
    async fn restore_workspace(
        &self,
        request: Request<ProtoWorkspaceId>,
    ) -> Result<Response<ProtoWorkspace>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("workspace id is required"));
        }
        let id = PersistWorkspaceId(req.value);
        let row = self
            .workspace_manager
            .restore_workspace(&id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(workspace_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Workspaces::UpdateWorkspaceSettings")]
    async fn update_workspace_settings(
        &self,
        request: Request<UpdateWorkspaceSettingsRequest>,
    ) -> Result<Response<ProtoWorkspace>, Status> {
        let req = request.into_inner();
        if req.workspace_id.is_empty() {
            return Err(Status::invalid_argument("workspace_id is required"));
        }
        let settings = req.settings.ok_or_else(|| {
            Status::invalid_argument("settings is required (use {} to send a no-op)")
        })?;
        // V0.1: a `Some(UNSPECIFIED)` permission_mode is rejected; the
        // caller signals "no change" by omitting the field entirely.
        let permission_mode_patch: Option<Option<String>> = match settings.permission_mode {
            Some(v) => Some(Some(permission_mode_from_i32(v)?)),
            None => None,
        };
        let id = PersistWorkspaceId(req.workspace_id);
        let row = self
            .workspace_manager
            .update_workspace_settings(&id, permission_mode_patch)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(workspace_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Workspaces::UpdateWorkspace")]
    async fn update_workspace(
        &self,
        request: Request<UpdateWorkspaceRequest>,
    ) -> Result<Response<ProtoWorkspace>, Status> {
        let req = request.into_inner();
        if req.workspace_id.is_empty() {
            return Err(Status::invalid_argument("workspace_id is required"));
        }
        // `optional string` → Option<String>. icon/description map to the
        // actor's nested Option (present = set/clear, absent = no change).
        let name = req.name;
        let icon = req.icon.map(Some);
        let description = req.description.map(Some);
        let repos: Vec<WorkspaceRepoSpec> = req
            .repos
            .into_iter()
            .map(|r| WorkspaceRepoSpec {
                repository_id: concerto_persist::RepositoryId(r.repository_id),
                sparse_cones: r.sparse_cones,
            })
            .collect();
        let id = PersistWorkspaceId(req.workspace_id);
        let row = self
            .workspace_manager
            .update_workspace(&id, name, icon, description, &repos)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(workspace_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Workspaces::ListWorkspaceRepos")]
    async fn list_workspace_repos(
        &self,
        request: Request<ProtoWorkspaceId>,
    ) -> Result<Response<ListWorkspaceReposResponse>, Status> {
        let req = request.into_inner();
        if req.value.is_empty() {
            return Err(Status::invalid_argument("workspace id is required"));
        }
        let id = PersistWorkspaceId(req.value);
        let repos = self
            .workspace_manager
            .list_workspace_repos(&id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(ListWorkspaceReposResponse {
            repos: repos
                .into_iter()
                .map(|r| WorkspaceRepoEntry {
                    repository_id: r.repository_id.0,
                    sparse_cones: r.sparse_cones,
                })
                .collect(),
        }))
    }
}

/// Convert a persisted `Workspace` into the wire shape.
fn workspace_to_proto(row: concerto_persist::Workspace) -> ProtoWorkspace {
    ProtoWorkspace {
        id: row.id.to_string(),
        name: row.name,
        slug: row.slug,
        icon: row.icon,
        description: row.description,
        permission_mode: row.permission_mode.as_deref().map(permission_mode_to_i32),
        created_at: Some(epoch_ms_to_ts(row.created_at)),
        archived_at: row.archived_at.map(epoch_ms_to_ts),
    }
}

fn epoch_ms_to_ts(ms: i64) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: ms.div_euclid(1000),
        nanos: (ms.rem_euclid(1000) * 1_000_000) as i32,
    }
}

/// Convert the wire `PermissionMode` enum (carried as `i32`) into the
/// lowercase SQL string the persistence layer expects.
///
/// `tonic::Status` is ~176 bytes so the `Result<String, Status>`
/// trips `clippy::result-large-err`; the function is only called once
/// per RPC at the request-deserialization step, so the cost is amortised
/// against the gRPC round-trip and not worth boxing.
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
