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
//! `concerto-state.json` (`repo_state`). Task 302 adds the sparse cones.
//! Task 304 adds the idle blob prewarm scheduler + the cancellable
//! `prewarm_blobs` / `PrewarmHandle` (`prefetch`). Task 305 adds the
//! cone-level size telemetry (`list_paths_in_cone` → [`cone_stats::ConeStats`],
//! read from the git index) + the unwired `suggest_cones` Maestro-delegate
//! seam ([`cone_stats::ConeSuggester`]).

pub mod actor;
pub mod cone_stats;
pub mod cones;
pub mod fsmonitor;
pub mod prefetch;
mod repo_state;

pub use actor::{RepoManager, RepoManagerActor, RepoManagerConfig, TreeEntryDomain};
pub use cone_stats::{ConeStats, ConeSuggestError, ConeSuggester};
pub use prefetch::{
    spawn_prefetch_scheduler, IdleState, NetState, PowerState, PrewarmHandle, PrewarmSignals,
};
