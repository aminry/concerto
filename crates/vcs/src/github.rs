//! [`GitHubProvider`] — the default [`VcsProvider`] backend on `octocrab`
//! (Task 313, `design/13 §3.1`).
//!
//! Uses `octocrab`'s typed `get`/`post`/`put` request helpers against the
//! configured base URI (default `https://api.github.com`; configurable for
//! GitHub Enterprise per R-10), deserializing into local `serde` projections
//! rather than coupling to octocrab's evolving model structs. TLS is rustls
//! (workspace pin posture — no openssl/native-tls).
//!
//! The PAT is read from the keychain (`SecretKind::GithubPat`, existing) — this
//! task ships PAT-only auth; the GitHub App option (`VcsSecretSlot::
//! GithubAppPrivateKey` + dual rate-limit pools) is Task 314.
//!
//! The GraphQL methods (`list_review_threads`/`resolve_thread`) are
//! signature-frozen stubs returning [`unimplemented_err`] — Task 316 fills them.
//! `revert_pr` is likewise a frozen stub (the revert-commit-by-default mechanics
//! land with the coordinated-merge loop, Task 320).

use async_trait::async_trait;
use concerto_error::{Error, Result};
use serde::Deserialize;
use url::Url;

use crate::provider::{
    unimplemented_err, CheckRun, CreatePrRequest, Deployment, Issue, MergeMethod, MergeReport,
    ProviderPrId, PullRequest, RevertReport, ReviewThread, ThreadId, VcsProvider,
};

/// Default GitHub REST base. GitHub Enterprise overrides this with
/// `https://<host>/api/v3` (R-10).
pub const DEFAULT_GITHUB_BASE_URI: &str = "https://api.github.com";

/// The octocrab-backed GitHub provider.
pub struct GitHubProvider {
    client: octocrab::Octocrab,
}

impl GitHubProvider {
    /// Build a provider from a PAT against the default `api.github.com` base.
    pub fn with_token(token: impl Into<String>) -> Result<Self> {
        Self::with_token_and_base(token, DEFAULT_GITHUB_BASE_URI)
    }

    /// Build a provider from a PAT against a caller-supplied base URI
    /// (GitHub Enterprise, R-10, or the `testkit` wiremock base).
    pub fn with_token_and_base(token: impl Into<String>, base_uri: &str) -> Result<Self> {
        ensure_crypto_provider();
        let client = octocrab::Octocrab::builder()
            .personal_token(token.into())
            .base_uri(base_uri)
            .map_err(map_octo_err)?
            .build()
            .map_err(map_octo_err)?;
        Ok(Self { client })
    }

    /// Wrap a pre-built octocrab client (used by the `testkit` harness, which
    /// points the client at a `wiremock::MockServer` base URL).
    pub fn from_client(client: octocrab::Octocrab) -> Self {
        Self { client }
    }
}

/// Install the process-level **ring** rustls `CryptoProvider` if none is set.
///
/// The workspace links both rustls crypto backends (iroh's hickory tree pulls
/// `aws-lc-rs`; octocrab's `rustls-ring` pulls `ring`), so rustls cannot
/// auto-select a default and `hyper-rustls` panics on the first TLS handshake
/// under `cargo test --workspace` feature unification. Installing `ring`
/// explicitly (idempotent — `install_default` returns `Err` if one is already
/// set, which we ignore) mirrors the relay crate's pattern and keeps the
/// no-openssl / pure-Rust / Windows-lane posture (`ring`, not `aws-lc-rs`).
fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn map_octo_err(e: octocrab::Error) -> Error {
    Error::Vcs(format!("github: {e}"))
}

// --- REST response projections (local; decoupled from octocrab's models) ---

#[derive(Debug, Deserialize)]
struct GhRef {
    #[serde(rename = "ref")]
    ref_name: String,
    sha: String,
}

#[derive(Debug, Deserialize)]
struct GhPull {
    number: i64,
    node_id: Option<String>,
    title: String,
    #[serde(default)]
    body: Option<String>,
    state: String,
    #[serde(default)]
    draft: bool,
    html_url: String,
    head: GhRef,
    base: GhRef,
    #[serde(default)]
    merged: bool,
}

impl GhPull {
    fn into_pull_request(self, repo_full_name: &str) -> PullRequest {
        // GitHub reports `state` as open/closed; a closed+merged PR is "merged",
        // an open draft is "draft" — normalize to the trait's vocabulary.
        let state = if self.merged {
            "merged".to_string()
        } else if self.state == "open" && self.draft {
            "draft".to_string()
        } else {
            self.state.to_lowercase()
        };
        PullRequest {
            id: ProviderPrId {
                repo_full_name: repo_full_name.to_string(),
                number: self.number,
                node_id: self.node_id,
            },
            title: self.title,
            body: self.body.unwrap_or_default(),
            state,
            url: self.html_url,
            base_ref: self.base.ref_name,
            head_ref: self.head.ref_name,
            head_sha: self.head.sha,
        }
    }
}

#[derive(Debug, Deserialize)]
struct GhCheckRun {
    name: String,
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    details_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhCheckRunsResponse {
    #[serde(default)]
    check_runs: Vec<GhCheckRun>,
}

#[derive(Debug, Deserialize)]
struct GhMergeResult {
    #[serde(default)]
    merged: bool,
    #[serde(default)]
    sha: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhDeployment {
    id: u64,
    #[serde(default)]
    environment: String,
    #[serde(default, rename = "ref")]
    ref_: String,
}

#[derive(Debug, Deserialize)]
struct GhLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GhIssue {
    number: i64,
    title: String,
    #[serde(default)]
    body: Option<String>,
    state: String,
    html_url: String,
    #[serde(default)]
    labels: Vec<GhLabel>,
    /// Present on PR objects; lets us reject PR URLs routed to issue fetch.
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
}

/// Parse `owner/repo` + the trailing `issues/<n>` number from a GitHub issue
/// URL. Returns `None` for non-issue paths.
fn parse_github_issue_url(url: &Url) -> Option<(String, i64)> {
    // Path: /owner/repo/issues/<number>  (also /pull/<n> which we reject).
    let segments: Vec<&str> = url.path().trim_matches('/').split('/').collect();
    if segments.len() >= 4 && segments[2] == "issues" {
        let number = segments[3].parse::<i64>().ok()?;
        return Some((format!("{}/{}", segments[0], segments[1]), number));
    }
    None
}

#[async_trait]
impl VcsProvider for GitHubProvider {
    async fn create_pr(&self, req: CreatePrRequest) -> Result<PullRequest> {
        let route = format!("/repos/{}/pulls", req.repo_full_name);
        let body = serde_json::json!({
            "title": req.title,
            "head": req.head,
            "base": req.base,
            "body": req.body,
            "draft": req.draft,
        });
        let pull: GhPull = self
            .client
            .post(route, Some(&body))
            .await
            .map_err(map_octo_err)?;
        Ok(pull.into_pull_request(&req.repo_full_name))
    }

    async fn get_pr(&self, id: ProviderPrId) -> Result<PullRequest> {
        let route = format!("/repos/{}/pulls/{}", id.repo_full_name, id.number);
        let pull: GhPull = self
            .client
            .get(route, None::<&()>)
            .await
            .map_err(map_octo_err)?;
        Ok(pull.into_pull_request(&id.repo_full_name))
    }

    async fn list_check_runs(&self, repo: &str, sha: &str) -> Result<Vec<CheckRun>> {
        let route = format!("/repos/{repo}/commits/{sha}/check-runs");
        let resp: GhCheckRunsResponse = self
            .client
            .get(route, None::<&()>)
            .await
            .map_err(map_octo_err)?;
        Ok(resp
            .check_runs
            .into_iter()
            .map(|r| CheckRun {
                name: r.name,
                status: r.status,
                conclusion: r.conclusion.unwrap_or_default(),
                details_url: r.details_url.unwrap_or_default(),
            })
            .collect())
    }

    async fn merge_pr(&self, id: ProviderPrId, method: MergeMethod) -> Result<MergeReport> {
        let route = format!("/repos/{}/pulls/{}/merge", id.repo_full_name, id.number);
        let body = serde_json::json!({ "merge_method": method.as_str() });
        let result: GhMergeResult = self
            .client
            .put(route, Some(&body))
            .await
            .map_err(map_octo_err)?;
        Ok(MergeReport {
            merged: result.merged,
            merge_commit_sha: result.sha,
            message: result.message.unwrap_or_default(),
        })
    }

    async fn revert_pr(&self, _id: ProviderPrId) -> Result<RevertReport> {
        // Signature frozen; the revert-commit-by-default mechanics (R-5) land
        // with the coordinated-revert loop (Task 320).
        Err(unimplemented_err(
            "GitHubProvider::revert_pr (filled by Task 320)",
        ))
    }

    async fn list_review_threads(&self, _id: ProviderPrId) -> Result<Vec<ReviewThread>> {
        // GraphQL — signature frozen; Task 316 supplies the query.
        Err(unimplemented_err(
            "GitHubProvider::list_review_threads (GraphQL; filled by Task 316)",
        ))
    }

    async fn resolve_thread(&self, _id: ThreadId) -> Result<()> {
        Err(unimplemented_err(
            "GitHubProvider::resolve_thread (GraphQL; filled by Task 316)",
        ))
    }

    async fn list_deployments(&self, repo: &str, ref_: &str) -> Result<Vec<Deployment>> {
        let route = format!("/repos/{repo}/deployments");
        let params = [("ref", ref_)];
        let deployments: Vec<GhDeployment> = self
            .client
            .get(route, Some(&params))
            .await
            .map_err(map_octo_err)?;
        // The latest deployment status requires a second call per deployment
        // (Task 316 aggregates them); this task lists deployments with an empty
        // `state` placeholder. Signature + listing frozen now.
        Ok(deployments
            .into_iter()
            .map(|d| Deployment {
                id: d.id.to_string(),
                environment: d.environment,
                state: String::new(),
                ref_: d.ref_,
            })
            .collect())
    }

    async fn fetch_issue(&self, url: &Url) -> Result<Option<Issue>> {
        let (repo, number) = match parse_github_issue_url(url) {
            Some(parsed) => parsed,
            None => return Err(Error::Validation(format!("not a GitHub issue URL: {url}"))),
        };
        let route = format!("/repos/{repo}/issues/{number}");
        let issue: GhIssue = self
            .client
            .get(route, None::<&()>)
            .await
            .map_err(map_octo_err)?;
        if issue.pull_request.is_some() {
            return Err(Error::Validation(format!(
                "URL {url} is a pull request, not an issue"
            )));
        }
        Ok(Some(Issue {
            number: issue.number,
            title: issue.title,
            body: issue.body.unwrap_or_default(),
            state: issue.state.to_lowercase(),
            url: issue.html_url,
            labels: issue.labels.into_iter().map(|l| l.name).collect(),
            // GitHub issues key on `number`; the provider-native string id is
            // a Linear/Jira concept (Task 317).
            external_id: String::new(),
        }))
    }
}
