//! gRPC `Skills` service handler (Task 39).
//!
//! Thin wrapper over [`crate::skills::SkillsRegistryHandle`]: translate
//! `concerto.v1.Skills` requests into handle calls and translate the
//! result back into proto messages.

use async_trait::async_trait;
use concerto_persist::{SkillFilter, SkillId, SkillRow, SkillScope, WorkspaceId};
use concerto_proto::v1::skills_server::Skills as SkillsService;
use concerto_proto::v1::{
    ListSkillsRequest, ListSkillsResponse, RefreshMarketplacesRequest, RefreshMarketplacesResponse,
    Skill as ProtoSkill, ToggleSkillRequest,
};
use tonic::{Request, Response, Status};

use crate::error_map::error_to_status;
use crate::skills::SkillsRegistryHandle;

#[derive(Clone)]
pub struct SkillsHandler {
    registry: SkillsRegistryHandle,
}

impl SkillsHandler {
    pub fn new(registry: SkillsRegistryHandle) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl SkillsService for SkillsHandler {
    #[tracing::instrument(skip_all, name = "Skills::ListSkills")]
    async fn list_skills(
        &self,
        request: Request<ListSkillsRequest>,
    ) -> Result<Response<ListSkillsResponse>, Status> {
        let req = request.into_inner();
        let scope = match req.scope.as_deref() {
            None | Some("") => None,
            Some(s) => Some(SkillScope::from_sql_str(s).ok_or_else(|| {
                Status::invalid_argument(format!(
                    "unknown scope {s:?} — expected personal|workspace|plugin|enterprise"
                ))
            })?),
        };
        let workspace_id = match req.workspace_id.as_deref() {
            None | Some("") => None,
            Some(w) => Some(WorkspaceId(w.to_string())),
        };
        let enabled_only = req.enabled_only.unwrap_or(false);
        let rows = self
            .registry
            .list(SkillFilter {
                scope,
                workspace_id,
                enabled_only,
            })
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(ListSkillsResponse {
            skills: rows.into_iter().map(skill_to_proto).collect(),
        }))
    }

    #[tracing::instrument(skip_all, name = "Skills::ToggleSkill")]
    async fn toggle_skill(
        &self,
        request: Request<ToggleSkillRequest>,
    ) -> Result<Response<ProtoSkill>, Status> {
        let req = request.into_inner();
        if req.skill_id.is_empty() {
            return Err(Status::invalid_argument("skill_id is required"));
        }
        let updated = self
            .registry
            .toggle(&SkillId(req.skill_id), req.enable)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(skill_to_proto(updated)))
    }

    #[tracing::instrument(skip_all, name = "Skills::RefreshMarketplaces")]
    async fn refresh_marketplaces(
        &self,
        request: Request<RefreshMarketplacesRequest>,
    ) -> Result<Response<RefreshMarketplacesResponse>, Status> {
        let req = request.into_inner();
        let workspace_filter = match req.workspace_id.as_deref() {
            None | Some("") => None,
            Some(w) => Some(WorkspaceId(w.to_string())),
        };
        let report = self
            .registry
            .refresh(workspace_filter.as_ref())
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(RefreshMarketplacesResponse {
            discovered_count: report.discovered_count as i64,
            errors: report.errors,
        }))
    }
}

fn skill_to_proto(row: SkillRow) -> ProtoSkill {
    let tools: Vec<String> = serde_json::from_str(&row.tools_json).unwrap_or_default();
    ProtoSkill {
        id: row.id.0,
        scope: row.scope.as_sql_str().to_string(),
        workspace_id: row.workspace_id.map(|w| w.0).unwrap_or_default(),
        name: row.name,
        slash_command: row.slash_command.unwrap_or_default(),
        description: row.description.unwrap_or_default(),
        tools,
        source_path: row.source_path,
        enabled: row.enabled,
    }
}
