//! Cone-level telemetry + the `suggest_cones` Maestro-delegate seam (Task
//! 305, `design/02 §3.2`/`§5.1`/`§5.2`, `PHASE3_PLANNING §2`/§4.6, D1).
//!
//! Two halves, mirroring `design/02 §3.2`'s last two paragraphs:
//!
//! 1. **Cone telemetry (LIVE in P3).** [`ConeStats`] (`file_count` +
//!    `disk_size_bytes`) is computed by [`super::RepoManager::list_paths_in_cone`]
//!    from the **git index** (NOT a filesystem walk) — the deterministic,
//!    CI-provable probe that drives the cone-picker UI (Task 322) and the
//!    P4 `create_workspace_from_description` planner (Task 411). The actual
//!    index decode lives in [`concerto_gix_wrap::cone_index_stats`] (where
//!    `gix` is a dep); this module re-shapes its result.
//!
//! 2. **`suggest_cones` (UNWIRED seam in P3).** The [`ConeSuggester`] trait +
//!    the `Option<Arc<dyn ConeSuggester>>` seam on `RepoManager`. `design/02
//!    §3.2` says the Repo Mgr "just publishes the interface"; the LLM call
//!    delegates to the Maestro Agent (08), wired in P4 (Task 411). With no
//!    injected suggester (the P3 default) the call returns
//!    [`Error::NotImplemented`]-shaped behavior — surfaced as
//!    `Status::unimplemented` at the handler (`design/02 §9`, the README's
//!    `notify_user`-stubbed-until-P5 precedent). An injected suggester is
//!    delegated to verbatim, so Task 411 wiring is a pure addition with no
//!    proto/trait change.

use std::sync::Arc;

use async_trait::async_trait;
use concerto_error::Result;
use concerto_gix_wrap::ConePath;
use concerto_persist::RepositoryId;

/// Cone-size telemetry — the Rust mirror of `concerto.v1.ConeStats` (Task
/// 305, FROZEN by `PHASE3_PLANNING §4.6`).
///
/// Read from the git index, NOT a filesystem walk (`design/02 §3.2`). See
/// [`concerto_gix_wrap::cone_index_stats`] for the (FROZEN) estimate basis:
/// `disk_size_bytes` is the sum of the in-cone file entries' recorded index
/// sizes, an order-of-magnitude figure (a blobless clone's not-yet-fetched
/// blobs read as 0).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConeStats {
    /// Tracked file entries the cone would materialize.
    pub file_count: u64,
    /// Sum of those entries' recorded sizes in the index, in bytes.
    pub disk_size_bytes: u64,
}

impl From<concerto_gix_wrap::ConeStats> for ConeStats {
    fn from(s: concerto_gix_wrap::ConeStats) -> Self {
        Self {
            file_count: s.file_count,
            disk_size_bytes: s.disk_size_bytes,
        }
    }
}

/// Plan-mode cone suggestion seam (Task 305, `design/02 §3.2`/`§9`, D1).
///
/// The Repo Mgr **publishes** this interface but does not implement it: the
/// real suggestion delegates to the Maestro Agent (08), wired in P4 (Task
/// 411). In P3 no implementor is injected, so
/// [`super::RepoManager::suggest_cones`] returns the unwired-seam signal that
/// the handler maps to `Status::unimplemented`.
///
/// `repo` identifies the repository whose tree the suggestion is scoped to;
/// `issue_text` is the issue/PR body the plan-mode flow parses. The return
/// is the suggested cone set (the same `Vec<ConePath>` shape the cone-picker
/// + `set_workarea_repo_cones` consume).
///
/// FROZEN signature (Task 305). Task 411 constructs a Maestro-backed
/// implementor and injects it via
/// [`super::RepoManager::with_cone_suggester`]; no trait/proto change then.
#[async_trait]
pub trait ConeSuggester: Send + Sync {
    /// Suggest a cone set for `repo` from `issue_text`. Delegates to the
    /// Maestro Agent (08) in the real (P4) implementor.
    async fn suggest_cones(&self, repo: &RepositoryId, issue_text: &str) -> Result<Vec<ConePath>>;
}

/// A boxed [`ConeSuggester`] seam value. `None` in P3 (the unwired default);
/// `Some` once Task 411 injects the Maestro-backed implementor.
pub type ConeSuggesterSeam = Option<Arc<dyn ConeSuggester>>;

/// Outcome of [`super::RepoManager::suggest_cones`] (Task 305, D1).
///
/// Distinguishes the **unwired-seam** case (no [`ConeSuggester`] injected —
/// the P3 default, mapped to `Status::unimplemented` at the handler) from a
/// genuine delegation failure once a suggester IS wired (P4). This keeps the
/// FROZEN contract honest: the unwired seam is NOT an empty success (which
/// would mislead the cone-picker into "no suggestions") and NOT a panic — it
/// is a typed signal the handler turns into `UNIMPLEMENTED`.
#[derive(Debug)]
pub enum ConeSuggestError {
    /// No [`ConeSuggester`] is injected. The plan-mode delegate is wired in
    /// P4 (Maestro, Task 411). The handler maps this to
    /// `Status::unimplemented`.
    Unwired,
    /// An injected suggester returned an error. The handler maps this through
    /// the usual `error_to_status` path.
    Delegate(concerto_error::Error),
}

impl std::fmt::Display for ConeSuggestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConeSuggestError::Unwired => {
                f.write_str("suggest_cones is wired in P4 (Maestro, Task 411)")
            }
            ConeSuggestError::Delegate(e) => write!(f, "suggest_cones delegate failed: {e}"),
        }
    }
}

impl std::error::Error for ConeSuggestError {}
