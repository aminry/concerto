//! Concerto VCS Provider Integration (`design/13`, Task 313).
//!
//! The dedicated crate that owns ALL interaction with external version-control
//! hosts (mirrors the sibling `crates/relay` / `crates/transport` layout). It
//! houses:
//!
//! - The **FROZEN [`VcsProvider`] trait** + its value types ([`provider`],
//!   transcribed from `design/13 §3.8` — a V2.0 stability contract).
//! - **[`GitHubProvider`]** ([`github`]) — the default backend on `octocrab`
//!   (rustls, PAT; GitHub-Enterprise base URL per R-10).
//! - **[`GitHubProviderViaCli`]** ([`github_cli`]) — the `gh` shell-out fallback,
//!   a thin trait adapter over the V0.1 [`gh_cli`] module (moved verbatim).
//! - **[`dispatch`]** — the per-call [`choose_backend`] decision
//!   (`design/13 §6.1`), the `fetch_issue` URL-host router, and the
//!   [`VcsState`]/[`ProviderKey`] cache skeleton (`design/13 §4`).
//! - **[`VcsHandle`]** ([`actor`]) — the cheap-clone handle with the FROZEN
//!   Task-45 method set (reused unchanged by the Core's `Vcs` gRPC handler) plus
//!   the new [`VcsHandle::fetch_issue_url`] router.
//! - **[`LinearClient`]** ([`linear`]) + **[`JiraClient`]** ([`jira`]) — the
//!   native issue-fetch clients (Task 317): a hand-rolled Linear GraphQL query
//!   and a Jira REST GET (ADF flattened to text, one OAuth refresh on 401), each
//!   mapping to the shared [`Issue`]. Wired into the [`VcsHandle::fetch_issue_url`]
//!   host router behind a 1 h in-memory [`IssueCache`]; issue bodies are never
//!   persisted (`design/13 §3.7` privacy floor).
//! - **[`write_back`]** — the FROZEN [`IssueWriteBack`] trait + LIVE no-op
//!   [`NoopWriteBack`] (Task 317, D5); the real status-transition-on-merge lands
//!   in Task 320.5 behind the same trait.
//! - **[`testkit`]** (behind `--features testkit`) — the shared wiremock-backed
//!   `FakeGitHub`/`FakeLinear`/`FakeJira` harness 314/315/316/317/320/320.5
//!   reuse (D2).
//!
//! The supervised `VcsProviderActor` (which needs the Core's `supervisor::Actor`
//! trait) stays in `crates/core/src/vcs/` and wraps [`VcsHandle`] — so the
//! Core's `boot.rs` + gRPC handler compile unchanged. The V0.1 `Vcs` gRPC
//! service is untouched; the trait is the *internal* surface.

pub mod actor;
pub mod dispatch;
pub mod gh_cli;
pub mod github;
pub mod github_cli;
pub mod jira;
pub mod linear;
pub mod provider;
pub mod rate_limit;
pub mod write_back;

#[cfg(feature = "testkit")]
pub mod testkit;

pub use actor::{repo_full_name_from_url, IssueFetchCreds, VcsConfig, VcsHandle};
pub use dispatch::{
    choose_backend, external_tracker_blocked, is_external_tracker_blocked, is_no_vcs_credentials,
    no_vcs_credentials, route_issue_host, system_now_secs, Backend, IssueCache, IssueHost, NowSecs,
    ProviderKey, RateLimitBudget, RepoCapabilities, VcsOp, VcsState, ISSUE_CACHE_TTL_SECS,
};
pub use github::{GitHubProvider, NowSecs as GithubNowSecs, DEFAULT_GITHUB_BASE_URI};
pub use github_cli::GitHubProviderViaCli;
pub use jira::{flatten_adf, parse_jira_key, JiraClient, RefreshToken};
pub use linear::{parse_linear_id, LinearClient, DEFAULT_LINEAR_BASE_URI};
pub use provider::{
    is_unimplemented, unimplemented_err, CheckRun, CreatePrRequest, Deployment, Issue, MergeMethod,
    MergeReport, ProviderPrId, PullRequest, RevertReport, ReviewThread, ThreadId, VcsProvider,
};
pub use rate_limit::{
    check_run_backoff_secs, degraded_interval_secs, is_rate_limited, rate_limited,
    rate_limited_reset_at, OpPriority, RateLimitPools, RateLimitWarning, ResumeQueue,
    CHECK_RUN_BACKOFF_SECS, DEGRADE_FRACTION, DEPLOYMENT_SECS, PR_STATE_BACKGROUND_SECS,
    PR_STATE_FOREGROUND_SECS, REVIEW_THREAD_SECS, WARN_FRACTION,
};
pub use write_back::{IssueProvider, IssueRef, IssueTransition, IssueWriteBack, NoopWriteBack};
