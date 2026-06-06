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
use std::sync::{Arc, Mutex};

use concerto_error::{Error, Result};
use url::Url;

use crate::provider::{CheckRun, Issue, ReviewThread, VcsProvider};

/// Which backend [`choose_backend`] selected for an operation.
///
/// The dispatcher returns a *decision*; the caller (the actor/handle) holds the
/// constructed providers. This keeps `choose_backend` pure + table-testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// A GitHub App installation-authed octocrab `GitHubProvider` (Task 314,
    /// R-7). Preferred over a PAT when the repo is configured for App auth
    /// (15000/hr pool, finer scope, easier rotation).
    OctocrabApp,
    /// The PAT-authed octocrab `GitHubProvider` (a PAT is configured; 5000/hr).
    Octocrab,
    /// The `gh` CLI fallback `GitHubProviderViaCli`.
    GhCli,
    /// The issue-host router owns this op (`fetch_issue`).
    IssueRouter,
}

impl Backend {
    /// The rate-limit pool ([`ProviderKey`]) calls on this backend bill against
    /// (Task 314, `design/13 §3.9`). `OctocrabApp` bills the per-app installation
    /// pool; PAT octocrab bills the PAT pool; `gh` bills its separately-tracked
    /// pool. `IssueRouter` has no PR-op pool (issue fetch is cached + not budgeted
    /// here), so it maps to the PAT pool when one applies.
    pub fn provider_key(self, app_id: Option<&str>) -> ProviderKey {
        match self {
            Backend::OctocrabApp => ProviderKey::GithubApp(app_id.unwrap_or_default().to_string()),
            Backend::Octocrab | Backend::IssueRouter => ProviderKey::GithubPat,
            Backend::GhCli => ProviderKey::GhCli,
        }
    }
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
#[derive(Debug, Clone, Copy, Default)]
pub struct RepoCapabilities {
    /// A GitHub **App** installation is configured for this repo (Task 314, R-7
    /// — `has_github_app`). Preferred over a PAT (higher quota, finer scope).
    pub has_github_app: bool,
    /// A PAT is configured for this repo (→ octocrab).
    pub has_octocrab_token: bool,
    /// `gh` is installed + authenticated (→ gh CLI).
    pub gh_available: bool,
}

/// Per-call backend choice (`design/13 §6.1`, FROZEN dispatch order). Per-call,
/// never user-chosen.
///
/// ```text
/// choose_backend(repo, op):
///     if op == fetch_issue:      use the URL-host router (Linear/Jira client)
///     elif repo.has_github_app:  use OctocrabApp   (Task 314, R-7 — preferred)
///     elif repo.has_octocrab_token: use Octocrab   (PAT)
///     elif gh_available():       use GhCli
///     else: Error::NoVcsCredentials
/// ```
///
/// Task 314 adds the `has_github_app` arm **above** the PAT arm: a repo
/// configured for App auth uses it (higher quota, finer scope, easier rotation —
/// R-7); otherwise PAT; otherwise `gh`. Still per-call, never user-chosen.
pub fn choose_backend(caps: RepoCapabilities, op: VcsOp) -> Result<Backend> {
    if op == VcsOp::FetchIssue {
        return Ok(Backend::IssueRouter);
    }
    if caps.has_github_app {
        Ok(Backend::OctocrabApp)
    } else if caps.has_octocrab_token {
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

/// The typed "external-tracker fetch blocked by `enterprise_data_privacy`"
/// error (Task 317, `design/13 §3.7` privacy floor). Reuses the FROZEN
/// `Error::Vcs` variant with a stable `vcs.external_tracker_blocked` prefix the
/// UI can switch on to explain *why* the fetch was refused (mirrors the
/// `no_vcs_credentials` convention). The Core consults the resolved project
/// setting BEFORE the outbound fetch; on a privacy-locked project the router
/// returns this rather than calling Linear/Jira.
pub fn external_tracker_blocked(host: &str) -> Error {
    Error::Vcs(format!(
        "vcs.external_tracker_blocked: {host} issue fetch refused — this project has \
         enterprise_data_privacy enabled, so issue content must not leave for an external tracker"
    ))
}

/// True when `e` is the [`external_tracker_blocked`] refusal.
pub fn is_external_tracker_blocked(e: &Error) -> bool {
    matches!(e, Error::Vcs(m) if m.starts_with("vcs.external_tracker_blocked"))
}

// ---------------------------------------------------------------------------
// Issue TTL cache (`design/13 §3.7`/§4: "fetched on demand, cached 1h in
// memory; issue bodies never persisted").
// ---------------------------------------------------------------------------

/// The issue-cache TTL: 1 hour, in seconds (`design/13 §3.7`/§4).
pub const ISSUE_CACHE_TTL_SECS: i64 = 3600;

/// A clock the [`IssueCache`] reads "now" from. Production passes
/// [`system_now_secs`]; tests pass a closure over the `testkit` `SyntheticClock`
/// so the 1 h-expiry path is deterministic. Boxed `dyn Fn` so the cache holds no
/// generic param.
pub type NowSecs = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Wall-clock "now" in epoch seconds (the production [`NowSecs`]).
pub fn system_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// An in-memory, 1 h-TTL cache of fetched issues keyed by canonicalized URL
/// (`design/13 §3.7`/§4). Issue bodies are held ONLY here — never written to
/// SQLite (the privacy floor). Cheap-clone (`Arc<Mutex<…>>`), so the router can
/// share one cache across calls.
///
/// The clock is injectable ([`NowSecs`]) so the TTL test drives expiry with the
/// `testkit` synthetic clock instead of sleeping an hour.
#[derive(Clone)]
pub struct IssueCache {
    inner: Arc<Mutex<HashMap<String, (i64, Issue)>>>,
    now: NowSecs,
}

impl IssueCache {
    /// Build a cache that reads time from `now`.
    pub fn new(now: NowSecs) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            now,
        }
    }

    /// Build a cache on the wall clock (the production path).
    pub fn system() -> Self {
        Self::new(Arc::new(system_now_secs))
    }

    /// Canonicalize a URL for use as a cache key: `scheme://host/path` with the
    /// host lowercased (query + fragment dropped, since the same issue is the
    /// same issue regardless of trailing query). A bare id (not a URL) keys on
    /// the trimmed string verbatim.
    pub fn canonical_key(url: &str) -> String {
        match Url::parse(url.trim()) {
            Ok(u) => {
                let host = u.host_str().unwrap_or("").to_ascii_lowercase();
                format!("{}://{}{}", u.scheme(), host, u.path())
            }
            // Not a URL (a bare id) → key on the trimmed string verbatim.
            Err(_) => url.trim().to_string(),
        }
    }

    /// Look up a still-fresh cached issue for `url`, evicting it if expired.
    /// Returns `None` on a miss or an expired entry.
    pub fn get(&self, url: &str) -> Option<Issue> {
        let key = Self::canonical_key(url);
        let now = (self.now)();
        let mut map = self.inner.lock().expect("issue cache mutex");
        match map.get(&key) {
            Some((stored_at, issue)) if now - *stored_at < ISSUE_CACHE_TTL_SECS => {
                Some(issue.clone())
            }
            Some(_) => {
                map.remove(&key);
                None
            }
            None => None,
        }
    }

    /// Insert (or refresh) the cached issue for `url` at the current time.
    pub fn put(&self, url: &str, issue: Issue) {
        let key = Self::canonical_key(url);
        let now = (self.now)();
        self.inner
            .lock()
            .expect("issue cache mutex")
            .insert(key, (now, issue));
    }
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
    /// Per-key rate-limit budgets (the `design/13 §4` map skeleton 313 froze).
    /// Task 314 keeps the live, debounced/queue-aware tracking in [`pools`]; this
    /// map is the materialized snapshot (refreshed via [`VcsState::sync_rate_limits`])
    /// so a reader holding only the frozen field still sees the current budgets.
    ///
    /// [`pools`]: VcsState::pools
    pub rate_limits: HashMap<ProviderKey, RateLimitBudget>,
    /// The live three-pool rate-limit tracker (Task 314, `design/13 §3.9`). Keyed
    /// off [`ProviderKey`] (App / PAT / `gh`), independent per pool. Owns the
    /// `vcs.rate_limit_warning` debounce + the per-pool resume queue. The
    /// dispatcher seeds it from `X-RateLimit-*` headers after each call.
    pub pools: crate::rate_limit::RateLimitPools,
    /// Per-repo webhook HMAC secrets — Task 315 populates (kept in keychain;
    /// this map is the hot cache).
    pub webhook_secrets: HashMap<String, [u8; 32]>,
}

impl VcsState {
    /// Fresh, empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Refresh the FROZEN `rate_limits` map from the live [`pools`] tracker
    /// (Task 314). Call after observing headers so a consumer reading the
    /// `design/13 §4` map skeleton sees the current per-pool budgets.
    ///
    /// [`pools`]: VcsState::pools
    pub fn sync_rate_limits(&mut self) {
        self.rate_limits = self.pools.snapshot().into_iter().collect();
    }

    /// The Settings → Diagnostics read accessor for the three rate-limit pools
    /// (Task 314, `design/13 §3.9` "soft warning in Settings → Diagnostics").
    /// Returns each pool's `(ProviderKey, RateLimitBudget)` sorted stably. The
    /// full UI / RPC is Task 324 / 709; this is the data source they call.
    pub fn rate_limit_diagnostics(&self) -> Vec<(ProviderKey, RateLimitBudget)> {
        self.pools.snapshot()
    }
}
