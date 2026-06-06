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
pub mod provider;

#[cfg(feature = "testkit")]
pub mod testkit;

pub use actor::{repo_full_name_from_url, VcsConfig, VcsHandle};
pub use dispatch::{
    choose_backend, is_no_vcs_credentials, no_vcs_credentials, route_issue_host, Backend,
    IssueHost, ProviderKey, RateLimitBudget, RepoCapabilities, VcsOp, VcsState,
};
pub use github::{GitHubProvider, DEFAULT_GITHUB_BASE_URI};
pub use github_cli::GitHubProviderViaCli;
pub use provider::{
    is_unimplemented, unimplemented_err, CheckRun, CreatePrRequest, Deployment, Issue, MergeMethod,
    MergeReport, ProviderPrId, PullRequest, RevertReport, ReviewThread, ThreadId, VcsProvider,
};
