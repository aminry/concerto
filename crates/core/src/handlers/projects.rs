//! gRPC `Projects` service handler (Task 24).
//!
//! V0.1 surface: a single read RPC, `ListProjects`. Project creation
//! over gRPC is deferred — V0.1 seeds the `projects` table via direct
//! SQL (see `crates/persist/src/projects.rs`). The Desktop sidebar
//! (Task 24) uses this RPC to populate its top-level node without
//! hardcoding a project id.

use std::sync::Arc;

use async_trait::async_trait;
use concerto_persist::Persistence;
use concerto_proto::v1::projects_server::Projects as ProjectsService;
use concerto_proto::v1::{ListProjectsRequest, ListProjectsResponse, Project as ProtoProject};
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
