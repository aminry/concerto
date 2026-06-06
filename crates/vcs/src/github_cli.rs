//! [`GitHubProviderViaCli`] — the `gh` CLI fallback [`VcsProvider`] backend
//! (Task 313, `design/13 §3.1`).
//!
//! A thin **trait adapter** over the V0.1 [`crate::gh_cli`] shell-out (moved
//! verbatim from `crates/core/src/vcs/gh_cli.rs`). This is a *wrap, not a
//! rewrite*: the adapter maps `gh`'s `serde` projections (`PrDetail`/`CheckRun`/
//! `IssueDetail`) onto the trait value types, and preserves the V0.1 token
//! hygiene (never logs subprocess stdout/stderr — only command name + arg count,
//! enforced inside `gh_cli::run_gh`), the `which`-style `gh`/`gh.exe` resolution,
//! and the `--title-file`/`--body-file` temp-file path.
//!
//! The GraphQL methods + `revert_pr` are signature-frozen stubs here too — `gh`
//! could shell out for some of these later, but freezing the surface now keeps
//! the two providers interchangeable behind the trait.

use std::path::PathBuf;

use async_trait::async_trait;
use concerto_error::{Error, Result};
use url::Url;

use crate::gh_cli;
use crate::provider::{
    unimplemented_err, CheckRun, CreatePrRequest, Deployment, Issue, MergeMethod, MergeReport,
    ProviderPrId, PullRequest, RevertReport, ReviewThread, ThreadId, VcsProvider,
};

/// The `gh`-CLI-backed GitHub provider.
pub struct GitHubProviderViaCli {
    gh_path: PathBuf,
}

impl GitHubProviderViaCli {
    /// Resolve `gh` on `PATH` and build the provider. Returns
    /// [`Error::Internal`] when `gh` is not installed.
    pub fn resolve() -> Result<Self> {
        Ok(Self {
            gh_path: gh_cli::resolve_gh_path()?,
        })
    }

    /// Build from an already-resolved `gh` path (used by callers that cache the
    /// resolution, e.g. the actor handle).
    pub fn with_path(gh_path: PathBuf) -> Self {
        Self { gh_path }
    }

    /// Probe `gh auth status`. Surfaces [`Error::VcsNotAuthenticated`].
    pub async fn check_auth(&self) -> Result<()> {
        gh_cli::check_auth(&self.gh_path).await
    }
}

/// Parse `owner/repo` + issue number from a github.com issue URL (the CLI path's
/// `fetch_issue` arm).
fn parse_github_issue_url(url: &Url) -> Option<(String, i64)> {
    let segments: Vec<&str> = url.path().trim_matches('/').split('/').collect();
    if segments.len() >= 4 && segments[2] == "issues" {
        let number = segments[3].parse::<i64>().ok()?;
        return Some((format!("{}/{}", segments[0], segments[1]), number));
    }
    None
}

#[async_trait]
impl VcsProvider for GitHubProviderViaCli {
    async fn create_pr(&self, req: CreatePrRequest) -> Result<PullRequest> {
        let number = gh_cli::create_pr(
            &self.gh_path,
            &req.repo_full_name,
            &req.head,
            &req.base,
            &req.title,
            &req.body,
        )
        .await?;
        // Re-query to pick up the canonical body + head SHA the server stored.
        let detail = gh_cli::view_pr(&self.gh_path, &req.repo_full_name, number).await?;
        Ok(pr_detail_to_pull_request(&req.repo_full_name, detail))
    }

    async fn get_pr(&self, id: ProviderPrId) -> Result<PullRequest> {
        let detail = gh_cli::view_pr(&self.gh_path, &id.repo_full_name, id.number).await?;
        Ok(pr_detail_to_pull_request(&id.repo_full_name, detail))
    }

    async fn list_check_runs(&self, repo: &str, sha: &str) -> Result<Vec<CheckRun>> {
        let runs = gh_cli::get_check_runs(&self.gh_path, repo, sha).await?;
        Ok(runs.into_iter().map(check_run_from_cli).collect())
    }

    async fn merge_pr(&self, id: ProviderPrId, method: MergeMethod) -> Result<MergeReport> {
        gh_cli::merge_pr(
            &self.gh_path,
            &id.repo_full_name,
            id.number,
            method.as_str(),
        )
        .await?;
        // `gh pr merge` returns no body; report success without a commit SHA.
        Ok(MergeReport {
            merged: true,
            merge_commit_sha: None,
            message: "merged via gh CLI".to_string(),
        })
    }

    async fn revert_pr(&self, _id: ProviderPrId) -> Result<RevertReport> {
        Err(unimplemented_err(
            "GitHubProviderViaCli::revert_pr (filled by Task 320)",
        ))
    }

    async fn list_review_threads(&self, _id: ProviderPrId) -> Result<Vec<ReviewThread>> {
        Err(unimplemented_err(
            "GitHubProviderViaCli::list_review_threads (filled by Task 316)",
        ))
    }

    async fn resolve_thread(&self, _id: ThreadId) -> Result<()> {
        Err(unimplemented_err(
            "GitHubProviderViaCli::resolve_thread (filled by Task 316)",
        ))
    }

    async fn list_deployments(&self, _repo: &str, _ref_: &str) -> Result<Vec<Deployment>> {
        Err(unimplemented_err(
            "GitHubProviderViaCli::list_deployments (filled by Task 316)",
        ))
    }

    async fn fetch_issue(&self, url: &Url) -> Result<Option<Issue>> {
        let (repo, number) = parse_github_issue_url(url)
            .ok_or_else(|| Error::Validation(format!("not a GitHub issue URL: {url}")))?;
        let detail = gh_cli::view_issue(&self.gh_path, &repo, number).await?;
        Ok(Some(issue_detail_to_issue(detail)))
    }
}

fn pr_detail_to_pull_request(repo_full_name: &str, d: gh_cli::PrDetail) -> PullRequest {
    PullRequest {
        id: ProviderPrId::new(repo_full_name, d.number),
        title: d.title,
        body: d.body,
        state: d.state.to_lowercase(),
        url: d.url,
        base_ref: d.base_ref_name,
        head_ref: d.head_ref_name,
        head_sha: d.head_ref_oid,
    }
}

fn check_run_from_cli(r: gh_cli::CheckRun) -> CheckRun {
    CheckRun {
        name: r.name,
        status: r.status,
        conclusion: r.conclusion,
        details_url: r.details_url,
    }
}

fn issue_detail_to_issue(d: gh_cli::IssueDetail) -> Issue {
    Issue {
        number: d.number,
        title: d.title,
        body: d.body,
        state: d.state.to_lowercase(),
        url: d.url,
        labels: d.labels.into_iter().map(|l| l.name).collect(),
    }
}
