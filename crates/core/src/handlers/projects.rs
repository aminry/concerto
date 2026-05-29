//! gRPC `Projects` service handler.
//!
//! Surface: `ListProjects` (Task 24) and `CreateProject` (post-V0.1 — added
//! so the Desktop sidebar can offer a "+ Project" affordance instead of
//! requiring direct SQL seeding). Persistence is via
//! [`concerto_persist::projects`]; the server assigns the UUIDv7 id and the
//! `created_at` epoch-ms timestamp.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use concerto_error::Error;
use concerto_persist::{NewProject, Persistence, ProjectId};
use concerto_proto::v1::projects_server::Projects as ProjectsService;
use concerto_proto::v1::{
    CreateProjectRequest, ListProjectsRequest, ListProjectsResponse, Project as ProtoProject,
};
use tonic::{Request, Response, Status};

use crate::error_map::error_to_status;

/// Implements the generated `Projects` service trait.
#[derive(Clone)]
pub struct ProjectsHandler {
    persistence: Arc<Persistence>,
}

impl ProjectsHandler {
    pub fn new(persistence: Arc<Persistence>) -> Self {
        Self { persistence }
    }
}

#[async_trait]
impl ProjectsService for ProjectsHandler {
    #[tracing::instrument(skip_all, name = "Projects::ListProjects")]
    async fn list_projects(
        &self,
        _request: Request<ListProjectsRequest>,
    ) -> Result<Response<ListProjectsResponse>, Status> {
        let rows = concerto_persist::projects::list_all(self.persistence.readers())
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(ListProjectsResponse {
            projects: rows.into_iter().map(project_to_proto).collect(),
        }))
    }

    #[tracing::instrument(skip_all, name = "Projects::CreateProject")]
    async fn create_project(
        &self,
        request: Request<CreateProjectRequest>,
    ) -> Result<Response<ProtoProject>, Status> {
        let req = request.into_inner();
        let name = req.name.trim().to_string();
        if name.is_empty() {
            return Err(error_to_status(Error::Validation(
                "name is required".into(),
            )));
        }
        let icon = req.icon.and_then(|s| {
            let t = s.trim().to_string();
            (!t.is_empty()).then_some(t)
        });
        let id = ProjectId(uuid::Uuid::now_v7().to_string());
        let created_at = now_unix_ms();
        let new_project = NewProject {
            id: id.clone(),
            name: name.clone(),
            icon: icon.clone(),
            created_at,
        };
        let mut writer = self.persistence.writer().await;
        concerto_persist::projects::insert(&mut writer, new_project)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(ProtoProject {
            id: id.0,
            name,
            icon,
            created_at: Some(epoch_ms_to_ts(created_at)),
            archived_at: None,
        }))
    }
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Convert a persisted `Project` into the wire shape.
fn project_to_proto(row: concerto_persist::Project) -> ProtoProject {
    ProtoProject {
        id: row.id.to_string(),
        name: row.name,
        icon: row.icon,
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
