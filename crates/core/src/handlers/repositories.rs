//! gRPC `Repositories` service handler (Task 18).
//!
//! Translates `concerto.v1.Repositories` requests into calls against
//! [`crate::repo_manager::RepoManager`]. Streaming surface (the
//! `Clone` RPC) follows the Tonic pattern: a `ReceiverStream` over an
//! `mpsc::channel(32)`, where the producer is a spawned task that runs
//! the clone and feeds `CloneProgress` messages as `concerto-gix-wrap`
//! emits them.

use std::pin::Pin;
use std::str::FromStr;

use async_trait::async_trait;
use concerto_gix_wrap::{CloneProgressEvent, CloneStrategy};
use concerto_persist::RepositoryId;
use concerto_proto::v1::repositories_server::Repositories as RepositoriesService;
use concerto_proto::v1::{
    AddRepoRequest, CloneProgress, CloneRequest, EstimateRepoSizeRequest, ListRepositoriesRequest,
    ListRepositoriesResponse, Repository, SizeReport,
};
use futures::Stream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::error_map::error_to_status;
use crate::repo_manager::RepoManager;

/// Implements the generated `Repositories` service trait.
///
/// Constructed with a clone of [`RepoManager`] — the handle is cheap to
/// clone and carries the shared per-repo lock map + persistence handle.
#[derive(Clone)]
pub struct RepositoriesHandler {
    repo_manager: RepoManager,
}

impl RepositoriesHandler {
    pub fn new(repo_manager: RepoManager) -> Self {
        Self { repo_manager }
    }
}

#[async_trait]
impl RepositoriesService for RepositoriesHandler {
    #[tracing::instrument(skip_all, name = "Repositories::AddRepository")]
    async fn add_repository(
        &self,
        request: Request<AddRepoRequest>,
    ) -> Result<Response<Repository>, Status> {
        let req = request.into_inner();
        if req.project_id.is_empty() {
            return Err(Status::invalid_argument("project_id is required"));
        }
        if req.name.is_empty() {
            return Err(Status::invalid_argument("name is required"));
        }
        if req.url.is_empty() {
            return Err(Status::invalid_argument("url is required"));
        }
        // Task 301: parse the wire `clone_strategy` (empty → Full, preserving
        // V0.1 callers). An unrecognized value is INVALID_ARGUMENT, never a
        // silent Full.
        let strategy = CloneStrategy::from_str(&req.clone_strategy)
            .map_err(|e| Status::invalid_argument(e.to_string()))?;
        let row = self
            .repo_manager
            .add_repository(
                &req.project_id,
                &req.name,
                &req.url,
                &req.default_branch,
                strategy,
                req.with_sparse,
            )
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(repository_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Repositories::EstimateRepoSize")]
    async fn estimate_repo_size(
        &self,
        request: Request<EstimateRepoSizeRequest>,
    ) -> Result<Response<SizeReport>, Status> {
        let req = request.into_inner();
        if req.url.is_empty() {
            return Err(Status::invalid_argument("url is required"));
        }
        let report = self
            .repo_manager
            .estimate_size(&req.url)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(SizeReport {
            size_bytes: report.size_bytes,
            object_count: report.object_count,
            branch_count: report.branch_count,
            // `recommended` is one of full|blobless — treeless is never
            // recommended (design/02 §12 R-1).
            recommended_strategy: report.recommended.as_str().to_string(),
            recommend_sparse: report.recommend_sparse,
        }))
    }

    /// Streaming response type for `Repositories.Clone`.
    type CloneStream = Pin<Box<dyn Stream<Item = Result<CloneProgress, Status>> + Send + 'static>>;

    #[tracing::instrument(skip_all, name = "Repositories::Clone")]
    async fn clone(
        &self,
        request: Request<CloneRequest>,
    ) -> Result<Response<Self::CloneStream>, Status> {
        let req = request.into_inner();
        if req.repository_id.is_empty() {
            return Err(Status::invalid_argument("repository_id is required"));
        }
        let id = RepositoryId(req.repository_id);

        // Outbound stream → caller.
        let (out_tx, out_rx) = mpsc::channel::<Result<CloneProgress, Status>>(32);
        // Internal progress channel: gix-wrap → adapter → out_tx.
        let (ev_tx, mut ev_rx) = mpsc::channel::<CloneProgressEvent>(32);

        // Forwarder: drains progress events and re-shapes them into
        // gRPC `CloneProgress` messages.
        let out_tx_for_forward = out_tx.clone();
        let forward_handle = tokio::spawn(async move {
            while let Some(ev) = ev_rx.recv().await {
                let msg = CloneProgress {
                    phase: ev.phase,
                    objects_received: ev.objects_received,
                    total_objects: ev.total_objects,
                    bytes_received: ev.bytes_received,
                    done: ev.done,
                };
                // Closed receiver = client disconnected; bail.
                if out_tx_for_forward.send(Ok(msg)).await.is_err() {
                    break;
                }
            }
        });

        // Worker: drives the actual clone. Closes the event channel
        // when done (intrinsically by dropping `ev_tx`).
        let manager = self.repo_manager.clone();
        let out_tx_for_worker = out_tx;
        tokio::spawn(async move {
            let result = manager.clone_repo(&id, Some(ev_tx)).await;
            // Wait for the forwarder to drain so the terminal `done`
            // event makes it out before we send the final result.
            let _ = forward_handle.await;
            if let Err(err) = result {
                let _ = out_tx_for_worker.send(Err(error_to_status(err))).await;
            }
            // Closing `out_tx_for_worker` (via drop) terminates the
            // stream on the client side.
        });

        let stream: Self::CloneStream = Box::pin(ReceiverStream::new(out_rx));
        Ok(Response::new(stream))
    }

    #[tracing::instrument(skip_all, name = "Repositories::ListByProject")]
    async fn list_by_project(
        &self,
        request: Request<ListRepositoriesRequest>,
    ) -> Result<Response<ListRepositoriesResponse>, Status> {
        let req = request.into_inner();
        if req.project_id.is_empty() {
            return Err(Status::invalid_argument("project_id is required"));
        }
        let rows = self
            .repo_manager
            .list_by_project(&req.project_id)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(ListRepositoriesResponse {
            repositories: rows.into_iter().map(repository_to_proto).collect(),
        }))
    }
}

/// Convert a persisted `Repository` into the wire shape.
fn repository_to_proto(row: concerto_persist::Repository) -> Repository {
    Repository {
        id: row.id.to_string(),
        project_id: row.project_id,
        name: row.name,
        url: row.url,
        local_path: row.local_path,
        clone_strategy: row.clone_strategy,
        default_branch: row.default_branch,
        last_fetch_at: row.last_fetch_at.map(|ms| prost_types::Timestamp {
            seconds: ms / 1000,
            nanos: ((ms % 1000) * 1_000_000) as i32,
        }),
    }
}
