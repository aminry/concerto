//! The FROZEN [`VcsProvider`] trait + its value types (Task 313).
//!
//! Transcribed faithfully from `design/13 §3.8` — this is one of the
//! extension-point trait seams locked in `design/18 §3.7`, and a **V2.0
//! stability contract**: future providers (GitLab, Bitbucket, Gerrit, GitHub
//! Enterprise variants) plug in behind exactly these nine methods. Getting the
//! method set + value-type field sets right NOW matters more than the impl
//! bodies (several method bodies are Task 316's to fill). **Do not redesign
//! this surface** — a breaking change is a "Revise" task per `README.md §9`.
//!
//! Where `design/13 §3.8` leaves a value type's fields implicit, they are
//! designed minimally + append-friendly here and FROZEN. `Issue` mirrors the
//! existing `vcs.proto` `Issue` shape (`number/title/body/state/url/labels`).
//!
//! ## The `Unimplemented` stub convention
//!
//! Three trait methods are **signature-frozen stubs** on `GitHubProvider`:
//! `list_review_threads` / `resolve_thread` (GraphQL — Task 316) and the
//! Linear/Jira arm of the `fetch_issue` router (Task 317). Per the task's
//! no-`unimplemented!()`-macro rule, a stub returns the typed
//! [`unimplemented_err`] (an `Error::Vcs` carrying the stable `"unimplemented:"`
//! prefix) rather than panicking. `concerto-error` is a prior-task FROZEN crate
//! (out of this task's Outputs) with no `Unimplemented` variant, so we reuse the
//! existing typed `Error::Vcs` variant with a recognizable prefix — recorded in
//! the Handoff.

use async_trait::async_trait;
use concerto_error::{Error, Result};
use url::Url;

/// Build the typed "not implemented yet" error a signature-frozen stub returns.
///
/// Uses the existing `Error::Vcs` variant (the `concerto-error` enum is FROZEN
/// and out of this task's Outputs — we do not add an `Unimplemented` variant)
/// with a stable `"unimplemented: "` prefix so callers + tests can recognize a
/// stub vs a real VCS failure. `what` names the method + the task that fills it.
pub fn unimplemented_err(what: &str) -> Error {
    Error::Vcs(format!("unimplemented: {what}"))
}

/// True when `e` is a signature-frozen-stub error from [`unimplemented_err`].
/// Lets tests assert "the seam returns Unimplemented" without matching on the
/// message text by hand.
pub fn is_unimplemented(e: &Error) -> bool {
    matches!(e, Error::Vcs(m) if m.starts_with("unimplemented:"))
}

/// How to merge a PR (`design/13 §3.8`). Maps onto GitHub's `merge|squash|rebase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    /// The GitHub API / `gh` flag spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            MergeMethod::Merge => "merge",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        }
    }

    /// Parse from the free-form method string the gRPC surface carries
    /// (`""`/`"merge"` → Merge). Returns a `Validation` error otherwise.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "" | "merge" => Ok(MergeMethod::Merge),
            "squash" => Ok(MergeMethod::Squash),
            "rebase" => Ok(MergeMethod::Rebase),
            other => Err(Error::Validation(format!(
                "merge method must be merge|squash|rebase (got `{other}`)"
            ))),
        }
    }
}

/// A provider-side PR identifier (`design/13 §3.8` `ProviderPrId`).
///
/// Newtype over the `(repo_full_name, number)` pair the REST API keys on, plus
/// an optional GraphQL `node_id` (Task 319 persists it; 316's GraphQL
/// resolve/thread calls need it). FROZEN field set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderPrId {
    /// `owner/repo`.
    pub repo_full_name: String,
    /// The PR number within the repo.
    pub number: i64,
    /// The GraphQL global node id, when known (Task 316/319). `None` for the
    /// REST-only paths.
    pub node_id: Option<String>,
}

impl ProviderPrId {
    /// Construct a REST-only id (no GraphQL node id yet).
    pub fn new(repo_full_name: impl Into<String>, number: i64) -> Self {
        Self {
            repo_full_name: repo_full_name.into(),
            number,
            node_id: None,
        }
    }
}

/// A review-thread identifier (`design/13 §3.8` `ThreadId`). The GraphQL node
/// id of the thread; FROZEN as an opaque string (Task 316 fills resolve()).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadId(pub String);

/// What `create_pr` is told to create (`design/13 §3.4`/§3.8). The title/body
/// are taken AS GIVEN (deterministic) — LLM composition is Task 321's, not here.
#[derive(Debug, Clone)]
pub struct CreatePrRequest {
    /// `owner/repo`.
    pub repo_full_name: String,
    /// Source branch (head).
    pub head: String,
    /// Target branch (base); empty → the provider's default branch.
    pub base: String,
    pub title: String,
    pub body: String,
    /// Open as a draft PR.
    pub draft: bool,
}

/// A pull request as the trait reports it (`design/13 §3.8`). Minimal,
/// append-friendly projection; FROZEN field set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub id: ProviderPrId,
    pub title: String,
    pub body: String,
    /// `open|closed|merged|draft` (lowercased).
    pub state: String,
    pub url: String,
    pub base_ref: String,
    pub head_ref: String,
    /// Tip commit SHA of the head branch.
    pub head_sha: String,
}

/// A CI check run (`design/13 §3.8`). Normalizes GitHub's `CheckRun` (workflow)
/// and legacy `StatusContext` into one shape (mirrors the V0.1 `gh_cli::CheckRun`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRun {
    pub name: String,
    /// `queued|in_progress|completed` (CheckRun) or `pending|success|failure|error`
    /// (StatusContext), copied verbatim.
    pub status: String,
    /// Terminal conclusion (`success|failure|neutral|cancelled|…`), empty until set.
    pub conclusion: String,
    pub details_url: String,
}

/// Outcome of a merge (`design/13 §3.8` `MergeReport`). FROZEN minimal shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeReport {
    pub merged: bool,
    /// The merge commit SHA, when the provider returns one.
    pub merge_commit_sha: Option<String>,
    /// Human-facing message (e.g. "Pull Request successfully merged").
    pub message: String,
}

/// Outcome of a revert (`design/13 §3.8`/§3.5 `RevertReport`). The revert is a
/// revert-commit-by-default (R-5); the URL of the revert PR, when one is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertReport {
    pub reverted: bool,
    /// URL of the revert PR (revert-commit strategy), when one was created.
    pub revert_pr_url: Option<String>,
    pub message: String,
}

/// A review thread (`design/13 §3.6`/§3.8 `ReviewThread`). GraphQL-sourced;
/// Task 316 populates `comments`. FROZEN minimal shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewThread {
    pub id: ThreadId,
    pub resolved: bool,
    /// File path the thread is anchored to (`None` for PR-level threads).
    pub path: Option<String>,
    /// The thread's comment bodies, oldest first (filled by Task 316).
    pub comments: Vec<String>,
}

/// A deployment (`design/13 §3.8`/§3.1 Deployments API `Deployment`). FROZEN
/// minimal shape; Task 316 aggregates statuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deployment {
    /// Provider deployment id.
    pub id: String,
    /// Target environment (`production|staging|…`).
    pub environment: String,
    /// Latest status (`success|failure|in_progress|queued|…`), empty if none.
    pub state: String,
    /// The ref/SHA this deployment targets.
    pub ref_: String,
}

/// An issue (`design/13 §3.7`/§3.8 `Issue`). Mirrors the FROZEN `vcs.proto`
/// `Issue` shape so the gRPC mapping is 1:1.
///
/// `number` is the **GitHub-only** integer id; Linear/Jira issues set it to
/// `0` and carry their provider-native string id in [`Issue::external_id`]
/// (`ENG-123` / `PROJ-45`) — the `string external_id = 7` field Task 317 added
/// to the proto.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Issue {
    /// GitHub integer id (`#<n>`); `0` for Linear/Jira (see `external_id`).
    pub number: i64,
    pub title: String,
    pub body: String,
    /// `open|closed` (lowercased) for GitHub; the tracker's status name
    /// (lowercased) for Linear/Jira.
    pub state: String,
    pub url: String,
    pub labels: Vec<String>,
    /// The provider-native string id (`ENG-123`/`PROJ-45`). Empty for GitHub.
    pub external_id: String,
}

/// The FROZEN VCS provider abstraction (`design/13 §3.8`).
///
/// One of the `design/18 §3.7` extension-point trait seams. The MIT Core ships
/// `GitHubProvider` (octocrab) + `GitHubProviderViaCli` (gh fallback); V2.0
/// adds GitLab/Bitbucket as additional impls behind this exact surface. The
/// nine methods + their value types are a V2.0 stability contract — **do not
/// change this surface**; extend via a "Revise" task only (`README.md §9`).
///
/// `Result` is `concerto_error::Result`. The GraphQL methods
/// (`list_review_threads`/`resolve_thread`) are implemented-stubs on
/// `GitHubProvider` (Task 316 fills them); their signatures are frozen now.
#[async_trait]
pub trait VcsProvider: Send + Sync + 'static {
    async fn create_pr(&self, req: CreatePrRequest) -> Result<PullRequest>;
    async fn get_pr(&self, id: ProviderPrId) -> Result<PullRequest>;
    async fn list_check_runs(&self, repo: &str, sha: &str) -> Result<Vec<CheckRun>>;
    async fn merge_pr(&self, id: ProviderPrId, method: MergeMethod) -> Result<MergeReport>;
    async fn revert_pr(&self, id: ProviderPrId) -> Result<RevertReport>;
    async fn list_review_threads(&self, id: ProviderPrId) -> Result<Vec<ReviewThread>>;
    async fn resolve_thread(&self, id: ThreadId) -> Result<()>;
    async fn list_deployments(&self, repo: &str, ref_: &str) -> Result<Vec<Deployment>>;
    async fn fetch_issue(&self, url: &Url) -> Result<Option<Issue>>;
}
