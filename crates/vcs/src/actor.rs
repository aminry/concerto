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

use concerto_keychain::SecretValue;

use crate::dispatch::{external_tracker_blocked, route_issue_host, IssueCache, IssueHost};
use crate::gh_cli;
use crate::github::GitHubProvider;
use crate::jira::{JiraClient, RefreshToken};
use crate::linear::LinearClient;
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
    /// Shared 1 h-TTL issue cache (`design/13 §3.7`/§4). Issue bodies live ONLY
    /// here — never in SQLite. Cloned with the handle so every clone shares one
    /// cache (Task 317).
    issue_cache: IssueCache,
}

impl VcsHandle {
    /// Build a fresh handle. The `gh` path is NOT resolved here.
    pub fn new(persistence: Arc<Persistence>) -> Self {
        Self {
            persistence,
            gh_path: Arc::new(tokio::sync::OnceCell::new()),
            issue_cache: IssueCache::system(),
        }
    }

    /// Build a handle with a caller-supplied issue cache (the `testkit`
    /// synthetic-clock cache in tests). Production uses [`VcsHandle::new`].
    pub fn with_issue_cache(persistence: Arc<Persistence>, issue_cache: IssueCache) -> Self {
        Self {
            persistence,
            gh_path: Arc::new(tokio::sync::OnceCell::new()),
            issue_cache,
        }
    }

    /// Borrow the shared read-only pool (the gRPC handler's repo→workarea lookup).
    pub fn persistence_readers(&self) -> &sqlx::SqlitePool {
        self.persistence.readers()
    }

    /// Exclusive write access to the SQLite writer (the `SetVcsCredential`
    /// handler upserts the non-secret `vcs_credentials` metadata row, Task 317).
    pub async fn persistence_writer(&self) -> concerto_persist::WriterGuard<'_> {
        self.persistence.writer().await
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

    /// Top-level `fetch_issue(url)` **router** (`design/13 §6.1` + §3.7). Parses
    /// the URL host and dispatches: `github.com`/Enterprise → the GitHub issue
    /// fetch (octocrab, Task 313); `linear.app` → the Linear GraphQL client;
    /// `*.atlassian.net` → the Jira REST client (Task 317). The result is cached
    /// for 1 h in memory (keyed by canonicalized URL); a still-fresh hit skips
    /// the HTTP call entirely. Issue bodies are NEVER persisted to SQLite.
    ///
    /// `creds` carries the per-arm credentials (read from the keychain by the
    /// caller) + the `enterprise_data_privacy` flag: a privacy-locked project
    /// refuses an external-tracker (Linear/Jira) fetch with the typed
    /// [`external_tracker_blocked`] error BEFORE any outbound call (the GitHub
    /// arm is the user's own repo host, not an external tracker, so it is not
    /// gated here).
    pub async fn fetch_issue_url(
        &self,
        url: &str,
        creds: &IssueFetchCreds<'_>,
    ) -> Result<Option<Issue>> {
        // 1 h-TTL cache hit → no HTTP call (privacy + latency).
        if let Some(cached) = self.issue_cache.get(url) {
            return Ok(Some(cached));
        }

        let parsed = Url::parse(url)
            .map_err(|e| Error::Validation(format!("invalid issue URL `{url}`: {e}")))?;
        let issue = match route_issue_host(&parsed)? {
            IssueHost::GitHub => {
                let token = creds.github_token.ok_or_else(|| {
                    Error::VcsNotAuthenticated(
                        "GitHub issue fetch needs a token (SecretKind::GithubPat)".to_string(),
                    )
                })?;
                let provider = GitHubProvider::with_token(token)?;
                provider.fetch_issue(&parsed).await?
            }
            IssueHost::Linear => {
                if creds.enterprise_data_privacy {
                    return Err(external_tracker_blocked("linear"));
                }
                let token = creds.linear_token.ok_or_else(|| {
                    Error::VcsNotAuthenticated(
                        "Linear issue fetch needs a token (VcsSecretSlot::LinearAccessToken); \
                         connect Linear in Settings"
                            .to_string(),
                    )
                })?;
                let client = match creds.linear_base {
                    Some(base) => LinearClient::with_base(base)?,
                    None => LinearClient::new()?,
                };
                Some(client.fetch(url, token).await?)
            }
            IssueHost::Jira => {
                if creds.enterprise_data_privacy {
                    return Err(external_tracker_blocked("jira"));
                }
                let token = creds.jira_token.ok_or_else(|| {
                    Error::VcsNotAuthenticated(
                        "Jira issue fetch needs a token (VcsSecretSlot::JiraAccessToken); \
                         connect Jira in Settings"
                            .to_string(),
                    )
                })?;
                // Jira's REST base is the Atlassian site host of the URL itself
                // (`https://<site>.atlassian.net`), unless the caller overrides
                // it (the testkit wiremock base).
                let base = match creds.jira_base {
                    Some(base) => base.to_string(),
                    None => jira_base_from_url(&parsed)?,
                };
                let client = JiraClient::with_base(&base)?;
                Some(client.fetch(url, token, creds.jira_refresh).await?)
            }
        };

        // Cache successful fetches (a `None`/absent issue is not cached).
        if let Some(ref issue) = issue {
            self.issue_cache.put(url, issue.clone());
        }
        Ok(issue)
    }

    /// Direct Linear issue fetch by id (`ENG-123`) or URL (`design/13 §5.1`
    /// `fetch_linear_issue`). Skips the host-routing step; still consults the 1 h
    /// cache + the `enterprise_data_privacy` gate. `linear_base` overrides the
    /// production endpoint (the testkit wiremock base) when `Some`.
    pub async fn fetch_linear_issue(
        &self,
        id_or_url: &str,
        token: &SecretValue,
        enterprise_data_privacy: bool,
        linear_base: Option<&str>,
    ) -> Result<Issue> {
        if enterprise_data_privacy {
            return Err(external_tracker_blocked("linear"));
        }
        if let Some(cached) = self.issue_cache.get(id_or_url) {
            return Ok(cached);
        }
        let client = match linear_base {
            Some(base) => LinearClient::with_base(base)?,
            None => LinearClient::new()?,
        };
        let issue = client.fetch(id_or_url, token).await?;
        self.issue_cache.put(id_or_url, issue.clone());
        Ok(issue)
    }

    /// Direct Jira issue fetch by key (`PROJ-45`) or URL (`design/13 §5.1`). The
    /// caller supplies the Atlassian site base + token + optional one-shot
    /// refresh. Consults the 1 h cache + the privacy gate.
    pub async fn fetch_jira_issue(
        &self,
        key_or_url: &str,
        base_uri: &str,
        token: &SecretValue,
        refresh: Option<&RefreshToken<'_>>,
        enterprise_data_privacy: bool,
    ) -> Result<Issue> {
        if enterprise_data_privacy {
            return Err(external_tracker_blocked("jira"));
        }
        if let Some(cached) = self.issue_cache.get(key_or_url) {
            return Ok(cached);
        }
        let client = JiraClient::with_base(base_uri)?;
        let issue = client.fetch(key_or_url, token, refresh).await?;
        self.issue_cache.put(key_or_url, issue.clone());
        Ok(issue)
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

/// Per-arm credentials + privacy flag for [`VcsHandle::fetch_issue_url`]
/// (Task 317). The caller (the gRPC handler) reads each token from the keychain
/// (GitHub PAT via `SecretKind::GithubPat`; Linear/Jira via the
/// `VcsSecretSlot::{Linear,Jira}AccessToken` slots) and resolves the project's
/// `enterprise_data_privacy` setting before constructing this. All fields are
/// borrowed so no secret is copied into the carrier.
///
/// Only the arm matching the URL host is consulted, so an unused arm may be
/// `None`. `linear_base`/`jira_base` override the production API base (the
/// `testkit` wiremock base in tests; `None` → the real endpoint / the URL's
/// Atlassian site for Jira).
#[derive(Default)]
pub struct IssueFetchCreds<'a> {
    /// GitHub PAT (the GitHub arm; not gated by `enterprise_data_privacy` —
    /// it is the user's own repo host, not an external tracker).
    pub github_token: Option<&'a str>,
    /// Linear OAuth access token or personal API key.
    pub linear_token: Option<&'a SecretValue>,
    /// Jira (Atlassian) OAuth access token.
    pub jira_token: Option<&'a SecretValue>,
    /// One-shot Jira OAuth refresh callback (invoked once on a 401).
    pub jira_refresh: Option<&'a RefreshToken<'a>>,
    /// Override the Linear API base (testkit). `None` → production.
    pub linear_base: Option<&'a str>,
    /// Override the Jira API base (testkit). `None` → the URL's Atlassian site.
    pub jira_base: Option<&'a str>,
    /// When `true`, refuse Linear/Jira (external-tracker) fetches with the typed
    /// `vcs.external_tracker_blocked` error (the `design/13 §3.7` privacy floor).
    pub enterprise_data_privacy: bool,
}

/// Derive the Jira REST base (`https://<site>.atlassian.net`) from a Jira issue
/// URL, dropping the path. Used when the caller does not override the base.
fn jira_base_from_url(url: &Url) -> Result<String> {
    let host = url
        .host_str()
        .ok_or_else(|| Error::Validation(format!("jira: URL has no host: {url}")))?;
    let scheme = url.scheme();
    Ok(format!("{scheme}://{host}"))
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
