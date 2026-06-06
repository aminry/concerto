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

use std::sync::Arc;

use async_trait::async_trait;
use concerto_keychain::{SecretKind, SecretValue, Secrets, VcsSecretSlot};
use concerto_persist::{
    NewVcsCredential, PullRequest as PersistPullRequest, RepositoryId as PersistRepositoryId,
    VcsCredentialId, WorkareaId as PersistWorkareaId,
};
use concerto_proto::v1::vcs_server::Vcs as VcsService;
use concerto_proto::v1::{
    CheckRun as ProtoCheckRun, CreatePrRequest, Deployment as ProtoDeployment,
    FetchIssueByUrlRequest, FetchIssueRequest, GetChecksRequest, GetChecksResponse, GetPrRequest,
    Issue as ProtoIssue, ListDeploymentsRequest, ListDeploymentsResponse, ListReviewThreadsRequest,
    ListReviewThreadsResponse, MergePrRequest, PullRequest as ProtoPullRequest,
    ResolveThreadRequest, ReviewThread as ProtoReviewThread,
    ReviewThreadComment as ProtoReviewThreadComment, SendThreadToAgentRequest,
    SetVcsCredentialRequest, VcsCredentialProvider,
};
use concerto_vcs::provider::{Deployment as VcsDeployment, ProviderPrId, ReviewThread, ThreadId};
use concerto_vcs::{Issue as VcsIssue, IssueFetchCreds};
use tonic::{Request, Response, Status};

use crate::error_map::error_to_status;
use crate::vcs::gh_cli;
use crate::vcs::VcsHandle;

/// A sink the "Send to agent" path posts the composed message to (Task 316,
/// `design/13 §3.6`). Decouples the cross-platform `Vcs` handler from the
/// `#[cfg(unix)]` agent supervisor — on unix the api_server wires an adapter
/// over `AgentSupervisorHandle::send_input`; elsewhere it is `None` and the RPC
/// returns `UNIMPLEMENTED`.
#[async_trait]
pub trait SessionMessageSink: Send + Sync + 'static {
    /// Deliver `message` to `session_id` (the agent's stdin). Returns the
    /// underlying error untouched so the handler can map it to a `Status`.
    async fn send(&self, session_id: &str, message: &str) -> concerto_error::Result<()>;
}

/// The unix [`SessionMessageSink`] adapter over the agent supervisor (Task
/// 316). Posts the composed thread message to the target session's stdin via
/// `AgentSupervisorHandle::send_input`. Unix-only (the supervisor is gated).
#[cfg(unix)]
pub struct AgentSupervisorSink {
    supervisor: crate::agent_supervisor::AgentSupervisorHandle,
}

#[cfg(unix)]
impl AgentSupervisorSink {
    pub fn new(supervisor: crate::agent_supervisor::AgentSupervisorHandle) -> Self {
        Self { supervisor }
    }
}

#[cfg(unix)]
#[async_trait]
impl SessionMessageSink for AgentSupervisorSink {
    async fn send(&self, session_id: &str, message: &str) -> concerto_error::Result<()> {
        let sid = concerto_persist::SessionId(session_id.to_string());
        self.supervisor
            .send_input(&sid, message.as_bytes().to_vec())
            .await
    }
}

/// Implements the generated `Vcs` service trait.
#[derive(Clone)]
pub struct VcsHandler {
    vcs: VcsHandle,
    /// Optional "Send to agent" sink (Task 316). `None` on non-unix targets
    /// (no agent supervisor) → `SendThreadToAgent` returns `UNIMPLEMENTED`.
    session_sink: Option<Arc<dyn SessionMessageSink>>,
}

impl VcsHandler {
    pub fn new(vcs: VcsHandle) -> Self {
        Self {
            vcs,
            session_sink: None,
        }
    }

    /// Attach the "Send to agent" session sink (Task 316). Wired on unix from
    /// the agent supervisor; returns `self` for chaining.
    pub fn with_session_sink(mut self, sink: Arc<dyn SessionMessageSink>) -> Self {
        self.session_sink = Some(sink);
        self
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

    #[tracing::instrument(skip_all, name = "Vcs::FetchIssueByUrl")]
    async fn fetch_issue_by_url(
        &self,
        request: Request<FetchIssueByUrlRequest>,
    ) -> Result<Response<ProtoIssue>, Status> {
        let req = request.into_inner();
        if req.url.is_empty() {
            return Err(Status::invalid_argument("url is required"));
        }

        // Resolve the GitHub PAT (existing slot) + the Linear/Jira OAuth tokens
        // (313's `VcsSecretSlot` keychain accessor). Tokens are wrapped in
        // `SecretValue` end-to-end and never logged. The `enterprise_data_privacy`
        // gate is consulted before any external-tracker call; without task 310's
        // resolver in this build, an external-tracker fetch is allowed by default
        // (the privacy floor is enforced once the resolver lands — see Handoff).
        let secrets = Secrets::new();
        let github_pat = secrets
            .get(SecretKind::GithubPat)
            .await
            .map_err(|e| error_to_status(e.into()))?;

        // The provider account id keys the Linear/Jira keychain slots. The
        // Desktop stored exactly one per provider via `SetVcsCredential`; we use
        // the most-recently-updated `vcs_credentials` row for the provider as the
        // scope id (V1.0 supports a single connected Linear/Jira account).
        let linear_token = self
            .load_provider_token(&secrets, "linear", VcsSecretSlot::LinearAccessToken)
            .await
            .map_err(error_to_status)?;
        let jira_token = self
            .load_provider_token(&secrets, "jira", VcsSecretSlot::JiraAccessToken)
            .await
            .map_err(error_to_status)?;

        let creds = IssueFetchCreds {
            github_token: github_pat.as_ref().map(|s| s.expose()),
            linear_token: linear_token.as_ref(),
            jira_token: jira_token.as_ref(),
            // Jira refresh + base default to the URL's Atlassian site; the
            // Desktop-mediated refresh re-stores the token via SetVcsCredential.
            jira_refresh: None,
            linear_base: None,
            jira_base: None,
            // No task-310 resolver wired here yet (see Handoff "Open questions").
            enterprise_data_privacy: false,
        };

        let issue = self
            .vcs
            .fetch_issue_url(&req.url, &creds)
            .await
            .map_err(error_to_status)?
            .ok_or_else(|| Status::not_found(format!("no issue at url {}", req.url)))?;
        Ok(Response::new(vcs_issue_to_proto(issue)))
    }

    #[tracing::instrument(skip_all, name = "Vcs::SetVcsCredential")]
    async fn set_vcs_credential(
        &self,
        request: Request<SetVcsCredentialRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.account_id.is_empty() {
            return Err(Status::invalid_argument("account_id is required"));
        }
        if req.access_token.is_empty() {
            return Err(Status::invalid_argument("access_token is required"));
        }
        let (provider_str, access_slot, refresh_slot) =
            match VcsCredentialProvider::try_from(req.provider)
                .unwrap_or(VcsCredentialProvider::Unspecified)
            {
                VcsCredentialProvider::Linear => (
                    "linear",
                    VcsSecretSlot::LinearAccessToken,
                    VcsSecretSlot::LinearRefreshToken,
                ),
                VcsCredentialProvider::Jira => (
                    "jira",
                    VcsSecretSlot::JiraAccessToken,
                    VcsSecretSlot::JiraRefreshToken,
                ),
                VcsCredentialProvider::Unspecified => {
                    return Err(Status::invalid_argument("provider must be LINEAR or JIRA"));
                }
            };

        let secrets = Secrets::new();
        // The token is the ONLY cleartext secret here: straight to the keychain,
        // never logged, never SQLite.
        secrets
            .set_vcs_secret(
                &req.account_id,
                access_slot,
                SecretValue::new(req.access_token),
            )
            .await
            .map_err(|e| error_to_status(e.into()))?;
        if let Some(refresh) = req.refresh_token.filter(|r| !r.is_empty()) {
            secrets
                .set_vcs_secret(&req.account_id, refresh_slot, SecretValue::new(refresh))
                .await
                .map_err(|e| error_to_status(e.into()))?;
        }

        // Non-secret metadata → the `vcs_credentials` table (313, migration 0012).
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let row = NewVcsCredential {
            id: VcsCredentialId(uuid::Uuid::now_v7().to_string()),
            provider: provider_str.to_string(),
            scope_id: req.account_id.clone(),
            external_account: Some(req.account_id),
            app_id: None,
            installation_id: None,
            token_expires_at: req.expires_at,
            created_at: now_ms,
            updated_at: now_ms,
        };
        {
            let mut writer = self.vcs.persistence_writer().await;
            concerto_persist::vcs_credentials::upsert(&mut writer, row)
                .await
                .map_err(error_to_status)?;
        }
        Ok(Response::new(()))
    }

    #[tracing::instrument(skip_all, name = "Vcs::ListReviewThreads")]
    async fn list_review_threads(
        &self,
        request: Request<ListReviewThreadsRequest>,
    ) -> Result<Response<ListReviewThreadsResponse>, Status> {
        let req = request.into_inner();
        if req.repository_id.is_empty() {
            return Err(Status::invalid_argument("repository_id is required"));
        }
        if req.pr_number <= 0 {
            return Err(Status::invalid_argument("pr_number must be > 0"));
        }
        let repo_id = PersistRepositoryId(req.repository_id.clone());
        let (provider, repo_full) = self.provider_for_repo(&repo_id).await?;
        let pr = ProviderPrId::new(repo_full, req.pr_number);
        // Refresh-on-open: fetch via GraphQL, fill the cache, emit per change.
        let threads = self
            .vcs
            .checks()
            .list_review_threads(&provider, &req.workarea_id, &req.repository_id, pr)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(ListReviewThreadsResponse {
            threads: threads.into_iter().map(review_thread_to_proto).collect(),
        }))
    }

    #[tracing::instrument(skip_all, name = "Vcs::ResolveThread")]
    async fn resolve_thread(
        &self,
        request: Request<ResolveThreadRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.repository_id.is_empty() {
            return Err(Status::invalid_argument("repository_id is required"));
        }
        if req.thread_id.is_empty() {
            return Err(Status::invalid_argument("thread_id is required"));
        }
        let repo_id = PersistRepositoryId(req.repository_id.clone());
        let (provider, repo_full) = self.provider_for_repo(&repo_id).await?;
        // The mutation keys only on the thread node id; the PR id locates the
        // cached thread set to flip + frame (number 0 is fine if uncached).
        let pr = ProviderPrId::new(repo_full, 0);
        self.vcs
            .checks()
            .resolve_thread(
                &provider,
                &req.workarea_id,
                &req.repository_id,
                &pr,
                ThreadId(req.thread_id),
            )
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }

    #[tracing::instrument(skip_all, name = "Vcs::ListDeployments")]
    async fn list_deployments(
        &self,
        request: Request<ListDeploymentsRequest>,
    ) -> Result<Response<ListDeploymentsResponse>, Status> {
        let req = request.into_inner();
        if req.repository_id.is_empty() {
            return Err(Status::invalid_argument("repository_id is required"));
        }
        if req.r#ref.is_empty() {
            return Err(Status::invalid_argument("ref is required"));
        }
        let repo_id = PersistRepositoryId(req.repository_id.clone());
        let (provider, repo_full) = self.provider_for_repo(&repo_id).await?;
        let deployments = self
            .vcs
            .checks()
            .list_deployments(
                &provider,
                &req.workarea_id,
                &req.repository_id,
                &repo_full,
                &req.r#ref,
            )
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(ListDeploymentsResponse {
            deployments: deployments.into_iter().map(deployment_to_proto).collect(),
        }))
    }

    #[tracing::instrument(skip_all, name = "Vcs::SendThreadToAgent")]
    async fn send_thread_to_agent(
        &self,
        request: Request<SendThreadToAgentRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.repository_id.is_empty() {
            return Err(Status::invalid_argument("repository_id is required"));
        }
        if req.thread_id.is_empty() {
            return Err(Status::invalid_argument("thread_id is required"));
        }
        if req.session_id.is_empty() {
            return Err(Status::invalid_argument("session_id is required"));
        }
        let sink = self.session_sink.as_ref().ok_or_else(|| {
            Status::unimplemented(
                "SendThreadToAgent needs the agent supervisor (not available on this Core target)",
            )
        })?;
        let repo_id = PersistRepositoryId(req.repository_id.clone());
        let (provider, repo_full) = self.provider_for_repo(&repo_id).await?;
        let pr = ProviderPrId::new(repo_full, req.pr_number);

        // Prefer the cached thread set (refresh-on-open already populated it);
        // fall back to a fresh GraphQL fetch when the cache is cold.
        let threads = match self.vcs.checks().cached_threads(&pr) {
            Some(t) => t,
            None => self
                .vcs
                .checks()
                .list_review_threads(&provider, &req.workarea_id, &req.repository_id, pr)
                .await
                .map_err(error_to_status)?,
        };
        let thread = threads
            .into_iter()
            .find(|t| t.id == ThreadId(req.thread_id.clone()))
            .ok_or_else(|| {
                Status::not_found(format!("review thread {} not found", req.thread_id))
            })?;

        let message = compose_thread_message(&thread);
        sink.send(&req.session_id, &message)
            .await
            .map_err(error_to_status)?;
        Ok(Response::new(()))
    }
}

impl VcsHandler {
    /// Build the octocrab provider for `repo_id` from the keychain GitHub PAT +
    /// return it with the repo's `owner/repo` full name. The GraphQL
    /// review-thread + REST check/deploy paths are octocrab-only.
    async fn provider_for_repo(
        &self,
        repo_id: &PersistRepositoryId,
    ) -> Result<(Arc<dyn concerto_vcs::provider::VcsProvider>, String), Status> {
        let repo_full = self
            .vcs
            .repo_full_name(repo_id)
            .await
            .map_err(error_to_status)?;
        let secrets = Secrets::new();
        let pat = secrets
            .get(SecretKind::GithubPat)
            .await
            .map_err(|e| error_to_status(e.into()))?
            .ok_or_else(|| {
                Status::failed_precondition(
                    "no GitHub PAT configured (SecretKind::GithubPat); connect GitHub in Settings",
                )
            })?;
        let provider = self
            .vcs
            .github_provider(pat.expose())
            .await
            .map_err(error_to_status)?;
        Ok((provider, repo_full))
    }
}

impl VcsHandler {
    /// Load a provider's keychain access token, keyed by the most-recently-set
    /// `vcs_credentials` scope id for that provider (V1.0 supports one connected
    /// Linear/Jira account). Returns `None` when no credential is configured.
    async fn load_provider_token(
        &self,
        secrets: &Secrets,
        provider: &str,
        slot: VcsSecretSlot,
    ) -> concerto_error::Result<Option<SecretValue>> {
        let creds = concerto_persist::vcs_credentials::list_by_provider(
            self.vcs.persistence_readers(),
            provider,
        )
        .await?;
        // Pick the most-recently-updated row (deterministic single-account V1.0).
        let scope = creds
            .into_iter()
            .max_by_key(|c| c.updated_at)
            .map(|c| c.scope_id);
        match scope {
            Some(scope_id) => Ok(secrets.get_vcs_secret(&scope_id, slot).await?),
            None => Ok(None),
        }
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
        // The V0.1 GitHub-by-(repo,number) path has no provider-native string id.
        external_id: String::new(),
    }
}

/// Map the shared `crates/vcs` `Issue` value type to the proto `Issue` (Task
/// 317). Linear/Jira set `number = 0` and carry their string id in `external_id`.
fn vcs_issue_to_proto(issue: VcsIssue) -> ProtoIssue {
    ProtoIssue {
        number: issue.number,
        title: issue.title,
        body: issue.body,
        state: issue.state,
        url: issue.url,
        labels: issue.labels,
        external_id: issue.external_id,
    }
}

/// Map a `crates/vcs` [`ReviewThread`] to the proto `ReviewThread` (Task 316).
/// The value type carries only comment bodies; the author is unknown at this
/// layer, so the proto comment's `author` is left empty (the GraphQL query
/// fetches authors, but the FROZEN value type does not carry them — extending
/// it is out of this task's scope per the 313 contract; documented in Handoff).
fn review_thread_to_proto(thread: ReviewThread) -> ProtoReviewThread {
    ProtoReviewThread {
        id: thread.id.0,
        resolved: thread.resolved,
        path: thread.path.unwrap_or_default(),
        comments: thread
            .comments
            .into_iter()
            .map(|body| ProtoReviewThreadComment {
                author: String::new(),
                body,
            })
            .collect(),
    }
}

/// Map a `crates/vcs` [`VcsDeployment`] to the proto `Deployment` (Task 316).
fn deployment_to_proto(d: VcsDeployment) -> ProtoDeployment {
    ProtoDeployment {
        id: d.id,
        environment: d.environment,
        state: d.state,
        r#ref: d.ref_,
    }
}

/// Compose the "Send to agent" message body from a review thread's context
/// (Task 316, `design/13 §3.6`). Deterministic + minimal: the anchor path + the
/// thread's comments, oldest first, as a plain-text block the agent can act on.
fn compose_thread_message(thread: &ReviewThread) -> String {
    let mut out = String::new();
    out.push_str("Please address this PR review thread");
    if let Some(path) = &thread.path {
        out.push_str(" on `");
        out.push_str(path);
        out.push('`');
    }
    out.push_str(":\n\n");
    for comment in &thread.comments {
        out.push_str("- ");
        out.push_str(comment);
        out.push('\n');
    }
    out
}
