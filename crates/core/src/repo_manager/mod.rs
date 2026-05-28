//! Repository Manager subsystem (Task 18, design/02).
//!
//! Owns the per-repo write lock and the V0.1 git operations the gRPC
//! `Repositories` service exposes:
//!
//! - `add_repository(project_id, url, name, default_branch)` — persists
//!   a new `repositories` row and returns the assigned [`RepositoryId`].
//! - `clone(repo_id, progress)` — locks the per-repo mutex, runs a full
//!   clone via `concerto-gix-wrap`, updates `last_fetch_at` on success.
//!
//! Fsmonitor, maintenance, sparse cones, prewarm, and repo-size
//! auto-recommendation are V1.0 (Task 28+).

pub mod actor;
pub mod fsmonitor;

pub use actor::{RepoManager, RepoManagerActor, RepoManagerConfig};
