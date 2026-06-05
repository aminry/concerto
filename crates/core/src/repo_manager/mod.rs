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
//! Fsmonitor + maintenance landed in Task 28. Task 301 (V1.0) adds the
//! real clone strategies (full / blobless / treeless), the pre-clone
//! repo-size → strategy recommendation, and the durable repo-local
//! `concerto-state.json` (`repo_state`). Sparse cones (302), prewarm
//! (304), and the cone-level size telemetry (305) remain follow-ons.

pub mod actor;
pub mod cones;
pub mod fsmonitor;
mod repo_state;

pub use actor::{RepoManager, RepoManagerActor, RepoManagerConfig};
