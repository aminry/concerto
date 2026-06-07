//! Workarea-creation logic (Task 20).
//!
//! Sits alongside [`crate::workspace_manager::actor::WorkspaceManager`]:
//! the [`WorkareaManager`] handle owns workarea lifecycle (create / get /
//! list / archive), worktree setup, and the `.context/` skeleton.
//!
//! ## Contract (Task 20, generalized to 1..N repos by Task 306)
//!
//! - `create_workarea` validates the workspace exists, is not archived,
//!   and has **at least one** repository attached (a 0-repo workspace is
//!   rejected per `design/03 §8`). It then materializes **one worktree
//!   per repo** (in `workspace_repos.position` order) inside a single
//!   workarea root: `git worktree add` + per-repo files-to-copy +
//!   per-repo `workarea_repos` row, with the `.context/` skeleton laid
//!   down once at the root. All DB writes commit in one transaction; any
//!   per-repo `git worktree add` failure aborts the whole create and
//!   cleans up every worktree built so far (the soft `partial` path is
//!   Task 307).
//! - Composer-name allocation picks the lowest-index name in
//!   [`crate::workspace_manager::COMPOSERS`] not already in use within
//!   the workspace; falls back to `<composer>-N` when the pool is
//!   exhausted. UNIQUE(`workspace_id, composer_name`) collisions trigger
//!   a retry with the next name.
//! - Branch name is `concerto/<composer>` (branch-rename hook lands in
//!   V1.0).
//! - `worktree_root` is `<data_dir>/workspaces/<workspace.slug>/<composer>/`.
//! - On-disk layout per `design/03 §4.2`:
//!   ```text
//!   <worktree_root>/
//!   ├── .context/
//!   │   ├── PROMPT.md
//!   │   ├── todos.md
//!   │   └── scratch/
//!   ├── <repo[0].name>/     ← git worktree add target (one per repo)
//!   ├── <repo[1].name>/
//!   └── …
//!   ```
//! - `.context/` is appended to each worktree's `.git/info/exclude` so
//!   agent scratch is not tracked.
//! - Workarea row + one `workarea_repos` row **per repo** + `created →
//!   active` status transition all commit in one transaction.
//! - On success, [`WorkareaEvent::Created`] is published on the
//!   broadcast channel.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use concerto_error::{Error, Result};
use concerto_persist::{
    NewWorkarea, NewWorkareaRepo, Persistence, Repository, Workarea, WorkareaId, WorkspaceId,
};
use concerto_vcs::provider::{
    MergeMethod, MergeReport as ProviderMergeReport, RevertReport as ProviderRevertReport,
};
use sqlx::Connection;
use tokio::sync::{broadcast, mpsc};

#[cfg(unix)]
use crate::agent_supervisor::AgentSupervisorHandle;
use crate::repo_manager::RepoManager;
use crate::supervisor::{Actor, ActorContext};
use crate::workspace_manager::archive::{
    recreate_worktrees, remove_worktrees_and_root, ArchiveOpts,
};
use crate::workspace_manager::{context_dir, files_to_copy, COMPOSERS};

/// Maximum number of composer-name suffix retries before giving up. 100
/// keeps runaway loops bounded; the pool is large enough that real
/// workloads stop long before this.
const MAX_COMPOSER_ATTEMPTS: u32 = 100;

/// Channel capacity for the in-process broadcast of [`WorkareaEvent`]s.
/// The future `Streams` service (Task 24) consumes from a subscriber.
const BROADCAST_CAPACITY: usize = 256;

/// Config for the actor's `run` loop. V0.1 has no knobs yet — the actor
/// parks on shutdown, mirroring `WorkspaceManagerActor`.
#[derive(Clone, Debug, Default)]
pub struct WorkareaManagerConfig;

/// Events published on workarea-state changes.
///
/// The `Created` payload carries the full `Workarea` row (~232 bytes
/// with `settings_json` added in Task 30); the size delta vs `Archived`
/// triggers `clippy::large_enum_variant`. Keeping the broadcast payload
/// unboxed matches `WorkspaceEvent::Created(Workspace)` next door, so
/// we silence the lint locally rather than box only this variant —
/// future events (status changes, branch rename) will also carry the
/// full row by convention.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum WorkareaEvent {
    /// A new workarea was created. Payload is the persisted row (with
    /// `status == "active"`).
    Created(Workarea),
    /// A workarea was archived. Payload is the workarea id.
    Archived(WorkareaId),
    /// A workarea was restored from archive (Task 31). Payload is the
    /// post-restore row (status reset to `"active"`, `permission_mode`
    /// reset to `NULL` per `design/03 §3.7`).
    Restored(Workarea),
    /// Task 307: a workarea's `status` changed through the
    /// [`WorkareaManager::transition_workarea`] FSM funnel (driven by
    /// Agent Supervisor session events or a user pause/resume). Carries
    /// the id and the from→to SQL status strings so subscribers (and the
    /// audit log) see the transition without re-reading the row.
    StatusChanged {
        id: WorkareaId,
        from: String,
        to: String,
    },
    /// Task 307: a multi-repo `create_workarea` finished as `partial` —
    /// ≥1 repo's `git worktree add` failed (`design/03 §8`). Carries the
    /// workarea id + the `repository_id`s that failed so the UI/retry can
    /// target exactly the unmaterialized repos.
    PartialCreate {
        id: WorkareaId,
        failed_repository_ids: Vec<String>,
    },
    /// Task 320: one coordinated-merge step succeeded (merged + checks passed).
    /// Carries the workarea id + `(step, total)` + the merged repo + merge SHA.
    /// Rides `workarea.events` AND the new `pr_set.events` subject (opaque JSON).
    PrSetMergeStepCompleted {
        id: WorkareaId,
        step: i32,
        total: i32,
        repository_full_name: String,
        pr_number: i64,
        merge_sha: String,
    },
    /// Task 320: a coordinated-merge step FAILED (checks failed / timed out /
    /// merge rejected). The loop pauses here without auto-reverting (`design/03
    /// §6.4`); the UI surfaces "Step N of M failed — auto-revert?".
    PrSetMergeFailedStep {
        id: WorkareaId,
        step: i32,
        total: i32,
        reason: String,
    },
    /// Task 320: every member of the PR set merged + passed checks.
    PrSetMerged { id: WorkareaId, total: i32 },
    /// Task 320: one member of the set was reverted by a coordinated revert.
    PrReverted {
        id: WorkareaId,
        repository_full_name: String,
        pr_number: i64,
    },
}

// ===========================================================================
// Task 320 — coordinated PR-set merge / revert types + the VCS merge seam
// ===========================================================================

/// Default `wait_for_check_runs` timeout for a coordinated-merge step
/// (`design/13 §3.5` / `design/05 §7.4`): 10 minutes.
pub const DEFAULT_MERGE_CHECK_TIMEOUT: Duration = Duration::from_secs(600);

/// One ordered member of a workarea's coordinated-merge plan (`design/03 §3.9`).
/// The `(repo, PR)` tuple resolved from the `pull_requests` rows, sorted by
/// `merge_order` (Task 319). `step`/`total` are 1-based positional indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeStep {
    pub step: i32,
    pub total: i32,
    pub repository_id: String,
    pub repository_full_name: String,
    pub pr_number: i64,
    pub head_sha: String,
    pub merge_order: i64,
    pub state: String,
}

/// The read-only coordinated-merge preview (`design/03 §5.1`): the ordered list
/// of `(repo, PR)` steps the UI renders and the merge loop iterates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergePlan {
    pub workarea_id: String,
    pub steps: Vec<MergeStep>,
}

/// Options for [`WorkareaManager::merge_workarea_pr_set`].
#[derive(Debug, Clone)]
pub struct MergeOpts {
    /// Merge method (`merge|squash|rebase`).
    pub method: MergeMethod,
    /// Per-step `wait_for_check_runs` timeout.
    pub timeout: Duration,
    /// `design/03/13 R-6` merge-anyway-despite-red override. Gated by
    /// `managed.json`'s `allowMergeWithFailingChecks`; when permitted, a
    /// non-`passed` checks outcome is a typed warning + audit entry instead of a
    /// pause.
    pub allow_failing_checks: bool,
}

impl Default for MergeOpts {
    fn default() -> Self {
        Self {
            method: MergeMethod::Merge,
            timeout: DEFAULT_MERGE_CHECK_TIMEOUT,
            allow_failing_checks: false,
        }
    }
}

/// Options for [`WorkareaManager::revert_workarea_pr_set`].
#[derive(Debug, Clone, Default)]
pub struct RevertOpts {
    /// Opt into the hard-reset strategy (`design/13 R-5`); default is the
    /// revert-commit strategy.
    pub hard_reset: bool,
}

/// Why a coordinated-merge step failed (mirrors the proto `FailureKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    ChecksFailed,
    ChecksTimeout,
    MergeConflict,
    MergeRejected,
}

/// A single frame emitted on the [`ProgressSink`] as the coordinated merge runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeProgress {
    StepStarted {
        step: i32,
        total: i32,
        repository_full_name: String,
        pr_number: i64,
    },
    StepCompleted {
        step: i32,
        total: i32,
        merge_sha: String,
    },
    StepFailed {
        step: i32,
        total: i32,
        reason: String,
        kind: FailureKind,
    },
    SetMerged {
        total: i32,
    },
    SetPaused {
        paused_at_step: i32,
        total: i32,
        reason: String,
    },
}

/// The channel the merge loop feeds [`MergeProgress`] frames into; the gRPC
/// server-stream handler holds the receiver and forwards them to the client.
pub type ProgressSink = mpsc::Sender<MergeProgress>;

/// Summary returned by [`WorkareaManager::merge_workarea_pr_set`]. The stream is
/// the source of truth for the live client; this is the terminal verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeReport {
    /// How many members merged successfully (and passed checks / were
    /// overridden).
    pub merged_steps: i32,
    /// Total members in the plan.
    pub total: i32,
    /// `Some(n)` (1-based) when the loop paused at step `n` without merging it;
    /// `None` when the whole set merged.
    pub paused_at_step: Option<i32>,
}

/// Per-member outcome of a coordinated revert (mirrors the proto `RevertStep`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevertOutcome {
    Reverted,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertStep {
    pub repository_full_name: String,
    pub pr_number: i64,
    pub outcome: RevertOutcome,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertReport {
    pub workarea_id: String,
    pub steps: Vec<RevertStep>,
}

/// The single-PR merge/revert seam the coordinated loop drives (`design/13
/// §3.5`: the loop sequences `VcsProvider::merge_pr` / `revert_pr`). A local
/// trait over the foreign [`concerto_vcs::VcsHandle`] (allowed by the orphan
/// rule, mirroring 318's `CheckRunsSource for VcsHandle`) so tests can inject a
/// scripted double without spinning up `gh`/octocrab. The production impl builds
/// an octocrab provider from the keychain PAT and routes through the trait.
#[async_trait]
pub trait PrSetVcs: Send + Sync + 'static {
    /// Merge one PR, returning the post-merge merge-commit SHA (`design/13
    /// §7.2`: the SHA `wait_for_check_runs` waits on is the MERGE commit, not the
    /// PR head).
    async fn merge_pr(
        &self,
        repository_id: &concerto_persist::RepositoryId,
        repository_full_name: &str,
        pr_number: i64,
        method: MergeMethod,
    ) -> Result<ProviderMergeReport>;

    /// Revert one merged PR (revert-commit by default; `hard_reset` opt-in,
    /// `design/13 R-5`).
    async fn revert_pr(
        &self,
        repository_id: &concerto_persist::RepositoryId,
        repository_full_name: &str,
        pr_number: i64,
        hard_reset: bool,
    ) -> Result<ProviderRevertReport>;
}

/// Cloneable handle to the Workarea Manager's shared state.
#[derive(Clone)]
pub struct WorkareaManager {
    persistence: Arc<Persistence>,
    repo_manager: RepoManager,
    /// `<data_dir>` — the workarea root is computed as
    /// `<data_dir>/workspaces/<workspace.slug>/<composer>/`.
    data_dir: Arc<PathBuf>,
    /// `<config_dir>` — used by Task 32's
    /// [`update_workarea_permission_mode`] /
    /// [`set_workarea_bypass_destructive_guard`] paths to read
    /// `managed.json`.
    config_dir: Arc<PathBuf>,
    events: broadcast::Sender<WorkareaEvent>,
    /// Optional Agent Supervisor handle (Task 31). Held so
    /// [`archive_workarea`] can drive `stop_session(reason=archive)` on
    /// every live session before tearing down the worktree. `None` in the
    /// in-process unit tests that don't spawn agent hosts.
    #[cfg(unix)]
    agent_supervisor: Option<AgentSupervisorHandle>,
    /// Task 308: the shared per-workarea edit-mutex registry. The
    /// Workarea Manager holds an `Arc` to the **same** registry the Agent
    /// Supervisor acquires on writes (constructed once in `boot.rs`), so
    /// it can read [`crate::workspace_manager::EditMutexRegistry::holder`]
    /// for UI / diagnostics ("blocked on `<session>`"). `None` in unit
    /// tests that don't wire it.
    edit_mutex: Option<Arc<crate::workspace_manager::EditMutexRegistry>>,
    /// Task 320: the single-PR merge/revert seam the coordinated loop drives.
    /// Wired at boot via [`Self::with_vcs`]; tests inject a scripted double via
    /// [`Self::with_pr_set_vcs`]. `None` ⇒ the coordinated merge/revert return a
    /// typed `vcs.not_configured` error.
    pr_set_vcs: Option<Arc<dyn PrSetVcs>>,
    /// Task 320: the Scheduler handle whose `wait_for_check_runs` the merge loop
    /// blocks on between members (Task 318). `#[cfg(unix)]`-gated to match the
    /// unix-only Scheduler module (agent-host PTY is unix-only in V1.0; Windows
    /// scheduler is Task 702/Phase 7). On non-unix the coordinated merge degrades
    /// to a typed "unsupported on this platform" error.
    #[cfg(unix)]
    scheduler: Option<crate::scheduler::SchedulerHandle>,
}

impl WorkareaManager {
    /// Build a fresh handle. Normally callers go through
    /// [`WorkareaManagerActor::new`]; this is `pub` so tests can
    /// construct one without the supervisor.
    pub fn new(
        persistence: Arc<Persistence>,
        repo_manager: RepoManager,
        data_dir: Arc<PathBuf>,
        config_dir: Arc<PathBuf>,
    ) -> Self {
        let (events, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            persistence,
            repo_manager,
            data_dir,
            config_dir,
            events,
            #[cfg(unix)]
            agent_supervisor: None,
            edit_mutex: None,
            pr_set_vcs: None,
            #[cfg(unix)]
            scheduler: None,
        }
    }

    /// Task 320: attach the [`concerto_vcs::VcsHandle`] as the coordinated-merge
    /// VCS seam (FROZEN signature). Wraps the handle into the production
    /// [`PrSetVcs`] impl (which builds an octocrab provider from the keychain PAT
    /// per member). Mirrors the [`Self::with_agent_supervisor`] builder pattern;
    /// populated at boot after the VCS handle exists.
    pub fn with_vcs(mut self, vcs: concerto_vcs::VcsHandle) -> Self {
        self.pr_set_vcs = Some(Arc::new(VcsHandleMerger { vcs }));
        self
    }

    /// Task 320: inject a [`PrSetVcs`] double directly (tests). Production uses
    /// [`Self::with_vcs`].
    pub fn with_pr_set_vcs(mut self, vcs: Arc<dyn PrSetVcs>) -> Self {
        self.pr_set_vcs = Some(vcs);
        self
    }

    /// Task 320: attach the Scheduler handle whose `wait_for_check_runs` the
    /// merge loop blocks on (FROZEN signature). `#[cfg(unix)]`-gated — the
    /// Scheduler module is unix-only. Populated at boot.
    #[cfg(unix)]
    pub fn with_scheduler(mut self, scheduler: crate::scheduler::SchedulerHandle) -> Self {
        self.scheduler = Some(scheduler);
        self
    }

    /// Attach an [`AgentSupervisorHandle`] so archive cascades can stop
    /// live sessions. Used by the production binary; integration tests
    /// can leave the supervisor `None` when they don't need session
    /// shutdown wired.
    #[cfg(unix)]
    pub fn with_agent_supervisor(mut self, supervisor: AgentSupervisorHandle) -> Self {
        self.agent_supervisor = Some(supervisor);
        self
    }

    /// Task 308: attach the shared per-workarea edit-mutex registry
    /// (`PHASE3_PLANNING §2`). The **same** `Arc` is handed to the Agent
    /// Supervisor (which acquires the lock on write-class tool calls) so
    /// both subsystems share one lock per workarea. The Workarea Manager
    /// only ever *reads* the holder via [`Self::edit_mutex_holder`].
    /// Mirrors the [`Self::with_agent_supervisor`] / `with_audit` builder
    /// pattern. Construct exactly one registry in `boot.rs`.
    pub fn with_edit_mutex_registry(
        mut self,
        registry: Arc<crate::workspace_manager::EditMutexRegistry>,
    ) -> Self {
        self.edit_mutex = Some(registry);
        self
    }

    /// Task 308: the session currently holding `workarea`'s edit lock, or
    /// `None` if the registry isn't wired, no lock exists for the
    /// workarea, or it is unheld. For UI / diagnostics only — acquiring
    /// the lock is the Agent Supervisor's job.
    pub async fn edit_mutex_holder(
        &self,
        workarea: &WorkareaId,
    ) -> Option<concerto_persist::SessionId> {
        let reg = self.edit_mutex.as_ref()?;
        reg.holder(workarea).await
    }

    /// Subscribe to `workarea.events`.
    pub fn subscribe(&self) -> broadcast::Receiver<WorkareaEvent> {
        self.events.subscribe()
    }

    /// Probe every non-archived workarea; mark rows whose `worktree_root`
    /// directory is gone from disk as `'crashed'` (`design/03 §6.5`).
    ///
    /// Called once at Core boot from `boot.rs` so a Concerto reinstall
    /// or `data_dir` wipe doesn't leave stale `active` rows pointing at
    /// non-existent worktrees. Returns the number of rows adopted.
    ///
    /// Task 307: the `→ crashed` transition routes through
    /// [`Self::transition_workarea`] (the `AdoptCrashed` FSM event) so each
    /// adoption audits + broadcasts `StatusChanged` like every other status
    /// change. A workarea already in a state with no `AdoptCrashed` edge
    /// (only `Archived` today) is skipped via the soft-reject path.
    pub async fn adopt_crashed_workareas(&self) -> Result<usize> {
        let missing =
            crate::workspace_manager::archive::list_missing_worktree_workareas(&self.persistence)
                .await?;
        let mut adopted = 0usize;
        for id in missing {
            match self
                .transition_workarea(
                    &id,
                    crate::workspace_manager::fsm::WorkareaEvent::AdoptCrashed,
                )
                .await
            {
                Ok(_) => {
                    adopted += 1;
                    tracing::info!(workarea = %id, "adopted crashed workarea");
                }
                // Soft-reject (e.g. an already-`crashed` row whose state has
                // no AdoptCrashed edge): not an error, just skip.
                Err(Error::Policy(_)) => {}
                Err(e) => tracing::warn!(
                    workarea = %id,
                    error = %e,
                    "failed to mark workarea crashed during boot sweep"
                ),
            }
        }
        Ok(adopted)
    }

    /// Create a workarea.
    ///
    /// Steps (per `design/03 §3.3` + §6.2; generalized to 1..N repos by
    /// Task 306):
    /// 1. Validate workspace exists + not archived; resolve its repos in
    ///    `workspace_repos.position` order (≥1 required, `design/03 §8`).
    /// 2. Ensure each repo is cloned on disk (via [`RepoManager`]).
    /// 3. Allocate a composer name + branch + worktree root path.
    /// 4. Lay down `.context/{PROMPT.md, todos.md, scratch/}` once at the
    ///    workarea root.
    /// 5. For **each** repo (in position order): run `git worktree add`
    ///    into `<worktree_root>/<repo.name>/`, append `.context/` to that
    ///    worktree's `.git/info/exclude`, and apply files-to-copy.
    /// 6. Persist `workareas` (status `"created"`) + one `workarea_repos`
    ///    row per **materialized** repo + transition to `"active"`
    ///    (all repos succeeded) or `"partial"` (≥1 repo's worktree-add
    ///    failed) in one transaction.
    /// 7. Emit [`WorkareaEvent::Created`]; on a partial create also emit
    ///    [`WorkareaEvent::PartialCreate`] with the failing repo ids.
    ///
    /// Task 307: a per-repo `git worktree add` failure no longer aborts a
    /// multi-repo create. The repos that succeeded are kept + persisted and
    /// the workarea is stamped `partial` (`design/03 §8`); the failing repo
    /// ids ride out on `workarea.events` for retry targeting. Only when
    /// **no** repo materialized (the sole repo of a single-repo workarea
    /// failed, or every repo failed) is the whole create aborted + cleaned
    /// up (306's behavior for the case where `partial` is meaningless).
    pub async fn create_workarea(
        &self,
        workspace_id: &str,
        permission_mode: Option<String>,
    ) -> Result<Workarea> {
        if workspace_id.is_empty() {
            return Err(Error::Validation("workspace_id is required".into()));
        }
        if let Some(mode) = permission_mode.as_deref() {
            validate_permission_mode(mode)?;
        }

        // Workspace must exist + not be archived.
        let ws_id = WorkspaceId(workspace_id.to_string());
        let workspace = concerto_persist::workspaces::get(self.persistence.readers(), &ws_id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("workspace {workspace_id} not found")))?;
        if workspace.archived_at.is_some() {
            return Err(Error::Validation(format!(
                "workspace.archived: workspace {workspace_id} is archived"
            )));
        }

        // Task 306: 1..N repositories attached, in `workspace_repos.position`
        // order (the FROZEN declaration order). A 0-repo workspace is
        // rejected here per `design/03 §8` (such a workspace can't
        // materialize any worktree).
        let repo_ids =
            concerto_persist::workspaces::list_repos(self.persistence.readers(), &ws_id).await?;
        if repo_ids.is_empty() {
            return Err(Error::Validation(format!(
                "workspace.no_repos: workspace {workspace_id} has no repositories attached; \
                 add at least one repo before creating a workarea"
            )));
        }
        // Resolve each repo row (in position order). The `local_path` /
        // `name` drive the per-repo clone + worktree path below.
        let mut repos: Vec<Repository> = Vec::with_capacity(repo_ids.len());
        for repo_id in &repo_ids {
            let repo = concerto_persist::repositories::get(self.persistence.readers(), repo_id)
                .await?
                .ok_or_else(|| {
                    Error::Internal(format!(
                        "workspace_repos points at non-existent repository {repo_id}"
                    ))
                })?;
            repos.push(repo);
        }

        // Ensure every repo is cloned on disk. If `local_path/.git`
        // already exists, the prior clone is reused. Otherwise we clone
        // synchronously (no progress sink — workarea creation is a
        // single user-facing action, the gRPC reply is the progress).
        // Driven once up-front (outside the composer-retry loop) so a
        // collision retry doesn't re-clone.
        for repo in &repos {
            let repo_local = PathBuf::from(&repo.local_path);
            if !repo_local.join(".git").exists() && !repo_local.join("HEAD").exists() {
                // Not a clone yet. Drive the per-repo lock + clone via the
                // RepoManager so a concurrent create_workarea on the same
                // repo serializes. `clone_repo` is idempotent at the FS
                // layer (git refuses if dest exists & non-empty).
                self.repo_manager.clone_repo(&repo.id, None).await?;
            }
        }

        // Allocate composer name with collision retry. The loop body
        // computes the candidate, builds on-disk artefacts for **every**
        // repo, then opens a transaction. On UNIQUE violation we roll
        // back, clean up the FS work for all repos, and try the next name.
        let now_ms = now_unix_ms();
        let mut attempt: u32 = 0;
        let workarea = loop {
            attempt += 1;
            if attempt > MAX_COMPOSER_ATTEMPTS {
                return Err(Error::Internal(format!(
                    "exhausted {MAX_COMPOSER_ATTEMPTS} composer allocation attempts for workspace {workspace_id}"
                )));
            }

            // Refresh the set of used names every attempt so a parallel
            // create_workarea's commit gets picked up.
            let used = concerto_persist::workareas::list_composer_names_in_workspace(
                self.persistence.readers(),
                &ws_id,
            )
            .await?;
            let composer = allocate_composer(&used).ok_or_else(|| {
                Error::Internal(format!(
                    "composer allocation exhausted suffix space for workspace {workspace_id}"
                ))
            })?;

            let branch = format!("concerto/{composer}");
            let worktree_root = self
                .data_dir
                .join("workspaces")
                .join(&workspace.slug)
                .join(&composer);

            // 1. Ensure the workarea root directory exists. If a prior
            //    failed attempt left stuff behind we'll discover and
            //    remove it before re-trying.
            tokio::fs::create_dir_all(&worktree_root).await?;

            // 2. Create the `.context/` skeleton ONCE at the workarea
            //    root (Task 30 expansion: adds `checkpoints/` and seeds
            //    PROMPT.md / todos.md bodies). Shared across all repos.
            context_dir::apply(&worktree_root).await?;

            // 3. For each repo (in position order): worktree add +
            //    exclude + files-to-copy. Track the per-repo worktree
            //    paths so a UNIQUE collision can clean up every worktree
            //    built so far. Task 307: a per-repo failure no longer
            //    aborts the whole multi-repo create — we record the
            //    failing repo, KEEP the repos that succeeded, and stamp the
            //    workarea `partial` (`design/03 §8`). Only when **no** repo
            //    succeeded (e.g. a single-repo workarea whose sole repo
            //    failed, or every repo failed) do we abort + clean up.
            //    Indices into `repos` for the repos that fully materialized.
            let mut ok_repo_idx: Vec<usize> = Vec::with_capacity(repos.len());
            // (repo_local, worktree_dir) for the materialized worktrees —
            // the collision-retry / abort cleanup list.
            let mut built: Vec<(PathBuf, PathBuf)> = Vec::with_capacity(repos.len());
            // The repos whose `git worktree add` (or follow-on setup)
            // failed — surfaced on `workarea.events` for retry targeting.
            let mut failed_repo_ids: Vec<String> = Vec::new();
            // The first error seen — propagated verbatim if we end up
            // aborting (no repo succeeded).
            let mut first_err: Option<Error> = None;

            for (idx, repo) in repos.iter().enumerate() {
                let repo_local = PathBuf::from(&repo.local_path);
                let repo_worktree = worktree_root.join(&repo.name);

                // 3a. Run `git worktree add` for this repo (the expensive
                //     step). On failure, record it and move to the next
                //     repo (soft `partial` path).
                if let Err(e) =
                    concerto_gix_wrap::worktree_add(&repo_local, &branch, &repo_worktree).await
                {
                    tracing::warn!(repo = %repo.id, error = %e, "worktree_add failed; marking repo for partial");
                    failed_repo_ids.push(repo.id.0.clone());
                    first_err.get_or_insert(e);
                    continue;
                }

                // 3b. Append `.context/` to this worktree's
                //     `.git/info/exclude`. Each worktree owns its own
                //     `.git/info/`; the worktree's `.git` is a pointer
                //     file, so we resolve the real `info/` via git's
                //     own layout.
                if let Err(e) = append_context_to_git_exclude(&repo_worktree).await {
                    tracing::warn!(repo = %repo.id, error = %e, "git exclude setup failed; marking repo for partial");
                    failed_repo_ids.push(repo.id.0.clone());
                    first_err.get_or_insert(e);
                    // Best-effort: undo the half-built worktree for this repo
                    // so a later retry starts clean.
                    let _ = remove_worktree_best_effort(&repo_local, &repo_worktree).await;
                    continue;
                }

                // 3c. Apply files-to-copy rules from this repo's
                //     `.concerto/.worktreeinclude` into its new worktree.
                //     Missing rules file → no-op. The `ignore` walker is
                //     sync; offload to a blocking pool. Per-repo reference
                //     worktree = that repo's `local_path` (the multi-repo
                //     reference-repo selection for cross-repo includes is
                //     Task 309's job — `workspace_repos.position` 0 is the
                //     reference; this task keeps the existing per-repo
                //     call working).
                let project_root = repo_local.clone();
                let dest_root = repo_worktree.clone();
                let applied = tokio::task::spawn_blocking(move || {
                    files_to_copy::apply(&project_root, &dest_root)
                })
                .await
                .map_err(|e| Error::Internal(format!("files_to_copy join: {e}")));
                match applied {
                    Ok(Ok(applied_count)) => {
                        tracing::debug!(
                            repo = %repo.id,
                            applied = applied_count,
                            "files_to_copy applied"
                        );
                    }
                    Ok(Err(e)) | Err(e) => {
                        tracing::warn!(repo = %repo.id, error = %e, "files_to_copy failed; marking repo for partial");
                        failed_repo_ids.push(repo.id.0.clone());
                        first_err.get_or_insert(e);
                        let _ = remove_worktree_best_effort(&repo_local, &repo_worktree).await;
                        continue;
                    }
                }

                // Fully materialized.
                ok_repo_idx.push(idx);
                built.push((repo_local.clone(), repo_worktree.clone()));
            }

            // If NO repo materialized, there is nothing to keep — abort
            // and propagate the first error (single-repo failure, or every
            // repo of a multi-repo create failed). Matches 306's
            // whole-create abort for the only case where `partial` is
            // meaningless.
            if ok_repo_idx.is_empty() {
                cleanup_worktrees(&built, &worktree_root).await;
                return Err(first_err.unwrap_or_else(|| {
                    Error::Internal(
                        "create_workarea: no repos materialized and no error recorded".into(),
                    )
                }));
            }

            // ≥1 repo succeeded. If any failed, this is a `partial`
            // workarea (`design/03 §8`); otherwise `active`.
            let is_partial = !failed_repo_ids.is_empty();
            let final_status = if is_partial { "partial" } else { "active" };

            // 4. Persist row + one junction row per **materialized** repo +
            //    status transition in one tx.
            let id = WorkareaId(uuid::Uuid::now_v7().to_string());
            let worktree_root_str = worktree_root.to_string_lossy().into_owned();

            let mut writer = self.persistence.writer().await;
            let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;

            let new_workarea = NewWorkarea {
                id: id.clone(),
                workspace_id: workspace_id.to_string(),
                composer_name: composer.clone(),
                branch_name: branch.clone(),
                worktree_root: worktree_root_str.clone(),
                status: "created".to_string(),
                permission_mode: permission_mode.clone(),
                created_at: now_ms,
            };

            match concerto_persist::workareas::insert(&mut tx, new_workarea).await {
                Ok(_) => {
                    // One `workarea_repos` row per **materialized** repo (in
                    // position order). Failed repos get no junction row —
                    // their ids ride out on `WorkareaEvent::PartialCreate`
                    // so a retry can re-`worktree add` and insert the row.
                    for &idx in &ok_repo_idx {
                        let repo = &repos[idx];
                        let repo_worktree = worktree_root.join(&repo.name);
                        concerto_persist::workareas::insert_workarea_repo(
                            &mut tx,
                            NewWorkareaRepo {
                                workarea_id: id.clone(),
                                repository_id: repo.id.clone(),
                                worktree_path: repo_worktree.to_string_lossy().into_owned(),
                                branch_override: None,
                                // Task 302: seed the default-empty cone
                                // (`"[]"`) per repo. The three-layer
                                // inherited cone resolution + seeding is
                                // owned by 302/305; this task wires the
                                // per-repo loop.
                                sparse_cones_json: NewWorkareaRepo::empty_cones(),
                            },
                        )
                        .await?;
                    }
                    concerto_persist::workareas::update_status(&mut tx, &id, final_status).await?;
                    // Stamp `files_to_copy_applied: true` onto the
                    // workarea's `settings_json` so a future re-run of
                    // the resolver short-circuits idempotently
                    // (`tasks/30 §Scope — in` last bullet). The full
                    // settings_json schema is design/03 §3.14.
                    let settings_json = r#"{"files_to_copy_applied":true}"#.to_string();
                    concerto_persist::workareas::set_settings_json(&mut tx, &id, &settings_json)
                        .await?;
                    tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
                    drop(writer);
                    if is_partial {
                        tracing::info!(
                            audit.kind = "workarea_created_partial",
                            audit.scope = "workarea",
                            audit.workarea_id = %id,
                            audit.failed_repos = failed_repo_ids.join(","),
                            "workarea created partial (>=1 repo worktree-add failed)"
                        );
                        let _ = self.events.send(WorkareaEvent::PartialCreate {
                            id: id.clone(),
                            failed_repository_ids: failed_repo_ids.clone(),
                        });
                    }
                    break Workarea {
                        id,
                        workspace_id: ws_id.clone(),
                        composer_name: composer,
                        branch_name: branch,
                        worktree_root: worktree_root_str,
                        status: final_status.to_string(),
                        permission_mode: permission_mode.clone(),
                        created_at: now_ms,
                        archived_at: None,
                        last_activity_at: None,
                        settings_json,
                    };
                }
                Err(Error::Sqlx(boxed))
                    if concerto_persist::workareas::is_unique_violation(&boxed) =>
                {
                    // Roll back the DB tx, undo every worktree, and pick
                    // the next composer.
                    let _ = tx.rollback().await;
                    drop(writer);
                    cleanup_worktrees(&built, &worktree_root).await;
                    continue;
                }
                Err(other) => {
                    let _ = tx.rollback().await;
                    drop(writer);
                    cleanup_worktrees(&built, &worktree_root).await;
                    return Err(other);
                }
            }
        };

        let _ = self.events.send(WorkareaEvent::Created(workarea.clone()));
        Ok(workarea)
    }

    /// Look up a workarea by id.
    pub async fn get(&self, id: &WorkareaId) -> Result<Option<Workarea>> {
        concerto_persist::workareas::get(self.persistence.readers(), id).await
    }

    /// List the cached `pull_requests` rows for this workarea.
    ///
    /// Task 45/319: the workarea's PR set is the implicit set of rows
    /// keyed by `workarea_id` (`design/13 §4`), ordered
    /// `(merge_order, pr_number)` — the user-reorderable merge plan.
    pub async fn list_pr_set(
        &self,
        workarea_id: &WorkareaId,
    ) -> Result<Vec<concerto_persist::PullRequest>> {
        if self.get(workarea_id).await?.is_none() {
            return Err(Error::NotFound(format!("workarea {workarea_id} not found")));
        }
        concerto_persist::pull_requests::list_by_workarea(self.persistence.readers(), workarea_id)
            .await
    }

    /// Task 319: set the `merge_order` of the PR in `workarea_id` for
    /// `repository_id`, then return the re-ordered PR set. Validates that
    /// the workarea and a matching PR row both exist (`NotFound`
    /// otherwise). Mirrors the `Update*` "return the updated entity"
    /// convention so the Desktop drag UI (Task 324) re-renders from the
    /// authoritative order in one round-trip.
    pub async fn set_merge_order(
        &self,
        workarea_id: &WorkareaId,
        repository_id: &concerto_persist::RepositoryId,
        order: i64,
    ) -> Result<Vec<concerto_persist::PullRequest>> {
        if self.get(workarea_id).await?.is_none() {
            return Err(Error::NotFound(format!("workarea {workarea_id} not found")));
        }
        let pr_id = concerto_persist::pull_requests::id_by_workarea_repo(
            self.persistence.readers(),
            workarea_id,
            repository_id,
        )
        .await?
        .ok_or_else(|| {
            Error::NotFound(format!(
                "workarea {workarea_id} has no PR for repository {repository_id}"
            ))
        })?;

        {
            let mut writer = self.persistence.writer().await;
            concerto_persist::pull_requests::set_merge_order(&mut writer, &pr_id, order).await?;
        }

        concerto_persist::pull_requests::list_by_workarea(self.persistence.readers(), workarea_id)
            .await
    }

    // -----------------------------------------------------------------------
    // Task 320 — coordinated PR-set merge / revert (`design/03 §3.9`/§6.4)
    // -----------------------------------------------------------------------

    /// Task 320: the read-only coordinated-merge preview (`design/03 §5.1`). Load
    /// the workarea's PR set ordered by `merge_order` (Task 319's `list_pr_set`,
    /// already sorted `(merge_order, pr_number)`) and project each member to a
    /// [`MergeStep`]. Rejects a non-existent workarea with `NotFound`.
    pub async fn get_workarea_merge_plan(&self, workarea_id: &WorkareaId) -> Result<MergePlan> {
        // `list_pr_set` does the existence check (NotFound) + the ordered load.
        let rows = self.list_pr_set(workarea_id).await?;
        let total = rows.len() as i32;
        let steps = rows
            .into_iter()
            .enumerate()
            .map(|(i, pr)| MergeStep {
                step: (i as i32) + 1,
                total,
                repository_id: pr.repository_id.0,
                repository_full_name: pr.repository_full_name,
                pr_number: pr.pr_number,
                head_sha: pr.head_sha,
                merge_order: pr.merge_order,
                state: pr.state,
            })
            .collect();
        Ok(MergePlan {
            workarea_id: workarea_id.0.clone(),
            steps,
        })
    }

    /// Task 320: the coordinated PR-set merge loop (`design/13 §3.5`, `design/03
    /// §6.4`). For each member in `merge_order`: emit `StepStarted` → `merge_pr`
    /// (capturing the post-merge merge-commit SHA, `design/13 §7.2`) →
    /// `wait_for_check_runs(post-merge SHA, opts.timeout, all-terminal)` (Task
    /// 318). On `passed`, mark the cache row merged + emit `StepCompleted` +
    /// broadcast `PrSetMergeStepCompleted` and continue. On fail/timeout, emit
    /// `StepFailed` + `SetPaused` + broadcast `PrSetMergeFailedStep`, **pause**
    /// (stop the loop, return `paused_at_step`), and do NOT auto-revert (the
    /// caller/UI picks fix-resume or [`Self::revert_workarea_pr_set`]). When the
    /// whole set merges, emit `SetMerged` + broadcast `PrSetMerged`.
    ///
    /// `opts.allow_failing_checks` (the `design/03/13 R-6` merge-anyway override)
    /// is gated by `managed.json`'s `allowMergeWithFailingChecks`; when the policy
    /// forbids it the request is rejected with [`Error::PolicyLocked`]
    /// (`policy.locked` → `PERMISSION_DENIED`) BEFORE any merge. When permitted, a
    /// non-`passed` outcome is a typed warning + audit entry instead of a pause.
    ///
    /// `#[cfg(unix)]` — references `crate::scheduler::wait_for_check_runs`, which
    /// is unix-only (agent-host PTY; Windows scheduler is Task 702/Phase 7). The
    /// non-unix stub returns a typed "unsupported on this platform" error so the
    /// gRPC surface still compiles + degrades cleanly.
    #[cfg(unix)]
    pub async fn merge_workarea_pr_set(
        &self,
        workarea_id: &WorkareaId,
        opts: MergeOpts,
        progress: ProgressSink,
    ) -> Result<MergeReport> {
        use crate::scheduler::wait_checks::RequiredChecks;

        let vcs = self
            .pr_set_vcs
            .as_ref()
            .ok_or_else(|| Error::Vcs("vcs.not_configured: VCS handle not wired".into()))?;
        let scheduler = self.scheduler.as_ref().ok_or_else(|| {
            Error::Vcs("scheduler.not_configured: scheduler handle not wired".into())
        })?;

        // R-6: the merge-anyway override is gated by managed.json BEFORE any
        // merge runs (a locked policy must never let one PR slip through).
        if opts.allow_failing_checks {
            self.enforce_merge_anyway_allowed()?;
        }

        let plan = self.get_workarea_merge_plan(workarea_id).await?;
        let total = plan.steps.len() as i32;

        // Empty PR set → a 0-step success (no error, `design/13 §3.5`).
        if plan.steps.is_empty() {
            let _ = progress.send(MergeProgress::SetMerged { total: 0 }).await;
            let _ = self.events.send(WorkareaEvent::PrSetMerged {
                id: workarea_id.clone(),
                total: 0,
            });
            return Ok(MergeReport {
                merged_steps: 0,
                total: 0,
                paused_at_step: None,
            });
        }

        let mut merged_steps = 0i32;
        for member in &plan.steps {
            let repo_id = concerto_persist::RepositoryId(member.repository_id.clone());

            let _ = progress
                .send(MergeProgress::StepStarted {
                    step: member.step,
                    total,
                    repository_full_name: member.repository_full_name.clone(),
                    pr_number: member.pr_number,
                })
                .await;

            // 1. Merge the PR (capture the post-merge merge-commit SHA).
            let merge_report = match vcs
                .merge_pr(
                    &repo_id,
                    &member.repository_full_name,
                    member.pr_number,
                    opts.method,
                )
                .await
            {
                Ok(r) if r.merged => r,
                Ok(r) => {
                    // The provider reported `merged: false` (e.g. not mergeable /
                    // a 405 conflict the provider mapped to a non-merge). Pause.
                    let reason = if r.message.is_empty() {
                        "merge rejected by provider".to_string()
                    } else {
                        r.message
                    };
                    return self
                        .pause_merge(
                            workarea_id,
                            &progress,
                            member,
                            total,
                            merged_steps,
                            reason,
                            FailureKind::MergeRejected,
                        )
                        .await;
                }
                Err(e) => {
                    // A merge conflict / API error. Surface the message + stop
                    // (the user resolves in the workarea, `design/13 §8`).
                    let kind = if is_merge_conflict(&e) {
                        FailureKind::MergeConflict
                    } else {
                        FailureKind::MergeRejected
                    };
                    return self
                        .pause_merge(
                            workarea_id,
                            &progress,
                            member,
                            total,
                            merged_steps,
                            e.to_string(),
                            kind,
                        )
                        .await;
                }
            };

            // The post-merge merge-commit SHA (`design/13 §7.2`). When the
            // provider returns none, fall back to the PR head SHA so the gate
            // still has something to poll (degraded, but never panics).
            let merge_sha = merge_report
                .merge_commit_sha
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| member.head_sha.clone());

            // 2. Wait for the merge commit's checks to resolve (Task 318).
            let outcome = scheduler
                .wait_for_check_runs(
                    repo_id.clone(),
                    &merge_sha,
                    opts.timeout,
                    RequiredChecks::AllTerminal,
                )
                .await?;

            if !outcome.passed {
                if opts.allow_failing_checks {
                    // R-6: merge-anyway override is on + policy-permitted. Treat
                    // the red/timeout outcome as a typed WARNING + audit entry
                    // and continue rather than pause.
                    tracing::warn!(
                        audit.kind = "pr_set_merge_with_failing_checks",
                        audit.scope = "workarea",
                        audit.workarea_id = %workarea_id,
                        audit.repository = %member.repository_full_name,
                        audit.pr_number = member.pr_number,
                        audit.merge_sha = %merge_sha,
                        audit.timed_out = outcome.timed_out,
                        "coordinated merge: checks not green but allowMergeWithFailingChecks override active; continuing"
                    );
                } else {
                    // Pause-on-fail: stop the loop, surface "Step N of M failed".
                    let (reason, kind) = if outcome.timed_out {
                        (
                            format!("checks timed out after {:?}", opts.timeout),
                            FailureKind::ChecksTimeout,
                        )
                    } else {
                        (
                            "required checks failed".to_string(),
                            FailureKind::ChecksFailed,
                        )
                    };
                    return self
                        .pause_merge(
                            workarea_id,
                            &progress,
                            member,
                            total,
                            merged_steps,
                            reason,
                            kind,
                        )
                        .await;
                }
            }

            // 3. Step passed (or was overridden). Mark the cache row merged so a
            //    later coordinated revert knows this member is revertible.
            self.mark_pr_merged(&repo_id, member.pr_number).await;
            merged_steps += 1;

            let _ = progress
                .send(MergeProgress::StepCompleted {
                    step: member.step,
                    total,
                    merge_sha: merge_sha.clone(),
                })
                .await;
            let _ = self.events.send(WorkareaEvent::PrSetMergeStepCompleted {
                id: workarea_id.clone(),
                step: member.step,
                total,
                repository_full_name: member.repository_full_name.clone(),
                pr_number: member.pr_number,
                merge_sha,
            });
        }

        // Every member merged.
        let _ = progress.send(MergeProgress::SetMerged { total }).await;
        let _ = self.events.send(WorkareaEvent::PrSetMerged {
            id: workarea_id.clone(),
            total,
        });
        tracing::info!(
            audit.kind = "pr_set_merged",
            audit.scope = "workarea",
            audit.workarea_id = %workarea_id,
            audit.total = total,
            "coordinated PR-set merge complete"
        );
        Ok(MergeReport {
            merged_steps,
            total,
            paused_at_step: None,
        })
    }

    /// Non-unix stub for [`Self::merge_workarea_pr_set`]: the Scheduler's
    /// `wait_for_check_runs` is unix-only, so the coordinated merge is
    /// unsupported on Windows in V1.0 (Task 702/Phase 7). Returns a typed error
    /// (NOT a panic / `unimplemented!()`).
    #[cfg(not(unix))]
    pub async fn merge_workarea_pr_set(
        &self,
        _workarea_id: &WorkareaId,
        _opts: MergeOpts,
        _progress: ProgressSink,
    ) -> Result<MergeReport> {
        Err(Error::Vcs(
            "unimplemented: coordinated PR-set merge is unsupported on this platform (Windows scheduler is Task 702/Phase 7)"
                .into(),
        ))
    }

    /// Task 320: the coordinated revert (`design/13 §3.5`, R-5). Walk the workarea's
    /// merged members in REVERSE `merge_order` and `revert_pr` each (revert-commit
    /// by default; `opts.hard_reset` opt-in). Un-merged members record `Skipped`.
    /// A per-member revert failure does NOT abort the rest — it records `Failed`
    /// and continues. Emits `PrReverted` per reverted member. Cross-platform (no
    /// Scheduler dependency — revert does not wait for checks).
    pub async fn revert_workarea_pr_set(
        &self,
        workarea_id: &WorkareaId,
        opts: RevertOpts,
    ) -> Result<RevertReport> {
        let vcs = self
            .pr_set_vcs
            .as_ref()
            .ok_or_else(|| Error::Vcs("vcs.not_configured: VCS handle not wired".into()))?;

        // `list_pr_set` does the existence check + the ordered load
        // (`(merge_order, pr_number)`); reverse it for revert.
        let mut rows = self.list_pr_set(workarea_id).await?;
        rows.reverse();

        let mut steps = Vec::with_capacity(rows.len());
        for pr in rows {
            let repo_id = pr.repository_id.clone();
            // Only members that actually merged are revertible.
            if pr.state != "merged" {
                steps.push(RevertStep {
                    repository_full_name: pr.repository_full_name.clone(),
                    pr_number: pr.pr_number,
                    outcome: RevertOutcome::Skipped,
                    detail: format!("not merged (state={})", pr.state),
                });
                continue;
            }
            match vcs
                .revert_pr(
                    &repo_id,
                    &pr.repository_full_name,
                    pr.pr_number,
                    opts.hard_reset,
                )
                .await
            {
                Ok(report) if report.reverted => {
                    self.mark_pr_reverted(&repo_id, pr.pr_number).await;
                    let _ = self.events.send(WorkareaEvent::PrReverted {
                        id: workarea_id.clone(),
                        repository_full_name: pr.repository_full_name.clone(),
                        pr_number: pr.pr_number,
                    });
                    tracing::info!(
                        audit.kind = "pr_reverted",
                        audit.scope = "workarea",
                        audit.workarea_id = %workarea_id,
                        audit.repository = %pr.repository_full_name,
                        audit.pr_number = pr.pr_number,
                        audit.hard_reset = opts.hard_reset,
                        "coordinated revert: member reverted"
                    );
                    steps.push(RevertStep {
                        repository_full_name: pr.repository_full_name.clone(),
                        pr_number: pr.pr_number,
                        outcome: RevertOutcome::Reverted,
                        detail: report.revert_pr_url.unwrap_or(report.message),
                    });
                }
                Ok(report) => steps.push(RevertStep {
                    repository_full_name: pr.repository_full_name.clone(),
                    pr_number: pr.pr_number,
                    outcome: RevertOutcome::Failed,
                    detail: if report.message.is_empty() {
                        "provider reported revert not applied".to_string()
                    } else {
                        report.message
                    },
                }),
                Err(e) => steps.push(RevertStep {
                    repository_full_name: pr.repository_full_name.clone(),
                    pr_number: pr.pr_number,
                    outcome: RevertOutcome::Failed,
                    detail: e.to_string(),
                }),
            }
        }

        Ok(RevertReport {
            workarea_id: workarea_id.0.clone(),
            steps,
        })
    }

    /// Emit the pause frames (`StepFailed` + `SetPaused`) + the
    /// `PrSetMergeFailedStep` broadcast, then return the paused [`MergeReport`].
    /// Shared exit for every pause-on-fail branch of the merge loop.
    #[cfg(unix)]
    #[allow(clippy::too_many_arguments)]
    async fn pause_merge(
        &self,
        workarea_id: &WorkareaId,
        progress: &ProgressSink,
        member: &MergeStep,
        total: i32,
        merged_steps: i32,
        reason: String,
        kind: FailureKind,
    ) -> Result<MergeReport> {
        let _ = progress
            .send(MergeProgress::StepFailed {
                step: member.step,
                total,
                reason: reason.clone(),
                kind,
            })
            .await;
        let _ = progress
            .send(MergeProgress::SetPaused {
                paused_at_step: member.step,
                total,
                reason: reason.clone(),
            })
            .await;
        let _ = self.events.send(WorkareaEvent::PrSetMergeFailedStep {
            id: workarea_id.clone(),
            step: member.step,
            total,
            reason: reason.clone(),
        });
        tracing::warn!(
            audit.kind = "pr_set_merge_paused",
            audit.scope = "workarea",
            audit.workarea_id = %workarea_id,
            audit.paused_at_step = member.step,
            audit.total = total,
            audit.reason = %reason,
            "coordinated PR-set merge paused at failed step"
        );
        Ok(MergeReport {
            merged_steps,
            total,
            paused_at_step: Some(member.step),
        })
    }

    /// R-6: reject `allow_failing_checks=true` when `managed.json` forbids it.
    /// The key is `allowMergeWithFailingChecks` (camelCase, `D9`); it is a NEW
    /// managed key beyond Task 211's frozen set, so it is read locally here (NOT
    /// added to 211's parser — see the Handoff forward-note) and defaults to
    /// `false` (security-conservative: the org must explicitly opt in). A locked
    /// policy returns [`Error::PolicyLocked`] (`policy.locked` →
    /// `PERMISSION_DENIED`), mirroring Task 32's bypass-guard rejection.
    // Only the `#[cfg(unix)]` coordinated-merge loop calls this (the Scheduler /
    // `wait_for_check_runs` is unix-only). Cross-platform body, so keep it compiled
    // everywhere but silence dead_code on non-unix where the caller is absent.
    #[cfg_attr(not(unix), allow(dead_code))]
    fn enforce_merge_anyway_allowed(&self) -> Result<()> {
        if !read_allow_merge_with_failing_checks(&self.config_dir) {
            return Err(Error::PolicyLocked(format!(
                "{}: managed.json forbids merge-with-failing-checks (allowMergeWithFailingChecks)",
                crate::security::POLICY_LOCKED_GENERIC
            )));
        }
        Ok(())
    }

    /// Update one cached `pull_requests` row's `state` to `merged` (best-effort;
    /// a failure is logged, not fatal — the GitHub merge already happened). Lets a
    /// later coordinated revert find revertible members.
    #[cfg_attr(not(unix), allow(dead_code))]
    async fn mark_pr_merged(&self, repo: &concerto_persist::RepositoryId, pr_number: i64) {
        self.set_pr_state(repo, pr_number, "merged").await;
    }

    /// Update one cached `pull_requests` row's `state` to `reverted` (best-effort).
    async fn mark_pr_reverted(&self, repo: &concerto_persist::RepositoryId, pr_number: i64) {
        self.set_pr_state(repo, pr_number, "reverted").await;
    }

    async fn set_pr_state(
        &self,
        repo: &concerto_persist::RepositoryId,
        pr_number: i64,
        state: &str,
    ) {
        let now_ms = now_unix_ms();
        let mut writer = self.persistence.writer().await;
        if let Err(e) = sqlx::query(
            "UPDATE pull_requests SET state = ?, updated_at = ? WHERE repository_id = ? AND pr_number = ?",
        )
        .bind(state)
        .bind(now_ms)
        .bind(&repo.0)
        .bind(pr_number)
        .execute(&mut *writer)
        .await
        {
            tracing::warn!(
                repo = %repo,
                pr_number,
                state,
                error = %e,
                "failed to update cached pull_requests.state after coordinated merge/revert"
            );
        }
    }

    /// List workareas in a workspace.
    pub async fn list_by_workspace(
        &self,
        workspace_id: &WorkspaceId,
        include_archived: bool,
    ) -> Result<Vec<Workarea>> {
        concerto_persist::workareas::list_by_workspace(
            self.persistence.readers(),
            workspace_id,
            include_archived,
        )
        .await
    }

    /// Resolve the worktree path on disk for one repository inside a
    /// workarea, then run `concerto_gix_wrap::diff_head` against it.
    ///
    /// Task 29's hot-path entry point for `Workareas.GetWorkareaRepoDiff`.
    /// The lookup is a single read against `workarea_repos`; the diff
    /// itself shells out to `git` and is offloaded via
    /// `tokio::task::spawn_blocking` so the gRPC reactor stays unblocked
    /// even on slow disks.
    pub async fn get_repo_diff(
        &self,
        workarea_id: &WorkareaId,
        repository_id: &concerto_persist::RepositoryId,
    ) -> Result<concerto_gix_wrap::DiffPayload> {
        // Workarea must exist before we go looking for its repos.
        if self.get(workarea_id).await?.is_none() {
            return Err(Error::NotFound(format!("workarea {workarea_id} not found")));
        }
        let worktree_path = concerto_persist::workareas::get_workarea_repo_worktree_path(
            self.persistence.readers(),
            workarea_id,
            repository_id,
        )
        .await?
        .ok_or_else(|| {
            Error::NotFound(format!(
                "repository {repository_id} is not attached to workarea {workarea_id}"
            ))
        })?;
        let worktree = PathBuf::from(worktree_path);
        // `diff_head` is `async` but drives a subprocess; running it
        // directly off the gRPC reactor is fine — the blocking work is
        // already on a tokio child process, not on this thread.
        concerto_gix_wrap::diff_head(&worktree).await
    }

    /// Resolve the worktree path for one repository inside a workarea, then
    /// run the FROZEN `concerto_gix_wrap::status` shell-out seam against it
    /// (Task 303, `design/02 §7.2`, `design/00 §7.7`).
    ///
    /// This is the status-read path the hot-path bench gate
    /// (`crates/gix-wrap/benches/status_sparse_gate.rs`) measures: the
    /// resolved `workarea_repos.worktree_path` is the **per-(workarea,
    /// repo) sparse-cone worktree** Task 302 materializes, so `status` only
    /// pays for the cone (the `--sparse-index` lever keeps the in-memory
    /// index proportional to the cone, not the whole monorepo — spike 104
    /// §4a). The seam itself stays untouched (Task 29 FROZEN); only this
    /// caller wiring routes it through the sparse cone.
    ///
    /// Sibling of [`Self::get_repo_diff`]: a single read against
    /// `workarea_repos`, then the shell-out (already driven by
    /// `tokio::process`, so it does not block the gRPC reactor). Spike 104
    /// returned GO for the shell-out path; **no gix-native rewrite** (the
    /// `gix` `status` feature stays off).
    pub async fn get_repo_status(
        &self,
        workarea_id: &WorkareaId,
        repository_id: &concerto_persist::RepositoryId,
    ) -> Result<concerto_gix_wrap::StatusReport> {
        if self.get(workarea_id).await?.is_none() {
            return Err(Error::NotFound(format!("workarea {workarea_id} not found")));
        }
        let worktree_path = concerto_persist::workareas::get_workarea_repo_worktree_path(
            self.persistence.readers(),
            workarea_id,
            repository_id,
        )
        .await?
        .ok_or_else(|| {
            Error::NotFound(format!(
                "repository {repository_id} is not attached to workarea {workarea_id}"
            ))
        })?;
        let worktree = PathBuf::from(worktree_path);
        concerto_gix_wrap::status(&worktree).await
    }

    /// Archive a workarea. Sets `archived_at` and transitions `status`
    /// to `"archived"`. Idempotent.
    ///
    /// Equivalent to `archive_workarea(id, ArchiveOpts::default())` —
    /// the worktree is kept on disk per `design/03` R-5 (fast restore).
    /// Kept as a thin wrapper for Task 20 call sites; new code should
    /// prefer [`Self::archive_workarea`] which exposes the
    /// `remove_worktree` knob.
    pub async fn archive(&self, id: &WorkareaId) -> Result<()> {
        self.archive_workarea(id, ArchiveOpts::default()).await
    }

    /// Archive a workarea with [`ArchiveOpts`] (Task 31).
    ///
    /// Steps (per `design/03 §3.7`):
    /// 1. Resolve the workarea row (404 if unknown).
    /// 2. Ask the Agent Supervisor to `stop_session(sid, "archive")` for
    ///    every session whose `ended_at IS NULL`. Errors logged
    ///    best-effort; the workarea archive proceeds either way (the DB
    ///    archive is the source of truth, the supervisor's in-memory
    ///    state is a fast-path cache).
    /// 3. If `opts.remove_worktree`, shell out to
    ///    `git worktree remove --force` for each repo and remove the
    ///    workarea root directory.
    /// 4. In one writer transaction: set `archived_at = now` AND
    ///    `status = 'archived'`.
    /// 5. Emit [`WorkareaEvent::Archived`].
    ///
    /// Idempotent: archiving an already-archived workarea re-stamps the
    /// timestamp and re-emits the event.
    pub async fn archive_workarea(&self, id: &WorkareaId, opts: ArchiveOpts) -> Result<()> {
        let workarea = self
            .get(id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("workarea {id} not found")))?;

        // 1. Stop every live session for this workarea. Best-effort —
        // the supervisor's in-memory state is a fast-path; the DB row's
        // `ended_at` is what callers should inspect.
        self.stop_live_sessions(id).await;

        // 2. Optional disk reclaim.
        if opts.remove_worktree {
            let worktree_root = PathBuf::from(&workarea.worktree_root);
            remove_worktrees_and_root(&self.persistence, id, &worktree_root).await?;
        }

        // 3. One-tx archive of the workarea row.
        let now_ms = now_unix_ms();
        let mut writer = self.persistence.writer().await;
        let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        concerto_persist::workareas::archive(&mut tx, id, now_ms).await?;
        tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        drop(writer);

        let _ = self.events.send(WorkareaEvent::Archived(id.clone()));
        Ok(())
    }

    /// Restore an archived workarea (Task 31).
    ///
    /// Steps (per `design/03 §3.7`):
    /// 1. Resolve the workarea row (404 if unknown).
    /// 2. If the worktree directory is gone from disk, re-run
    ///    `git worktree add` using the stored `branch_name`.
    /// 3. In one writer transaction: clear `archived_at`, reset
    ///    `permission_mode = NULL` (security stance — restored workareas
    ///    inherit the workspace default rather than silently resuming
    ///    elevated modes), set `status = 'active'`.
    /// 4. Emit [`WorkareaEvent::Restored`].
    pub async fn restore_workarea(&self, id: &WorkareaId) -> Result<Workarea> {
        let workarea = self
            .get(id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("workarea {id} not found")))?;

        // Re-create worktree if missing.
        let worktree_root = PathBuf::from(&workarea.worktree_root);
        recreate_worktrees(&self.persistence, id, &worktree_root, &workarea.branch_name).await?;

        // Clear archived_at + reset permission_mode + status='active'.
        let mut writer = self.persistence.writer().await;
        let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        concerto_persist::workareas::restore(&mut tx, id).await?;
        tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        drop(writer);

        // Re-read so the event payload reflects the post-restore row.
        let restored = self
            .get(id)
            .await?
            .ok_or_else(|| Error::Internal(format!("workarea {id} vanished mid-restore")))?;

        let _ = self.events.send(WorkareaEvent::Restored(restored.clone()));
        Ok(restored)
    }

    /// Task 307: the single funnel for every workarea status change.
    ///
    /// Loads the current `status`, parses it to a [`WorkareaState`], applies
    /// the pure [`fsm::transition`] graph, persists the new status via
    /// [`workareas::update_status`], broadcasts
    /// [`WorkareaEvent::StatusChanged`], and emits a `tracing::info!` audit
    /// line. Returns the post-transition [`Workarea`] row.
    ///
    /// ## Error mapping
    ///
    /// An illegal `(state, event)` pair is **never** a panic: the FSM's
    /// `Err(Validation(INVALID_TRANSITION_WIRE_CODE …))` is re-wrapped as a
    /// typed [`Error::Policy`] (→ `Code::FailedPrecondition` over gRPC) that
    /// preserves the wire-code prefix, and logged at debug. The
    /// union-of-sessions derivation (`design/03 §3.1`) can re-apply the same
    /// event, so idempotent no-op re-applies (e.g. a stray `SessionStarted`
    /// on a `running` workarea) are expected and rejected softly, not fatal.
    ///
    /// A no-op transition where the FSM maps the state to itself (none today
    /// except `Archived + Archive`) still writes + broadcasts so subscribers
    /// see a consistent stream; the audit line records `from == to`.
    pub async fn transition_workarea(
        &self,
        id: &WorkareaId,
        event: crate::workspace_manager::fsm::WorkareaEvent,
    ) -> Result<Workarea> {
        use crate::workspace_manager::fsm;

        let workarea = self
            .get(id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("workarea {id} not found")))?;

        let from = fsm::WorkareaState::from_sql(&workarea.status).ok_or_else(|| {
            Error::Internal(format!(
                "workarea {id} has an unrecognized status {:?} in the DB",
                workarea.status
            ))
        })?;

        let to = match fsm::transition(from, event) {
            Ok(to) => to,
            Err(Error::Validation(msg)) if msg.starts_with(fsm::INVALID_TRANSITION_WIRE_CODE) => {
                tracing::debug!(
                    workarea = %id,
                    from = %from.as_sql(),
                    ?event,
                    "rejected illegal workarea transition (soft)"
                );
                // Preserve the wire-code prefix but surface as a typed
                // FAILED_PRECONDITION rather than InvalidArgument: this is a
                // state-machine precondition, not a malformed argument.
                return Err(Error::Policy(msg));
            }
            Err(e) => return Err(e),
        };

        let from_sql = from.as_sql().to_string();
        let to_sql = to.as_sql().to_string();

        let mut writer = self.persistence.writer().await;
        let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        concerto_persist::workareas::update_status(&mut tx, id, &to_sql).await?;
        tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        drop(writer);

        tracing::info!(
            audit.kind = "workarea_status_changed",
            audit.scope = "workarea",
            audit.workarea_id = %id,
            audit.from = %from_sql,
            audit.to = %to_sql,
            audit.event = ?event,
            "workarea status transition"
        );

        let _ = self.events.send(WorkareaEvent::StatusChanged {
            id: id.clone(),
            from: from_sql,
            to: to_sql,
        });

        self.get(id)
            .await?
            .ok_or_else(|| Error::Internal(format!("workarea {id} vanished mid-transition")))
    }

    /// Task 307: derive + apply a workarea transition from one Agent
    /// Supervisor [`AgentEvent`] (`design/03 §3.1`: a workarea's effective
    /// status is the union of its sessions' states).
    ///
    /// Maps the supervisor's event to the FSM event and funnels it through
    /// [`Self::transition_workarea`]. `SessionFinished` only fires when **no
    /// other** session on the workarea is still live (`sessions WHERE
    /// workarea_id=? AND ended_at IS NULL` is empty) — with multi-session
    /// (Task 308) this prevents a finishing session from prematurely marking
    /// a workarea `finished` while a sibling session keeps running. Events
    /// the FSM has no edge for from the current state are swallowed at debug
    /// (the soft-reject path), so this is safe to call on every event.
    #[cfg(unix)]
    pub async fn apply_session_event(
        &self,
        workarea_id: &WorkareaId,
        event: &crate::agent_supervisor::AgentEvent,
    ) {
        use crate::agent_supervisor::AgentEvent;
        use crate::workspace_manager::fsm::WorkareaEvent as Fsm;

        let fsm_event = match event {
            AgentEvent::Started { .. } => Fsm::SessionStarted,
            AgentEvent::AwaitingApproval { .. } => Fsm::SessionAwaiting,
            AgentEvent::ApprovalResolved { .. } => Fsm::SessionResumed,
            AgentEvent::Crashed { .. } => Fsm::SessionCrashed,
            AgentEvent::Exited { .. } => {
                // Union-of-sessions: only transition to `finished` once no
                // session on this workarea is still live. The supervisor has
                // already stamped this session's `ended_at` before emitting
                // `Exited`, so an empty live set means the last one ended.
                match concerto_persist::sessions::list_live_ids_by_workarea(
                    self.persistence.readers(),
                    workarea_id,
                )
                .await
                {
                    Ok(live) if live.is_empty() => Fsm::SessionFinished,
                    Ok(_) => return,
                    Err(e) => {
                        tracing::warn!(
                            workarea = %workarea_id,
                            error = %e,
                            "apply_session_event: live-session probe failed; skipping finish"
                        );
                        return;
                    }
                }
            }
            // Message / ToolCall / ToolResult / TurnComplete / ContextUsage /
            // CheckpointCreated do not drive the workarea FSM.
            _ => return,
        };

        match self.transition_workarea(workarea_id, fsm_event).await {
            Ok(_) => {}
            // Soft-rejects (FAILED_PRECONDITION) are expected: the same event
            // may re-apply, or arrive in a state with no edge. Log at debug.
            Err(Error::Policy(_)) => {}
            Err(e) => tracing::warn!(
                workarea = %workarea_id,
                error = %e,
                "apply_session_event: transition failed"
            ),
        }
    }

    /// Task 307: spawn the background pump that drives the workarea FSM
    /// from Agent Supervisor session events.
    ///
    /// Mirrors the Suggestion Engine's `spawn_session_pump`
    /// ([`crate::suggestions`]): a 1 s poll over live sessions
    /// (`sessions WHERE ended_at IS NULL`) subscribes — once each — to every
    /// session's [`AgentEvent`] broadcast (with replay so a fast-finishing
    /// echo session's burst is not missed) and forwards each event into
    /// [`Self::apply_session_event`]. After a session ends, a final poll
    /// still observes the row until `ended_at` is set; the per-session pump
    /// task exits when the broadcast channel closes.
    ///
    /// Run from `boot.rs` once the supervisor handle exists; cancelled when
    /// the root `shutdown` token fires.
    #[cfg(unix)]
    pub fn spawn_session_fsm_pump(
        &self,
        supervisor: AgentSupervisorHandle,
        shutdown: tokio_util::sync::CancellationToken,
    ) {
        let handle = self.clone();
        tokio::spawn(async move {
            use concerto_persist::SessionId;
            use std::collections::HashSet;

            let mut attached: HashSet<SessionId> = HashSet::new();
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = tick.tick() => {
                        let live = match list_all_live_sessions(&handle.persistence).await {
                            Ok(rows) => rows,
                            Err(e) => {
                                tracing::debug!(error = %e, "workarea.fsm_pump: list failed");
                                continue;
                            }
                        };
                        for (workarea_id, session_id) in live {
                            if !attached.insert(session_id.clone()) {
                                continue;
                            }
                            let Some((replay, mut rx)) =
                                supervisor.subscribe_events_with_replay(&session_id).await
                            else {
                                // Session vanished between poll + subscribe;
                                // forget it so a later tick can retry.
                                attached.remove(&session_id);
                                continue;
                            };
                            // Replay the buffered burst first so a session
                            // that started + finished between two ticks still
                            // drives `running` → `finished`.
                            for ev in replay {
                                handle.apply_session_event(&workarea_id, &ev).await;
                            }
                            let pump = handle.clone();
                            let wid = workarea_id.clone();
                            tokio::spawn(async move {
                                while let Ok(ev) = rx.recv().await {
                                    pump.apply_session_event(&wid, &ev).await;
                                }
                            });
                        }
                    }
                }
            }
            tracing::debug!("workarea.fsm_pump exited");
        });
    }

    /// Task 307: hard-pause a workarea (`design/03 R-9`). Stops every live
    /// session via the Agent Supervisor, then funnels `Pause` through
    /// [`Self::transition_workarea`] → `paused`.
    pub async fn pause_workarea(&self, id: &WorkareaId) -> Result<Workarea> {
        // Ensure the workarea exists before stopping sessions (404 maps
        // cleanly; stop_live_sessions is a best-effort no-op otherwise).
        if self.get(id).await?.is_none() {
            return Err(Error::NotFound(format!("workarea {id} not found")));
        }
        self.stop_live_sessions(id).await;
        self.transition_workarea(id, crate::workspace_manager::fsm::WorkareaEvent::Pause)
            .await
    }

    /// Task 307: resume a paused workarea (`design/03 R-9`). Funnels
    /// `Resume` through [`Self::transition_workarea`] → `active`. Cold-resume
    /// of the actual sessions is the user's next action (start/restart), not
    /// automatic.
    pub async fn resume_workarea(&self, id: &WorkareaId) -> Result<Workarea> {
        self.transition_workarea(id, crate::workspace_manager::fsm::WorkareaEvent::Resume)
            .await
    }

    /// Best-effort: ask the Agent Supervisor to stop every session whose
    /// `ended_at IS NULL` for `workarea_id`. Errors are logged and
    /// swallowed — the archive cascade owns the eventual DB state via
    /// `archive_workarea`, the supervisor's in-memory map is a fast-path
    /// cache.
    async fn stop_live_sessions(&self, workarea_id: &WorkareaId) {
        #[cfg(unix)]
        {
            let Some(sup) = self.agent_supervisor.as_ref() else {
                return;
            };
            let live = match concerto_persist::sessions::list_live_ids_by_workarea(
                self.persistence.readers(),
                workarea_id,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        workarea = %workarea_id,
                        error = %e,
                        "failed to list live sessions during archive"
                    );
                    return;
                }
            };
            for sid in live {
                if let Err(e) = sup.stop_session(&sid, Some("archive".to_string())).await {
                    tracing::warn!(
                        session = %sid,
                        workarea = %workarea_id,
                        error = %e,
                        "stop_session failed during archive; continuing"
                    );
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = workarea_id;
        }
    }

    /// Task 32: change `workareas.permission_mode`.
    ///
    /// `mode = Some(m)` writes the SQL string; `None` clears the column
    /// (inherit-from-workspace). When the requested mode is `yolo`,
    /// `acknowledgement` MUST equal [`crate::security::ACK_YOLO`]
    /// (literal `"I understand"`) — otherwise the server rejects with
    /// `policy:` (FAILED_PRECONDITION). The managed-policy cap is
    /// enforced after parsing: requesting `yolo` when `managed.json`
    /// caps to `auto` returns `policy.locked` (PERMISSION_DENIED).
    ///
    /// Emits a `tracing::info!` audit event with
    /// `audit.kind = "permission_mode_changed"`, `audit.scope =
    /// "workarea"`, and the from→to transition.
    pub async fn update_workarea_permission_mode(
        &self,
        id: &WorkareaId,
        mode: Option<&str>,
        acknowledgement: &str,
    ) -> Result<Workarea> {
        let existing = self
            .get(id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("workarea {id} not found")))?;

        let mode_str: Option<String> = match mode {
            Some(s) => {
                let parsed = crate::security::parse_permission_mode(s)?;
                if parsed == crate::security::PermissionMode::Yolo
                    && !crate::security::ack_for_yolo(acknowledgement)
                {
                    return Err(Error::Policy(format!(
                        "policy.acknowledgement_required: setting permission_mode={} requires acknowledgement={:?}",
                        parsed.as_str(),
                        crate::security::ACK_YOLO
                    )));
                }
                let managed = crate::security::load_managed_policy(&self.config_dir)?;
                let _capped = crate::security::permission::enforce_managed_cap(parsed, &managed)?;
                Some(parsed.as_str().to_string())
            }
            None => None,
        };

        let mut writer = self.persistence.writer().await;
        let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        concerto_persist::workareas::set_permission_mode(&mut tx, id, mode_str.as_deref()).await?;
        tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        drop(writer);

        tracing::info!(
            audit.kind = "permission_mode_changed",
            audit.scope = "workarea",
            audit.workarea_id = %id,
            audit.from = %existing.permission_mode.as_deref().unwrap_or("inherit"),
            audit.to = %mode_str.as_deref().unwrap_or("inherit"),
            audit.acknowledgement_provided = !acknowledgement.is_empty(),
            "workarea permission_mode changed"
        );

        self.get(id)
            .await?
            .ok_or_else(|| Error::Internal(format!("workarea {id} vanished mid-update")))
    }

    /// Task 32: toggle `workareas.bypass_destructive_guard`.
    ///
    /// When `enable = true`, `acknowledgement` MUST equal
    /// [`crate::security::ACK_BYPASS_DESTRUCTIVE_GUARD`] (literal
    /// `"I understand the risks"`); otherwise rejected with `policy:`
    /// (FAILED_PRECONDITION). Disabling does not require an
    /// acknowledgement. The managed-policy `allow_bypass_destructive_guard`
    /// flag is enforced: when `false`, `enable = true` returns
    /// `policy.locked` (PERMISSION_DENIED).
    pub async fn set_workarea_bypass_destructive_guard(
        &self,
        id: &WorkareaId,
        enable: bool,
        acknowledgement: &str,
    ) -> Result<Workarea> {
        let _existing = self
            .get(id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("workarea {id} not found")))?;

        if enable && !crate::security::ack_for_bypass_destructive_guard(acknowledgement) {
            return Err(Error::Policy(format!(
                "policy.acknowledgement_required: setting bypass_destructive_guard=true requires acknowledgement={:?}",
                crate::security::ACK_BYPASS_DESTRUCTIVE_GUARD
            )));
        }
        let managed = crate::security::load_managed_policy(&self.config_dir)?;
        crate::security::permission::enforce_managed_bypass(enable, &managed)?;

        let mut writer = self.persistence.writer().await;
        let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        concerto_persist::workareas::set_bypass_destructive_guard(&mut tx, id, Some(enable))
            .await?;
        tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        drop(writer);

        tracing::info!(
            audit.kind = "bypass_destructive_guard_changed",
            audit.scope = "workarea",
            audit.workarea_id = %id,
            audit.to = enable,
            audit.acknowledgement_provided = !acknowledgement.is_empty(),
            "workarea bypass_destructive_guard changed"
        );

        self.get(id)
            .await?
            .ok_or_else(|| Error::Internal(format!("workarea {id} vanished mid-update")))
    }

    /// Internal hook used by [`super::WorkspaceManager::archive`]:
    /// stop the live sessions + (optionally) tear down the worktree for
    /// one workarea, WITHOUT touching the workarea's DB row. The caller
    /// stamps `archived_at` for the whole batch in a single transaction.
    ///
    /// Skips the DB UPDATE so the cascade stays atomic at the
    /// workspace-archive level.
    pub(crate) async fn archive_workarea_side_effects(
        &self,
        id: &WorkareaId,
        worktree_root: &Path,
        opts: ArchiveOpts,
    ) -> Result<()> {
        self.stop_live_sessions(id).await;
        if opts.remove_worktree {
            remove_worktrees_and_root(&self.persistence, id, worktree_root).await?;
        }
        Ok(())
    }

    /// Republish a [`WorkareaEvent::Archived`] from the
    /// `archive_workspace` cascade. The DB UPDATE happens inside the
    /// workspace-level transaction; this method lets the WorkspaceManager
    /// fan-out the event after commit so subscribers see the same shape
    /// as a single-workarea archive.
    ///
    /// Returns the number of receivers that observed the event, mirroring
    /// `broadcast::Sender::send` semantics. Errors are swallowed (a
    /// closed channel is not a workspace-archive failure).
    pub(crate) fn publish_archived(&self, id: WorkareaId) -> usize {
        self.events.send(WorkareaEvent::Archived(id)).unwrap_or(0)
    }
}

/// Pick the lowest-index composer name not in `used`.
///
/// Falls back to `<composer>-N` (starting at `N = 2`) once the pool is
/// exhausted. Returns `None` only if the suffix space (`MAX_COMPOSER_ATTEMPTS`
/// rounds per composer) is also full — effectively never.
fn allocate_composer(used: &std::collections::HashSet<String>) -> Option<String> {
    // First pass: bare names.
    if let Some(name) = COMPOSERS.iter().find(|n| !used.contains(**n)) {
        return Some((*name).to_string());
    }
    // Overflow: <composer>-N. Try suffixes 2..=99 round-robin over the
    // pool; the bound matches `MAX_COMPOSER_ATTEMPTS` so the manager's
    // outer cap stays the authoritative limit.
    for suffix in 2..=MAX_COMPOSER_ATTEMPTS {
        for base in COMPOSERS.iter() {
            let candidate = format!("{base}-{suffix}");
            if !used.contains(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Every live session as `(workarea_id, session_id)` (`ended_at IS NULL`).
/// Used by the Task-307 FSM pump to find sessions to subscribe to. Reads
/// the read-only pool so it does not contend with writers.
#[cfg(unix)]
async fn list_all_live_sessions(
    persistence: &Arc<Persistence>,
) -> Result<Vec<(WorkareaId, concerto_persist::SessionId)>> {
    use sqlx::Row as _;
    let rows = sqlx::query("SELECT id, workarea_id FROM sessions WHERE ended_at IS NULL")
        .fetch_all(persistence.readers())
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                WorkareaId(r.get::<String, _>("workarea_id")),
                concerto_persist::SessionId(r.get::<String, _>("id")),
            )
        })
        .collect())
}

fn validate_permission_mode(mode: &str) -> Result<()> {
    match mode {
        "strict" | "normal" | "auto" | "yolo" => Ok(()),
        other => Err(Error::Validation(format!(
            "permission_mode {other:?} must be one of strict|normal|auto|yolo"
        ))),
    }
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Task 320: heuristically classify a VCS merge error as a merge conflict
/// (GitHub returns HTTP 405 "Pull Request is not mergeable" — `design/13 §8`).
/// Used only to pick the [`FailureKind`] the UI shows; never affects control
/// flow (a conflict and a generic rejection both pause the loop).
#[cfg(unix)]
fn is_merge_conflict(e: &Error) -> bool {
    let m = e.to_string().to_lowercase();
    m.contains("not mergeable")
        || m.contains("merge conflict")
        || m.contains("405")
        || m.contains("conflict")
}

/// Task 320: read the NEW `allowMergeWithFailingChecks` key from
/// `<config_dir>/managed.json` (camelCase per `D9`). This is a key BEYOND Task
/// 211's frozen `ManagedPolicy` parser, so it is read locally here rather than
/// added to that parser (Handoff forward-note). Defaults to `false`
/// (security-conservative: the merge-anyway override is locked unless the org
/// explicitly opts in). A missing file / unparseable JSON / missing key all
/// yield `false` (locked) — an org artifact being broken must not unlock the
/// override.
// Only the `#[cfg(unix)]` coordinated-merge loop reads this (cross-platform body);
// keep it compiled everywhere but silence dead_code on non-unix (caller absent).
#[cfg_attr(not(unix), allow(dead_code))]
fn read_allow_merge_with_failing_checks(config_dir: &Path) -> bool {
    let path = config_dir.join(crate::security::managed::MANAGED_FILE_NAME);
    let Ok(bytes) = std::fs::read(&path) else {
        return false;
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    // Accept both the camelCase key (`D9`) and the snake_case alias, mirroring
    // 211's dual-spelling tolerance.
    json.get("allowMergeWithFailingChecks")
        .or_else(|| json.get("allow_merge_with_failing_checks"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Production [`PrSetVcs`] impl over the [`concerto_vcs::VcsHandle`]: builds an
/// octocrab provider from the keychain GitHub PAT (`SecretKind::GithubPat`) per
/// call (mirroring the `Vcs` gRPC handler's `provider_for_repo`) and routes the
/// single-PR merge/revert through the FROZEN `VcsProvider` trait so the loop gets
/// a real `MergeReport` (with the post-merge SHA, `design/13 §7.2`) / `RevertReport`.
struct VcsHandleMerger {
    vcs: concerto_vcs::VcsHandle,
}

impl VcsHandleMerger {
    /// Build the octocrab provider for a merge/revert call from the keychain PAT.
    async fn provider(&self) -> Result<Arc<dyn concerto_vcs::provider::VcsProvider>> {
        let secrets = concerto_keychain::Secrets::new();
        let pat = secrets
            .get(concerto_keychain::SecretKind::GithubPat)
            .await
            .map_err(|e| Error::Internal(format!("loading GitHub PAT: {e}")))?
            .ok_or_else(|| {
                Error::VcsNotAuthenticated(
                    "no GitHub PAT configured (SecretKind::GithubPat); connect GitHub in Settings"
                        .to_string(),
                )
            })?;
        self.vcs.github_provider(pat.expose()).await
    }
}

#[async_trait]
impl PrSetVcs for VcsHandleMerger {
    async fn merge_pr(
        &self,
        _repository_id: &concerto_persist::RepositoryId,
        repository_full_name: &str,
        pr_number: i64,
        method: MergeMethod,
    ) -> Result<ProviderMergeReport> {
        let provider = self.provider().await?;
        let id =
            concerto_vcs::provider::ProviderPrId::new(repository_full_name.to_string(), pr_number);
        provider.merge_pr(id, method).await
    }

    async fn revert_pr(
        &self,
        _repository_id: &concerto_persist::RepositoryId,
        repository_full_name: &str,
        pr_number: i64,
        _hard_reset: bool,
    ) -> Result<ProviderRevertReport> {
        let provider = self.provider().await?;
        let id =
            concerto_vcs::provider::ProviderPrId::new(repository_full_name.to_string(), pr_number);
        provider.revert_pr(id).await
    }
}

/// Append `.context/` to the worktree's `.git/info/exclude`.
///
/// Each worktree has its own `.git/info/`. In a linked worktree, the
/// worktree's top-level `.git` is a regular file containing a `gitdir:`
/// pointer to the per-worktree gitdir under the main repo's
/// `.git/worktrees/<name>/`. `info/exclude` lives at that gitdir, NOT at
/// the shared `objects/` location.
async fn append_context_to_git_exclude(worktree: &Path) -> Result<()> {
    let gitdir = resolve_gitdir(worktree).await?;
    let info_dir = gitdir.join("info");
    tokio::fs::create_dir_all(&info_dir).await?;
    let exclude_path = info_dir.join("exclude");

    // If exclude already exists, only append if `.context/` is not
    // already present (idempotent under retry).
    let already_has = match tokio::fs::read_to_string(&exclude_path).await {
        Ok(s) => s.lines().any(|l| l.trim() == ".context/"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(Error::Io(e)),
    };
    if already_has {
        return Ok(());
    }

    use tokio::io::AsyncWriteExt;
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude_path)
        .await?;
    f.write_all(b".context/\n").await?;
    f.flush().await?;
    Ok(())
}

/// Resolve a worktree's actual gitdir. For the main worktree this is
/// `<worktree>/.git/`; for a linked worktree, `<worktree>/.git` is a
/// regular file containing `gitdir: <path>`.
async fn resolve_gitdir(worktree: &Path) -> Result<PathBuf> {
    let dot_git = worktree.join(".git");
    let md = tokio::fs::metadata(&dot_git).await?;
    if md.is_dir() {
        return Ok(dot_git);
    }
    // Pointer file.
    let contents = tokio::fs::read_to_string(&dot_git).await?;
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("gitdir:") {
            let raw = rest.trim();
            let p = PathBuf::from(raw);
            // The path is usually absolute; tolerate a relative one by
            // joining against the worktree dir.
            return Ok(if p.is_absolute() { p } else { worktree.join(p) });
        }
    }
    Err(Error::Internal(format!(
        "worktree .git pointer file at {} is missing a `gitdir:` line",
        dot_git.display()
    )))
}

/// Best-effort cleanup of every per-repo worktree built during a failed
/// `create_workarea` attempt (a UNIQUE composer collision, a per-repo
/// `git worktree add` failure, or a DB error mid-tx; Task 306). Each
/// `(repo_local, worktree_dir)` pair is removed via
/// [`remove_worktree_best_effort`], then the shared `worktree_root` (and
/// the `.context/` skeleton under it) is removed wholesale. All errors
/// are swallowed — the next composer attempt re-creates the tree, and
/// the abort path has already captured the real error to propagate.
async fn cleanup_worktrees(built: &[(PathBuf, PathBuf)], worktree_root: &Path) {
    for (repo_local, worktree_dir) in built {
        let _ = remove_worktree_best_effort(repo_local, worktree_dir).await;
    }
    let _ = tokio::fs::remove_dir_all(worktree_root).await;
}

/// Shell out to `git worktree remove --force <dest>` for the rare
/// collision-retry cleanup path. Best-effort; errors are swallowed by
/// the caller (the directory removal that follows handles the leftover
/// FS state regardless of git's bookkeeping).
async fn remove_worktree_best_effort(repo_dir: &Path, dest: &Path) -> Result<()> {
    use tokio::process::Command;
    let out = Command::new("git")
        .args(["worktree", "remove", "--force", &dest.to_string_lossy()])
        .current_dir(repo_dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .await
        .map_err(Error::Io)?;
    if !out.status.success() {
        // Don't surface the error — collision retry is best-effort
        // and the disk-side `remove_dir_all` is the real cleanup.
        tracing::debug!(
            stderr = %String::from_utf8_lossy(&out.stderr),
            "git worktree remove failed during collision cleanup; ignoring"
        );
    }
    Ok(())
}

/// Supervised actor wrapper. Mirrors `WorkspaceManagerActor`.
pub struct WorkareaManagerActor {
    handle: WorkareaManager,
}

impl WorkareaManagerActor {
    pub fn new(
        persistence: Arc<Persistence>,
        repo_manager: RepoManager,
        data_dir: Arc<PathBuf>,
        config_dir: Arc<PathBuf>,
    ) -> Self {
        Self {
            handle: WorkareaManager::new(persistence, repo_manager, data_dir, config_dir),
        }
    }

    /// Cheap clone of the shared handle.
    pub fn handle(&self) -> WorkareaManager {
        self.handle.clone()
    }
}

#[async_trait]
impl Actor for WorkareaManagerActor {
    const NAME: &'static str = "workarea-manager";
    type Config = WorkareaManagerConfig;

    async fn run(self, ctx: ActorContext<Self::Config>) -> Result<()> {
        tracing::info!("WorkareaManager ready");
        ctx.shutdown.cancelled().await;
        tracing::debug!("WorkareaManager actor shutting down");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn allocate_picks_first_when_empty() {
        let used = HashSet::new();
        assert_eq!(allocate_composer(&used).as_deref(), Some(COMPOSERS[0]));
    }

    #[test]
    fn allocate_skips_used_names() {
        let mut used = HashSet::new();
        used.insert(COMPOSERS[0].to_string());
        used.insert(COMPOSERS[1].to_string());
        let pick = allocate_composer(&used).expect("alloc");
        assert!(!used.contains(&pick));
        // The pick should be one of the early COMPOSERS entries.
        assert!(COMPOSERS.iter().any(|n| *n == pick));
    }

    #[test]
    fn allocate_overflows_to_suffix() {
        // Mark every bare composer name used.
        let used: HashSet<String> = COMPOSERS.iter().map(|n| n.to_string()).collect();
        let pick = allocate_composer(&used).expect("alloc with overflow");
        assert!(
            pick.contains('-'),
            "overflow allocation should yield a `-N` suffix, got {pick:?}"
        );
        // First overflow candidate should be `<COMPOSERS[0]>-2`.
        let expected = format!("{}-2", COMPOSERS[0]);
        assert_eq!(pick, expected);
    }
}
