//! VCS Provider Integration (Task 45, `design/13`).
//!
//! V0.1 ships GitHub support exclusively via `gh` CLI shell-out
//! (`design/13 §2` "gh CLI shell-out" row). The full octocrab REST /
//! GraphQL client + webhook receiver + PR-set coordinated-merge
//! semantics land in V1.0; GitLab / Bitbucket in V2.0.
//!
//! Public surface:
//!
//! - [`VcsHandle`] / [`VcsProviderActor`] / [`VcsConfig`] — same actor
//!   pattern as the other Core managers.
//! - [`gh_cli`] — direct subprocess wrappers for the five `gh`
//!   commands V0.1 exercises (`pr list`, `pr view`, `pr create`,
//!   `pr merge`, `api …/check-runs`, `issue view`).

pub mod actor;
pub mod gh_cli;

pub use actor::{VcsConfig, VcsHandle, VcsProviderActor};
