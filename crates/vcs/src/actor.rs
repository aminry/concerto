//! Cloneable [`VcsHandle`] + [`VcsConfig`] (Task 45, moved to `crates/vcs` by
//! Task 313).
//!
//! The V0.1 `VcsHandle` lived in `crates/core/src/vcs/actor.rs` and shelled out
//! to `gh`. Task 313 moves the handle here so the new `crates/vcs` crate owns
//! the whole VCS surface; the supervised `VcsProviderActor` (which needs the
//! Core's `supervisor::Actor` trait) stays in `crates/core/src/vcs/` and wraps
//! this handle — so `boot.rs` + the `Vcs` gRPC handler compile unchanged.
//!
//! ## Frozen surface (Task 45 — preserved verbatim)
//!
//! The Task-45 `VcsHandle` method signatures (`create_pr`/`get_pr`/`list_prs`/
//! `merge_pr`/`get_check_runs`/`fetch_issue` keyed by `RepositoryId`) are
//! FROZEN and reused by the existing gRPC handler — extend, never break. The
//! handle keeps shelling out to `gh` (the V0.1 behavior) so the V0.1 `Vcs` gRPC
//! path is byte-for-byte unchanged. The new octocrab `GitHubProvider` +
//! `choose_backend` dispatch + the `fetch_issue(url)` router are the *internal*
//! trait surface this task adds; wiring the handle to dispatch through the
//! trait is a follow-on (the gRPC proto is untouched this task).
//!
//! Task 313 adds [`VcsHandle::fetch_issue_url`] — the top-level URL-host router
//! (`design/13 §6.1`): github.com → the GitHub issue fetch; linear.app /
//! *.atlassian.net → the Task-317 seam (`Unimplemented`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use concerto_error::{Error, Result};
use concerto_persist::{
    NewPullRequest, Persistence, PullRequest, PullRequestId, Repository, RepositoryId, WorkareaId,
};
use url::Url;

use crate::dispatch::{issue_router_unimplemented, route_issue_host, IssueHost};
use crate::gh_cli;
use crate::github::GitHubProvider;
use crate::provider::{Issue, VcsProvider};

/// Config for the supervised actor's `run` loop. V0.1 has no knobs.
#[derive(Clone, Debug, Default)]
pub struct VcsConfig;

/// Cheap-cloneable, shareable handle to the VCS provider.
///
/// Cloning is `O(Arc)`; the persistence handle and the resolved `gh` path are
/// shared across clones. The `gh` path is resolved lazily on first use.
#[derive(Clone)]
pub struct VcsHandle {
    persistence: Arc<Persistence>,
    gh_path: Arc<tokio::sync::OnceCell<PathBuf>>,
}

impl VcsHandle {
    /// Build a fresh handle. The `gh` path is NOT resolved here.
    pub fn new(persistence: Arc<Persistence>) -> Self {
        Self {
            persistence,
            gh_path: Arc::new(tokio::sync::OnceCell::new()),
        }
    }

    /// Borrow the shared read-only pool (the gRPC handler's repo→workarea lookup).
    pub fn persistence_readers(&self) -> &sqlx::SqlitePool {
        self.persistence.readers()
    }

    /// Resolve (and cache) the `gh` binary path.
    pub async fn gh(&self) -> Result<&std::path::Path> {
        let path = self
            .gh_path
            .get_or_try_init(|| async { gh_cli::resolve_gh_path() })
            .await?;
        Ok(path.as_path())
    }

    /// Probe authentication once. Returns [`Error::VcsNotAuthenticated`] on
    /// failure.
    pub async fn check_auth(&self) -> Result<()> {
        let gh = self.gh().await?;
        gh_cli::check_auth(gh).await
    }

    /// List PRs in `repository_id`'s upstream repo via `gh pr list`.
    pub async fn list_prs(&self, repository_id: &RepositoryId) -> Result<Vec<gh_cli::PrSummary>> {
        let gh = self.gh().await?;
        let repo = self.resolve_repo_full_name(repository_id).await?;
        gh_cli::list_prs(gh, &repo).await
    }

    /// Look up one PR by number, upsert the cache row, return the projection.
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

    /// Create a PR for `head_ref` against `base_ref` and persist the cache row.
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

    /// Fetch a GitHub issue via `gh issue view` (Task-45 frozen signature,
    /// keyed by `RepositoryId` + number).
    pub async fn fetch_issue(
        &self,
        repository_id: &RepositoryId,
        issue_number: i64,
    ) -> Result<gh_cli::IssueDetail> {
        let gh = self.gh().await?;
        let repo = self.resolve_repo_full_name(repository_id).await?;
        gh_cli::view_issue(gh, &repo, issue_number).await
    }

    /// Top-level `fetch_issue(url)` **router** (Task 313, `design/13 §6.1` + §2
    /// row "313 fetch_issue routing"). Parses the URL host and dispatches:
    /// github.com → the GitHub issue fetch (via the octocrab `GitHubProvider`
    /// built from the supplied PAT); linear.app / *.atlassian.net → the Task-317
    /// seam (returns the typed `Unimplemented`).
    ///
    /// `github_token` is the PAT to authenticate the GitHub arm. The per-provider
    /// `fetch_issue(&Url)` stays on the `VcsProvider` trait; this is the
    /// top-level dispatch the gRPC/Maestro callers use with a raw URL string.
    pub async fn fetch_issue_url(
        &self,
        url: &str,
        github_token: Option<&str>,
    ) -> Result<Option<Issue>> {
        let parsed = Url::parse(url)
            .map_err(|e| Error::Validation(format!("invalid issue URL `{url}`: {e}")))?;
        match route_issue_host(&parsed)? {
            IssueHost::GitHub => {
                let token = github_token.ok_or_else(|| {
                    Error::VcsNotAuthenticated(
                        "GitHub issue fetch needs a token (SecretKind::GithubPat)".to_string(),
                    )
                })?;
                let provider = GitHubProvider::with_token(token)?;
                provider.fetch_issue(&parsed).await
            }
            host @ (IssueHost::Linear | IssueHost::Jira) => Err(issue_router_unimplemented(host)),
        }
    }

    // ---- internal helpers ----

    async fn load_repository(&self, id: &RepositoryId) -> Result<Repository> {
        concerto_persist::repositories::get(self.persistence.readers(), id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("repository {id} not found")))
    }

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

/// Extract `owner/repo` from a GitHub URL of any common shape
/// (`https://github.com/owner/repo.git`, `git@github.com:owner/repo`,
/// `https://github.com/owner/repo`). Moved verbatim from the V0.1 actor.
pub fn repo_full_name_from_url(url: &str) -> Option<String> {
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
