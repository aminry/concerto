//! gRPC `Vcs` service handler (Task 45).
//!
//! Translates `concerto.v1.Vcs` requests into calls against
//! [`crate::vcs::VcsHandle`]. V0.1 surface mirrors `design/13 §5` with
//! only the five RPCs the GitHub demo path needs:
//!
//! - `GetPullRequest` — `gh pr view` + cache upsert.
//! - `CreatePullRequest` — `gh pr create` + cache upsert.
//! - `MergePullRequest` — `gh pr merge`.
//! - `GetChecks` — `gh api …/check-runs`.
//! - `FetchIssue` — `gh issue view`.

use async_trait::async_trait;
use concerto_persist::{
    PullRequest as PersistPullRequest, RepositoryId as PersistRepositoryId,
    WorkareaId as PersistWorkareaId,
};
use concerto_proto::v1::vcs_server::Vcs as VcsService;
use concerto_proto::v1::{
    CheckRun as ProtoCheckRun, CreatePrRequest, FetchIssueRequest, GetChecksRequest,
    GetChecksResponse, GetPrRequest, Issue as ProtoIssue, MergePrRequest,
    PullRequest as ProtoPullRequest,
};
use tonic::{Request, Response, Status};

use crate::error_map::error_to_status;
use crate::vcs::gh_cli;
use crate::vcs::VcsHandle;

/// Implements the generated `Vcs` service trait.
#[derive(Clone)]
pub struct VcsHandler {
    vcs: VcsHandle,
}

impl VcsHandler {
    pub fn new(vcs: VcsHandle) -> Self {
        Self { vcs }
    }
}

#[async_trait]
impl VcsService for VcsHandler {
    #[tracing::instrument(skip_all, name = "Vcs::GetPullRequest")]
    async fn get_pull_request(
        &self,
        request: Request<GetPrRequest>,
    ) -> Result<Response<ProtoPullRequest>, Status> {
        let req = request.into_inner();
        if req.repository_id.is_empty() {
            return Err(Status::invalid_argument("repository_id is required"));
        }
        if req.pr_number <= 0 {
            return Err(Status::invalid_argument("pr_number must be > 0"));
        }
        // GetPullRequest is a refresh-by-(repo, number); the workarea
        // id is not on the request because the cache row may not exist
        // yet. We resolve the workarea via an existing row if present;
        // otherwise the caller MUST use CreatePullRequest first.
        let repository_id = PersistRepositoryId(req.repository_id.clone());
        let workarea_id = find_workarea_for_pr(&self.vcs, &repository_id, req.pr_number)
            .await
            .map_err(error_to_status)?
            .ok_or_else(|| {
                Status::not_found(format!(
                    "no cached PR for repository {} pr_number {}; \
                     call CreatePullRequest first",
                    req.repository_id, req.pr_number
                ))
            })?;
        let row = self
            .vcs
            .get_pr(&workarea_id, &repository_id, req.pr_number)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(pull_request_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Vcs::CreatePullRequest")]
    async fn create_pull_request(
        &self,
        request: Request<CreatePrRequest>,
    ) -> Result<Response<ProtoPullRequest>, Status> {
        let req = request.into_inner();
        if req.workarea_id.is_empty() {
            return Err(Status::invalid_argument("workarea_id is required"));
        }
        if req.repository_id.is_empty() {
            return Err(Status::invalid_argument("repository_id is required"));
        }
        if req.head.is_empty() {
            return Err(Status::invalid_argument("head is required"));
        }
        if req.title.is_empty() {
            return Err(Status::invalid_argument("title is required"));
        }
        let workarea_id = PersistWorkareaId(req.workarea_id);
        let repository_id = PersistRepositoryId(req.repository_id);
        let row = self
            .vcs
            .create_pr(
                &workarea_id,
                &repository_id,
                &req.base,
                &req.head,
                &req.title,
                &req.body,
            )
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(pull_request_to_proto(row)))
    }

    #[tracing::instrument(skip_all, name = "Vcs::MergePullRequest")]
    async fn merge_pull_request(
        &self,
        request: Request<MergePrRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.repository_id.is_empty() {
            return Err(Status::invalid_argument("repository_id is required"));
        }
        if req.pr_number <= 0 {
            return Err(Status::invalid_argument("pr_number must be > 0"));
        }
        let repository_id = PersistRepositoryId(req.repository_id);
        self.vcs
            .merge_pr(&repository_id, req.pr_number, &req.method)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }

    #[tracing::instrument(skip_all, name = "Vcs::GetChecks")]
    async fn get_checks(
        &self,
        request: Request<GetChecksRequest>,
    ) -> Result<Response<GetChecksResponse>, Status> {
        let req = request.into_inner();
        if req.repository_id.is_empty() {
            return Err(Status::invalid_argument("repository_id is required"));
        }
        if req.sha.is_empty() {
            return Err(Status::invalid_argument("sha is required"));
        }
        let repository_id = PersistRepositoryId(req.repository_id);
        let runs = self
            .vcs
            .get_check_runs(&repository_id, &req.sha)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(GetChecksResponse {
            checks: runs.into_iter().map(check_run_to_proto).collect(),
        }))
    }

    #[tracing::instrument(skip_all, name = "Vcs::FetchIssue")]
    async fn fetch_issue(
        &self,
        request: Request<FetchIssueRequest>,
    ) -> Result<Response<ProtoIssue>, Status> {
        let req = request.into_inner();
        if req.repository_id.is_empty() {
            return Err(Status::invalid_argument("repository_id is required"));
        }
        if req.issue_number <= 0 {
            return Err(Status::invalid_argument("issue_number must be > 0"));
        }
        let repository_id = PersistRepositoryId(req.repository_id);
        let detail = self
            .vcs
            .fetch_issue(&repository_id, req.issue_number)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(issue_to_proto(detail)))
    }
}

/// Look up the workarea id that owns the cached row for
/// `(repository_id, pr_number)`. Returns `None` when no row is
/// cached yet.
async fn find_workarea_for_pr(
    vcs: &VcsHandle,
    repository_id: &PersistRepositoryId,
    pr_number: i64,
) -> concerto_error::Result<Option<PersistWorkareaId>> {
    let pool = vcs.persistence_readers();
    let row = sqlx::query_scalar::<_, String>(
        "SELECT workarea_id FROM pull_requests
         WHERE repository_id = ? AND pr_number = ?
         LIMIT 1",
    )
    .bind(&repository_id.0)
    .bind(pr_number)
    .fetch_optional(pool)
    .await
    .map_err(|e| concerto_error::Error::Sqlx(Box::new(e)))?;
    Ok(row.map(PersistWorkareaId))
}

fn pull_request_to_proto(row: PersistPullRequest) -> ProtoPullRequest {
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

fn check_run_to_proto(run: gh_cli::CheckRun) -> ProtoCheckRun {
    ProtoCheckRun {
        name: run.name,
        status: run.status,
        conclusion: run.conclusion,
        details_url: run.details_url,
    }
}

fn issue_to_proto(detail: gh_cli::IssueDetail) -> ProtoIssue {
    ProtoIssue {
        number: detail.number,
        title: detail.title,
        body: detail.body,
        state: detail.state.to_lowercase(),
        url: detail.url,
        labels: detail.labels.into_iter().map(|l| l.name).collect(),
    }
}
