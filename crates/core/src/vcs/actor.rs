//! `VcsProviderActor` + cloneable [`VcsHandle`] (Task 45).
//!
//! Same actor pattern as the other Core managers — the actor's `run`
//! parks on shutdown; all meaningful work flows through the cheap-to-
//! clone handle. V0.1 ships exactly one backend: `gh` CLI shell-out
//! (per `design/13 §2` V0.1 row).
//!
//! ## Public surface (frozen at Task 45)
//!
//! - [`VcsHandle::create_pr`] — `gh pr create`, persists row.
//! - [`VcsHandle::get_pr`] — `gh pr view`, refreshes row.
//! - [`VcsHandle::list_prs`] — `gh pr list`, no DB write.
//! - [`VcsHandle::merge_pr`] — `gh pr merge`.
//! - [`VcsHandle::get_check_runs`] — `gh api …/check-runs`.
//! - [`VcsHandle::fetch_issue`] — `gh issue view`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use concerto_error::{Error, Result};
use concerto_persist::{
    NewPullRequest, Persistence, PullRequest, PullRequestId, Repository, RepositoryId, WorkareaId,
};

use super::gh_cli;
use crate::supervisor::{Actor, ActorContext};

/// Config for the actor's `run` loop. V0.1 has no knobs — the actor
/// parks on shutdown.
#[derive(Clone, Debug, Default)]
pub struct VcsConfig;

/// Cheap-cloneable, shareable handle to the VCS provider.
///
/// Cloning is `O(Arc)`; both the persistence handle and the resolved
/// `gh` path are shared across clones. The `gh` path is resolved
/// lazily on first use so a Core that never touches VCS doesn't pay
/// the PATH-walk cost.
#[derive(Clone)]
pub struct VcsHandle {
    persistence: Arc<Persistence>,
    /// `Some` once `gh` is resolved on PATH; cached so subsequent
    /// calls skip the walk. `None` until the first lookup. Wrapped in
    /// a `tokio::sync::OnceCell` so the resolution races to completion
    /// across concurrent callers without locking the steady-state
    /// path.
    gh_path: Arc<tokio::sync::OnceCell<PathBuf>>,
}

impl VcsHandle {
    /// Build a fresh handle. The `gh` path is NOT resolved here; it
    /// is resolved on first call via [`Self::gh`].
    pub fn new(persistence: Arc<Persistence>) -> Self {
        Self {
            persistence,
            gh_path: Arc::new(tokio::sync::OnceCell::new()),
        }
    }

    /// Borrow the shared read-only pool. Used by the gRPC `Vcs`
    /// handler to satisfy `GetPullRequest`'s repo-id → workarea-id
    /// lookup without plumbing a separate `Arc<Persistence>` through.
    pub fn persistence_readers(&self) -> &sqlx::SqlitePool {
        self.persistence.readers()
    }

    /// Resolve (and cache) the `gh` binary path. Returns
    /// [`Error::Internal`] when `gh` is not on PATH.
    pub async fn gh(&self) -> Result<&std::path::Path> {
        let path = self
            .gh_path
            .get_or_try_init(|| async { gh_cli::resolve_gh_path() })
            .await?;
        Ok(path.as_path())
    }

    /// Probe authentication once. Returns
    /// [`Error::VcsNotAuthenticated`] on failure. The Core boot path
    /// MAY call this opportunistically; per-RPC callers do not need
    /// to call it (each `gh` invocation surfaces the same error if
    /// auth has lapsed).
    pub async fn check_auth(&self) -> Result<()> {
        let gh = self.gh().await?;
        gh_cli::check_auth(gh).await
    }

    /// List PRs in `repository_id`'s upstream repo via `gh pr list`.
    /// Does not touch the cache.
    pub async fn list_prs(&self, repository_id: &RepositoryId) -> Result<Vec<gh_cli::PrSummary>> {
        let gh = self.gh().await?;
        let repo = self.resolve_repo_full_name(repository_id).await?;
        gh_cli::list_prs(gh, &repo).await
    }

    /// Look up one PR by number, upsert the cache row keyed by
    /// `(workarea_id, repository_id)`, and return the cached projection.
    pub async fn get_pr(
        &self,
        workarea_id: &WorkareaId,
        repository_id: &RepositoryId,
        pr_number: i64,
    ) -> Result<PullRequest> {
        let gh = self.gh().await?;
        let repo = self.resolve_repo_full_name(repository_id).await?;
        let detail = gh_cli::view_pr(gh, &repo, pr_number).await?;
        self.upsert_from_detail(workarea_id, repository_id, &detail)
            .await
    }

    /// Create a PR for `head_ref` against `base_ref` and persist the
    /// resulting cache row.
    pub async fn create_pr(
        &self,
        workarea_id: &WorkareaId,
        repository_id: &RepositoryId,
        base_ref: &str,
        head_ref: &str,
        title: &str,
        body: &str,
    ) -> Result<PullRequest> {
        let gh = self.gh().await?;
        let repository = self.load_repository(repository_id).await?;
        let repo_name = repo_full_name_from_url(&repository.url).ok_or_else(|| {
            Error::Validation(format!(
                "repository {repository_id} url `{}` is not a parseable github.com URL",
                repository.url
            ))
        })?;
        let base = if base_ref.is_empty() {
            repository.default_branch.as_str()
        } else {
            base_ref
        };
        let number = gh_cli::create_pr(gh, &repo_name, head_ref, base, title, body).await?;

        // Re-query to pick up the head SHA + the canonical body the
        // server stored.
        let detail = gh_cli::view_pr(gh, &repo_name, number).await?;
        self.upsert_from_detail(workarea_id, repository_id, &detail)
            .await
    }

    /// Merge an existing PR. `method` ∈ `merge|squash|rebase`.
    pub async fn merge_pr(
        &self,
        repository_id: &RepositoryId,
        pr_number: i64,
        method: &str,
    ) -> Result<()> {
        let gh = self.gh().await?;
        let repo = self.resolve_repo_full_name(repository_id).await?;
        gh_cli::merge_pr(gh, &repo, pr_number, method).await
    }

    /// List check runs for a commit SHA on the given repo.
    pub async fn get_check_runs(
        &self,
        repository_id: &RepositoryId,
        sha: &str,
    ) -> Result<Vec<gh_cli::CheckRun>> {
        let gh = self.gh().await?;
        let repo = self.resolve_repo_full_name(repository_id).await?;
        gh_cli::get_check_runs(gh, &repo, sha).await
    }

    /// Fetch a GitHub issue via `gh issue view`. The repository's
    /// upstream URL is used as the host context.
    pub async fn fetch_issue(
        &self,
        repository_id: &RepositoryId,
        issue_number: i64,
    ) -> Result<gh_cli::IssueDetail> {
        let gh = self.gh().await?;
        let repo = self.resolve_repo_full_name(repository_id).await?;
        gh_cli::view_issue(gh, &repo, issue_number).await
    }

    // ---- internal helpers ----

    async fn load_repository(&self, id: &RepositoryId) -> Result<Repository> {
        concerto_persist::repositories::get(self.persistence.readers(), id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("repository {id} not found")))
    }

    /// Resolve the `owner/repo` GitHub identifier for `repository_id`.
    async fn resolve_repo_full_name(&self, id: &RepositoryId) -> Result<String> {
        let row = self.load_repository(id).await?;
        repo_full_name_from_url(&row.url).ok_or_else(|| {
            Error::Validation(format!(
                "repository {id} url `{}` is not a parseable github.com URL",
                row.url
            ))
        })
    }

    async fn upsert_from_detail(
        &self,
        workarea_id: &WorkareaId,
        repository_id: &RepositoryId,
        detail: &gh_cli::PrDetail,
    ) -> Result<PullRequest> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let new_id = PullRequestId(uuid::Uuid::now_v7().to_string());
        let row = NewPullRequest {
            id: new_id,
            workarea_id: workarea_id.clone(),
            repository_id: repository_id.clone(),
            provider: "github".to_string(),
            pr_number: detail.number,
            base_ref: detail.base_ref_name.clone(),
            head_ref: detail.head_ref_name.clone(),
            state: detail.state.to_lowercase(),
            title: detail.title.clone(),
            body: detail.body.clone(),
            url: detail.url.clone(),
            head_sha: detail.head_ref_oid.clone(),
            created_at: now_ms,
            updated_at: now_ms,
        };
        let id = {
            let mut writer = self.persistence.writer().await;
            concerto_persist::pull_requests::upsert(&mut writer, row).await?
        };
        concerto_persist::pull_requests::get(self.persistence.readers(), &id)
            .await?
            .ok_or_else(|| Error::Internal(format!("pull_request {id} missing after upsert")))
    }
}

/// Supervised actor that owns the [`VcsHandle`]. `run` parks on
/// shutdown; the supervisor's factory clones the handle on each
/// restart so the cached `gh` path survives a wrapper panic.
pub struct VcsProviderActor {
    handle: VcsHandle,
}

impl VcsProviderActor {
    /// Build a fresh actor with a new handle.
    pub fn new(persistence: Arc<Persistence>) -> Self {
        Self {
            handle: VcsHandle::new(persistence),
        }
    }

    /// Cheap clone of the shared handle.
    pub fn handle(&self) -> VcsHandle {
        self.handle.clone()
    }
}

#[async_trait]
impl Actor for VcsProviderActor {
    const NAME: &'static str = "vcs-provider";
    type Config = VcsConfig;

    async fn run(self, ctx: ActorContext<Self::Config>) -> Result<()> {
        tracing::info!("VCS provider ready (gh CLI backend)");
        ctx.shutdown.cancelled().await;
        tracing::debug!("VCS provider actor shutting down");
        Ok(())
    }
}

/// Extract `owner/repo` from a GitHub URL of any common shape
/// (`https://github.com/owner/repo.git`, `git@github.com:owner/repo`,
/// `https://github.com/owner/repo`).
///
/// V0.1 only handles github.com URLs — V2.0's GitLab / Bitbucket
/// adapters will dispatch on the host before reaching this helper.
pub(crate) fn repo_full_name_from_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let stripped = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    // SSH form: `git@github.com:owner/repo`
    if let Some(after_colon) = stripped.strip_prefix("git@github.com:") {
        if after_colon.matches('/').count() == 1 {
            return Some(after_colon.to_string());
        }
    }
    // HTTPS form: `https://github.com/owner/repo` (also handles http://)
    for prefix in ["https://github.com/", "http://github.com/"] {
        if let Some(rest) = stripped.strip_prefix(prefix) {
            // Drop any trailing path / query.
            let head: String = rest
                .split(['/', '#', '?'])
                .take(2)
                .collect::<Vec<_>>()
                .join("/");
            if head.matches('/').count() == 1 && !head.is_empty() {
                return Some(head);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_https_url() {
        assert_eq!(
            repo_full_name_from_url("https://github.com/owner/repo.git"),
            Some("owner/repo".to_string())
        );
        assert_eq!(
            repo_full_name_from_url("https://github.com/owner/repo"),
            Some("owner/repo".to_string())
        );
    }

    #[test]
    fn parses_ssh_url() {
        assert_eq!(
            repo_full_name_from_url("git@github.com:owner/repo.git"),
            Some("owner/repo".to_string())
        );
    }

    #[test]
    fn rejects_non_github_url() {
        assert_eq!(
            repo_full_name_from_url("https://gitlab.com/owner/repo"),
            None
        );
    }
}
