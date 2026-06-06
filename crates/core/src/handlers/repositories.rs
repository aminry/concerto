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
use concerto_gix_wrap::{CloneProgressEvent, CloneStrategy, PrewarmProgressEvent};
use concerto_persist::{RepositoryId, WorkareaId};
use concerto_proto::v1::repositories_server::Repositories as RepositoriesService;
use concerto_proto::v1::{
    AddRepoRequest, CloneProgress, CloneRequest, ConeStats, EstimateConeSizeRequest,
    EstimateRepoSizeRequest, ListRepositoriesRequest, ListRepositoriesResponse, PrewarmProgress,
    PrewarmRequest, Repository, SetConesRequest, SetConesResponse, SizeReport,
};
use futures::Stream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

use crate::error_map::error_to_status;
use crate::repo_manager::{ConeSuggestError, RepoManager};

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

    #[tracing::instrument(skip_all, name = "Repositories::SetCones")]
    async fn set_cones(
        &self,
        request: Request<SetConesRequest>,
    ) -> Result<Response<SetConesResponse>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        if req.repository_id.is_empty() {
            return Err(Status::invalid_argument("repository_id is required"));
        }
        let workarea = WorkareaId(req.workarea_id);
        let repo = RepositoryId(req.repository_id);
        // Task 302: apply + persist the cone. A bad cone path surfaces as
        // `Error::Git` from `sparse_set`'s pre-apply probe, which
        // `error_to_status` maps to INVALID_ARGUMENT; nothing is
        // half-applied.
        self.repo_manager
            .set_workarea_repo_cones(&workarea, &repo, &req.cone_paths)
            .await
            .map_err(error_to_status)?;
        // Echo back the applied cone set (the same paths now materialized +
        // persisted).
        Ok(Response::new(SetConesResponse {
            cone_paths: req.cone_paths,
        }))
    }

    #[tracing::instrument(skip_all, name = "Repositories::EstimateConeSize")]
    async fn estimate_cone_size(
        &self,
        request: Request<EstimateConeSizeRequest>,
    ) -> Result<Response<ConeStats>, Status> {
        let req = request.into_inner();
        if req.repository_id.is_empty() {
            return Err(Status::invalid_argument("repository_id is required"));
        }
        let repo = RepositoryId(req.repository_id);
        // Task 305: read the git index for the in-cone file count + disk-size
        // estimate. Empty `cone_paths` falls back to the repo cone defaults,
        // then to the whole tree (see `list_paths_in_cone`).
        let stats = self
            .repo_manager
            .list_paths_in_cone(&repo, &req.cone_paths)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(ConeStats {
            file_count: stats.file_count,
            disk_size_bytes: stats.disk_size_bytes,
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

    /// Streaming response type for `Repositories.PrewarmBlobs` (Task 304).
    type PrewarmBlobsStream =
        Pin<Box<dyn Stream<Item = Result<PrewarmProgress, Status>> + Send + 'static>>;

    #[tracing::instrument(skip_all, name = "Repositories::PrewarmBlobs")]
    async fn prewarm_blobs(
        &self,
        request: Request<PrewarmRequest>,
    ) -> Result<Response<Self::PrewarmBlobsStream>, Status> {
        let req = request.into_inner();
        if req.repository_id.is_empty() {
            return Err(Status::invalid_argument("repository_id is required"));
        }
        if req.commit.is_empty() {
            return Err(Status::invalid_argument("commit is required"));
        }
        let id = RepositoryId(req.repository_id);

        // Outbound stream → caller (mirrors the `Clone` streaming pattern).
        let (out_tx, out_rx) = mpsc::channel::<Result<PrewarmProgress, Status>>(32);
        // Internal progress channel: gix-wrap → adapter → out_tx.
        let (ev_tx, mut ev_rx) = mpsc::channel::<PrewarmProgressEvent>(32);

        // Forwarder: reshapes prewarm progress into gRPC `PrewarmProgress`.
        let out_tx_for_forward = out_tx.clone();
        let forward_handle = tokio::spawn(async move {
            while let Some(ev) = ev_rx.recv().await {
                let msg = PrewarmProgress {
                    blobs_fetched: ev.blobs_fetched,
                    blobs_total: ev.blobs_total,
                    done: ev.done,
                };
                if out_tx_for_forward.send(Ok(msg)).await.is_err() {
                    break; // client disconnected
                }
            }
        });

        // Kick the prewarm. The returned handle's CancellationToken fires on
        // drop, so when the worker task below finishes (after the forwarder
        // drains) the handle drops and any still-running fetch is cancelled —
        // covering the client-disconnect case.
        let manager = self.repo_manager.clone();
        let out_tx_for_worker = out_tx;
        tokio::spawn(async move {
            let handle = match manager
                .prewarm_blobs_streaming(&id, &req.cone_paths, &req.commit, ev_tx)
                .await
            {
                Ok(h) => h,
                Err(err) => {
                    let _ = out_tx_for_worker.send(Err(error_to_status(err))).await;
                    return;
                }
            };
            // Wait for the prewarm job to finish, then drain the forwarder so
            // the terminal `done` event reaches the client before the stream
            // closes.
            handle.join().await;
            let _ = forward_handle.await;
            // Dropping `out_tx_for_worker` terminates the client stream.
        });

        let stream: Self::PrewarmBlobsStream = Box::pin(ReceiverStream::new(out_rx));
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

/// Map a [`ConeSuggestError`] to a gRPC [`Status`] (Task 305, D1).
///
/// The FROZEN contract: an **unwired** `suggest_cones` seam ([`ConeSuggestError::Unwired`])
/// surfaces as `Status::unimplemented` — NOT an empty success and NOT a panic
/// — so the (P4, Task 411) Maestro wiring is a pure addition. A real
/// delegation failure once a suggester is injected flows through the usual
/// [`error_to_status`] mapping.
///
/// `suggest_cones` has no gRPC RPC in P3 (`PHASE3_PLANNING` scopes 305's
/// `suggest_cones` to a Rust trait seam only); this helper is the handler-layer
/// mapping the P4 RPC will reuse verbatim, and the Tier-1 test asserts it
/// against the unwired + injected-mock paths.
pub fn cone_suggest_error_to_status(err: ConeSuggestError) -> Status {
    match err {
        ConeSuggestError::Unwired => {
            Status::unimplemented("suggest_cones is wired in P4 (Maestro, Task 411)")
        }
        ConeSuggestError::Delegate(e) => error_to_status(e),
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
