//! Per-call backend dispatch + the `fetch_issue` URL router + the `VcsState`
//! in-memory cache skeleton (Task 313, `design/13 §6.1`/§4).
//!
//! - [`choose_backend`] implements the `design/13 §6.1` pseudocode: per-call,
//!   never user-chosen — `fetch_issue` routes to the host router; else a
//!   configured octocrab token → `GitHubProvider`; else `gh` available →
//!   `GitHubProviderViaCli`; else [`Error::Vcs`] `NoVcsCredentials`.
//! - [`route_issue_host`] classifies an issue URL by host (GitHub vs
//!   Linear/Jira). The GitHub arm is live (Task 313); Linear/Jira are a routing
//!   seam returning [`unimplemented_err`] until Task 317 supplies the clients.
//! - [`VcsState`] / [`ProviderKey`] are the `design/13 §4` cache skeleton —
//!   this task populates only `providers`; `rate_limits` is Task 314's,
//!   `webhook_secrets`/`threads_cache` are 315/316's. The `ProviderKey` shape is
//!   FROZEN here (314 keys its three rate-limit pools off it).

use std::collections::HashMap;
use std::sync::Arc;

use concerto_error::{Error, Result};
use url::Url;

use crate::provider::{unimplemented_err, CheckRun, ReviewThread, VcsProvider};

/// Which backend [`choose_backend`] selected for an operation.
///
/// The dispatcher returns a *decision*; the caller (the actor/handle) holds the
/// constructed providers. This keeps `choose_backend` pure + table-testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// The default octocrab `GitHubProvider` (a PAT/App token is configured).
    Octocrab,
    /// The `gh` CLI fallback `GitHubProviderViaCli`.
    GhCli,
    /// The issue-host router owns this op (`fetch_issue`).
    IssueRouter,
}

/// The operation being dispatched. `design/13 §6.1` branches `fetch_issue`
/// before the token/cli check; every other op goes through the token→cli ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcsOp {
    FetchIssue,
    /// Any PR/check/deploy/merge op (the token→cli ladder applies).
    PrOp,
}

/// What a repo offers for backend selection. The dispatcher is given booleans
/// (computed by the caller from the keychain / `PATH`) so it stays pure.
#[derive(Debug, Clone, Copy)]
pub struct RepoCapabilities {
    /// A PAT or App token is configured for this repo (→ octocrab).
    pub has_octocrab_token: bool,
    /// `gh` is installed + authenticated (→ gh CLI).
    pub gh_available: bool,
}

/// Per-call backend choice (`design/13 §6.1`, FROZEN dispatch order). Per-call,
/// never user-chosen.
///
/// ```text
/// choose_backend(repo, op):
///     if op == fetch_issue: use the URL-host router (Linear/Jira client)
///     elif repo.has_octocrab_token: use Octocrab
///     elif gh_available():           use GhCli
///     else: Error::NoVcsCredentials
/// ```
pub fn choose_backend(caps: RepoCapabilities, op: VcsOp) -> Result<Backend> {
    if op == VcsOp::FetchIssue {
        return Ok(Backend::IssueRouter);
    }
    if caps.has_octocrab_token {
        Ok(Backend::Octocrab)
    } else if caps.gh_available {
        Ok(Backend::GhCli)
    } else {
        Err(no_vcs_credentials())
    }
}

/// The typed `NoVcsCredentials` error (`design/13 §6.1`/§8). Reuses the existing
/// `Error::Vcs` variant (the `concerto-error` enum is FROZEN/out-of-scope) with a
/// stable `no_vcs_credentials` prefix the UI can switch on to launch the
/// credential-setup wizard.
pub fn no_vcs_credentials() -> Error {
    Error::Vcs(
        "no_vcs_credentials: configure a GitHub token or install + authenticate `gh`".to_string(),
    )
}

/// True when `e` is the [`no_vcs_credentials`] decision error.
pub fn is_no_vcs_credentials(e: &Error) -> bool {
    matches!(e, Error::Vcs(m) if m.starts_with("no_vcs_credentials"))
}

/// The provider an issue URL routes to (`design/13 §6.1` `fetch_issue` arm).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueHost {
    /// `github.com` or a GitHub Enterprise host → the GitHub `fetch_issue`.
    GitHub,
    /// `linear.app` → Task 317's Linear client (seam).
    Linear,
    /// `*.atlassian.net` → Task 317's Jira client (seam).
    Jira,
}

/// Classify an issue URL by host. The Linear/Jira arms are recognized here so
/// the router can return a typed `Unimplemented` seam (Task 317 fills the
/// clients); an unrecognized host is a `Validation` error.
pub fn route_issue_host(url: &Url) -> Result<IssueHost> {
    let host = url
        .host_str()
        .ok_or_else(|| Error::Validation(format!("issue URL has no host: {url}")))?
        .to_ascii_lowercase();
    if host == "github.com" || host == "www.github.com" || host.ends_with(".github.com") {
        // `.github.com` also matches GitHub Enterprise's `github.<corp>.com`?
        // No — Enterprise hosts are arbitrary; we treat the canonical github.com
        // (+ subdomains) as GitHub and let an explicit Enterprise base be wired
        // by the caller. The common path (github.com) is what the router needs.
        Ok(IssueHost::GitHub)
    } else if host == "linear.app" {
        Ok(IssueHost::Linear)
    } else if host.ends_with(".atlassian.net") {
        Ok(IssueHost::Jira)
    } else {
        Err(Error::Validation(format!(
            "unrecognized issue host `{host}` (expected github.com / linear.app / *.atlassian.net)"
        )))
    }
}

/// The Linear/Jira routing seam Task 317 fills. Returns the typed
/// `Unimplemented` error until the native clients land.
pub fn issue_router_unimplemented(host: IssueHost) -> Error {
    let which = match host {
        IssueHost::Linear => "Linear",
        IssueHost::Jira => "Jira",
        IssueHost::GitHub => "GitHub", // unreachable in the seam path
    };
    unimplemented_err(&format!(
        "{which} issue fetch (router seam; filled by Task 317)"
    ))
}

// ---------------------------------------------------------------------------
// VcsState skeleton (`design/13 §4`)
// ---------------------------------------------------------------------------

/// Which provider/credential pool a rate-limit budget is keyed on
/// (`design/13 §4`). **FROZEN** — Task 314 keys its three rate-limit pools off
/// these three variants.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ProviderKey {
    /// The PAT-authenticated octocrab pool (5000/hr).
    GithubPat,
    /// A GitHub App installation pool, keyed by app id (15000/hr) — Task 314.
    GithubApp(String),
    /// The `gh` CLI pool (separately tracked, `design/13 §3.1`).
    GhCli,
}

/// A rate-limit budget snapshot (`design/13 §4`/§3.9). Skeleton only — Task 314
/// populates it from the `X-RateLimit-*` headers + the synthetic clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitBudget {
    pub limit: u32,
    pub remaining: u32,
    /// Reset time, epoch seconds.
    pub reset_at: i64,
}

/// In-memory VCS caches (`design/13 §4`). Skeleton: this task populates only
/// `providers` + the PR/check caches; `rate_limits` is Task 314's,
/// `webhook_secrets` is 315's, `threads_cache` is 316's.
#[derive(Default)]
pub struct VcsState {
    /// Per-repo selected provider (keyed by repo full name).
    pub providers: HashMap<String, Arc<dyn VcsProvider>>,
    /// Cached check runs, keyed by `(repo, sha)` (TTL 30s — Task 316 wires TTL).
    pub check_cache: HashMap<(String, String), Vec<CheckRun>>,
    /// Cached review threads, keyed by PR node id — Task 316 populates.
    pub threads_cache: HashMap<String, Vec<ReviewThread>>,
    /// Per-key rate-limit budgets — Task 314 populates.
    pub rate_limits: HashMap<ProviderKey, RateLimitBudget>,
    /// Per-repo webhook HMAC secrets — Task 315 populates (kept in keychain;
    /// this map is the hot cache).
    pub webhook_secrets: HashMap<String, [u8; 32]>,
}

impl VcsState {
    /// Fresh, empty state.
    pub fn new() -> Self {
        Self::default()
    }
}
