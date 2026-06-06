//! `RepoManagerActor` — supervised owner of the per-repo write lock.
//!
//! The actor is intentionally thin in V0.1: its job is to expose a
//! [`RepoManager`] handle (cloneable, cheap) that the gRPC
//! `RepositoriesHandler` calls into directly, and to keep the supervisor
//! happy by holding a `run` loop that idles on shutdown. Heavy state
//! (per-repo mutexes, the `Arc<Persistence>` handle, the local repo
//! root) lives on the handle itself, which is built once in
//! [`RepoManagerActor::new`] and cloned into each restart by the
//! supervisor's factory.
//!
//! Concurrency rule (`design/02 §6.1`): one write per repository at a
//! time. Two clones for different repos run in parallel; two clones for
//! the same repo serialize on the per-repo mutex.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use concerto_error::{Error, Result};
use concerto_gix_wrap::{self as gixw, CloneStrategy, ConePath, PrewarmProgressEvent, SizeReport};
use concerto_persist::{NewRepository, Persistence, Repository, RepositoryId, WorkareaId};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::audit::{AuditActor, AuditEvent, AuditKind, AuditWriter, EntityKind};
use crate::repo_manager::prefetch::{BandwidthLimiter, PrewarmHandle, GLOBAL_PREWARM_CONCURRENCY};
use crate::repo_manager::{cones, fsmonitor, prefetch, repo_state};
use crate::supervisor::{Actor, ActorContext};

/// The `> 10 GB` non-sparse threshold that triggers a `repo.size_warning`
/// (`design/02 §5.3`). Matches the `estimate_repo_size` tier boundary.
const SIZE_WARNING_THRESHOLD_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// Config for the actor's `run` loop. V0.1 carries the on-disk repo
/// root (default `<data_dir>/repos/`); future tasks (28, fsmonitor)
/// add more knobs.
#[derive(Clone, Debug)]
pub struct RepoManagerConfig {
    /// `<data_dir>/repos/` — each repo lives at `<repos_root>/<id>/`.
    pub repos_root: PathBuf,
}

/// Cloneable handle to the Repository Manager's shared state.
///
/// All real work happens through this struct: the actor's `run` only
/// exists to keep the supervisor in the picture (so a future watchdog
/// or hot-reload hook has somewhere to land). Callers get one by
/// constructing the actor (which builds the handle) and pulling it via
/// [`RepoManagerActor::handle`].
#[derive(Clone)]
pub struct RepoManager {
    persistence: Arc<Persistence>,
    repos_root: PathBuf,
    /// Per-repo write mutex. `Arc<Mutex<HashMap<...>>>` so the outer
    /// map is shared across clones; each entry's `Arc<Mutex<()>>` is
    /// cloned out before the outer guard drops, so two repos can be
    /// in flight simultaneously without holding the outer lock.
    write_locks: Arc<Mutex<HashMap<RepositoryId, Arc<Mutex<()>>>>>,
    /// Per-repo fsmonitor restart history (Task 28). Shared between the
    /// `clone_repo` bring-up path and the 30s supervisor loop spawned
    /// by `RepoManagerActor::run` so a flaky daemon's restart count
    /// crosses both surfaces.
    fsmonitor_history: Arc<Mutex<HashMap<RepositoryId, fsmonitor::RestartHistory>>>,
    /// Task 302 audit writer. `None` in tests that don't care about audit
    /// emission; `Some` in production (wired in `boot.rs`). When `Some`,
    /// the `§8` force-non-cone-to-cone path appends a typed
    /// [`AuditKind::SparseConfigForcedToCone`] event.
    audit: Option<AuditWriter>,
    /// Task 304: global prewarm concurrency cap (`design/02 §6.1`). Shared
    /// across clones so the 2-permit limit holds across all repos. A
    /// prewarm job holds one permit for its whole fetch.
    prewarm_sem: Arc<Semaphore>,
    /// Task 304: per-repo bandwidth-cap seam (`design/02 §6.1`). Consulted
    /// before each prewarm fetch; the real throttle is a follow-on.
    bandwidth: BandwidthLimiter,
    /// Task 304: injected idle/power/net signal bundle for the prewarm
    /// scheduler (`PHASE3_PLANNING §2`). Defaults to the conservative
    /// `never_prewarm` bundle; `boot.rs` injects the host bundle, and tests
    /// inject deterministic mocks.
    prewarm_signals: prefetch::PrewarmSignals,
}

impl RepoManager {
    /// Build a fresh handle. Normally callers go through
    /// [`RepoManagerActor::new`]; this is `pub` so tests can construct
    /// one without going through the supervisor.
    pub fn new(persistence: Arc<Persistence>, repos_root: PathBuf) -> Self {
        Self {
            persistence,
            repos_root,
            write_locks: Arc::new(Mutex::new(HashMap::new())),
            fsmonitor_history: Arc::new(Mutex::new(HashMap::new())),
            audit: None,
            prewarm_sem: Arc::new(Semaphore::new(GLOBAL_PREWARM_CONCURRENCY)),
            bandwidth: BandwidthLimiter::new(),
            prewarm_signals: prefetch::never_prewarm_signals(),
        }
    }

    /// Inject the prewarm scheduler's idle/power/net signal bundle (Task
    /// 304). Production wires `prefetch::signals::host_signals()` in
    /// `boot.rs`; tests inject a deterministic mock. Mirrors
    /// [`with_audit`](Self::with_audit).
    pub fn with_prewarm_signals(mut self, signals: prefetch::PrewarmSignals) -> Self {
        self.prewarm_signals = signals;
        self
    }

    /// The injected prewarm signal bundle (used by `RepoManagerActor::run`
    /// to spawn the scheduler). Cloneable — closures are `Arc`-wrapped.
    pub(crate) fn prewarm_signals(&self) -> prefetch::PrewarmSignals {
        self.prewarm_signals.clone()
    }

    /// Test/inspection hook: how many times the per-repo bandwidth limiter
    /// has been consulted on the prewarm path (`design/02 §6.1`).
    pub fn bandwidth_consult_count(&self) -> u64 {
        self.bandwidth.consult_count()
    }

    /// Attach a Task 44 [`AuditWriter`] so the Task 302 `§8`
    /// force-non-cone-to-cone path emits a typed audit event. Production
    /// wires this in `boot.rs`; tests can leave `audit = None`. Mirrors
    /// `WorkspaceManager::with_audit`.
    pub fn with_audit(mut self, audit: AuditWriter) -> Self {
        self.audit = Some(audit);
        self
    }

    /// Cloneable handle to the per-repo fsmonitor restart history. The
    /// `RepoManagerActor::run` task hands this to
    /// [`fsmonitor::spawn_supervisor`] so the supervisor and the
    /// clone-time bring-up share one bookkeeping map.
    pub(crate) fn fsmonitor_history(
        &self,
    ) -> Arc<Mutex<HashMap<RepositoryId, fsmonitor::RestartHistory>>> {
        Arc::clone(&self.fsmonitor_history)
    }

    /// Cloneable handle to the persistence layer. Used by
    /// `RepoManagerActor::run` to construct the supervisor loop.
    pub(crate) fn persistence(&self) -> Arc<Persistence> {
        Arc::clone(&self.persistence)
    }

    /// Persist a new `repositories` row with a real clone [`CloneStrategy`]
    /// (Task 301).
    ///
    /// `default_branch` falls back to `"main"` when empty. The on-disk
    /// path is `<repos_root>/<id>/` (`design/02 §4`). The chosen strategy
    /// is written into the `repositories.clone_strategy` column verbatim
    /// (`full | blobless | treeless`) — V0.1's hardcoded `"full"` is gone.
    ///
    /// `with_sparse` is the recommendation's "Blobless + Sparse" intent.
    /// V1.0 has no per-repo column for it — the actual sparse-checkout
    /// init/set is per-(workarea, repo) and owned by **Task 302** (which
    /// reads cones from `workarea_repos.sparse_cones_json`). 301 accepts +
    /// validates the flag for the locked `AddRepository` signature but does
    /// not persist it; see this task's Handoff *Deliberate debt*.
    pub async fn add_repository(
        &self,
        project_id: &str,
        name: &str,
        url: &str,
        default_branch: &str,
        strategy: CloneStrategy,
        with_sparse: bool,
    ) -> Result<Repository> {
        // `with_sparse` is part of the locked signature; Task 302 wires the
        // real per-workarea sparse setup. Bind it so the parameter is not a
        // dead arg and the intent is greppable for 302.
        let _ = with_sparse;
        let id = RepositoryId(uuid::Uuid::now_v7().to_string());
        let local_path = self.repos_root.join(id.as_str());
        let default_branch = if default_branch.is_empty() {
            "main".to_string()
        } else {
            default_branch.to_string()
        };
        let strategy_str = strategy.as_str().to_string();
        let row = NewRepository {
            id: id.clone(),
            project_id: project_id.to_string(),
            name: name.to_string(),
            url: url.to_string(),
            local_path: local_path.to_string_lossy().into_owned(),
            // Task 301: persist the real strategy (design/02 §2/§3.1).
            clone_strategy: strategy_str.clone(),
            default_branch: default_branch.clone(),
        };
        let mut writer = self.persistence.writer().await;
        concerto_persist::repositories::insert(&mut writer, row).await?;
        Ok(Repository {
            id,
            project_id: project_id.to_string(),
            name: name.to_string(),
            url: url.to_string(),
            local_path: local_path.to_string_lossy().into_owned(),
            clone_strategy: strategy_str,
            default_branch,
            // Task 302: a freshly-added repo has the SQL default (`'[]'`)
            // for its cone-defaults layer; the user sets repo-level
            // defaults later (Desktop, Task 322). Mirror the column default
            // here so the returned row matches what was just inserted.
            cone_defaults_json: "[]".to_string(),
            // Task 310: a freshly-added repo has the SQL default (`'{}'`) for
            // its action-prefs layer (migration 0011, `design/04 §3.13`);
            // mirror the column default so the returned row matches the insert.
            action_prefs_json: "{}".to_string(),
            last_fetch_at: None,
            // No daemon recorded until `clone_repo` finishes and the
            // post-clone fsmonitor bring-up persists a PID (Task 28).
            fs_monitor_pid: None,
        })
    }

    /// Probe a git `url`'s size and recommend a [`CloneStrategy`] BEFORE
    /// adding the repo (Task 301, `design/02 §3.5`/`§7.1`).
    ///
    /// Thin pass-through to [`gixw::estimate_repo_size`]; the per-add /
    /// explicit-RPC probe must never touch the Core boot path. A probe
    /// failure (private repo, offline) propagates as [`Error::Git`] — the
    /// caller falls back to a manual strategy pick.
    pub async fn estimate_size(&self, url: &str) -> Result<SizeReport> {
        gixw::estimate_repo_size(url).await
    }

    /// Look up a repository by id.
    pub async fn get(&self, id: &RepositoryId) -> Result<Option<Repository>> {
        concerto_persist::repositories::get(self.persistence.readers(), id).await
    }

    /// List every repository attached to `project_id`. Read-only. The
    /// Desktop "Add Repository" form (Task 25) renders this list so the
    /// New Workspace modal's repo picker has something to show.
    pub async fn list_by_project(&self, project_id: &str) -> Result<Vec<Repository>> {
        concerto_persist::repositories::list_by_project(self.persistence.readers(), project_id)
            .await
    }

    /// Acquire (creating on first use) the per-repo write mutex.
    ///
    /// The returned `Arc<Mutex<()>>` is cloned out before the outer
    /// `write_locks` guard drops, so two different repos do not
    /// contend on the outer map.
    async fn write_lock_for(&self, id: &RepositoryId) -> Arc<Mutex<()>> {
        let mut guard = self.write_locks.lock().await;
        guard
            .entry(id.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Clone the repository identified by `id` using its persisted
    /// [`CloneStrategy`] (Task 301).
    ///
    /// Named `clone_repo` rather than `clone` so it doesn't shadow the
    /// `Clone::clone` blanket impl (the type derives `Clone`).
    ///
    /// Reads the row's `clone_strategy` column (`full | blobless |
    /// treeless`) and routes through [`gixw::clone_with_strategy`] instead
    /// of the V0.1 hardcoded `clone_full`. The clone is non-sparse here:
    /// per-(workarea, repo) sparse setup is **Task 302**, so a `with_sparse`
    /// flag at add-time is not threaded into this path in V1.0 (see the
    /// task Handoff). An unrecognized stored strategy is an [`Error::Git`].
    ///
    /// Locks the per-repo mutex for the duration. Two clones of
    /// different repos can proceed in parallel; two clones of the same
    /// repo serialize. On success: updates `last_fetch_at`, writes
    /// `size_bytes`/`object_count` to the repo-local `concerto-state.json`
    /// (`design/02 §4`), and emits `repo.size_warning` when a `> 10 GB`
    /// repo was cloned non-sparse (`design/02 §5.3`).
    pub async fn clone_repo(
        &self,
        id: &RepositoryId,
        progress: Option<gixw::ProgressSink>,
    ) -> Result<()> {
        let row = self
            .get(id)
            .await?
            .ok_or_else(|| Error::Internal(format!("repository {id} not found")))?;
        let strategy: CloneStrategy = row.clone_strategy.parse()?;
        let lock = self.write_lock_for(id).await;
        let _guard = lock.lock().await;

        let dest = PathBuf::from(&row.local_path);
        // V1.0 (Task 301): route through the strategy-aware clone. Sparse
        // checkout (the `--sparse --no-checkout` flags) is Task 302's job,
        // so this path always clones non-sparse (`with_sparse = false`).
        gixw::clone_with_strategy(&row.url, &dest, strategy, false, progress).await?;

        // Task 28 — post-clone bring-up:
        //   1. Apply the four locked `git config` performance keys.
        //   2. Register OS-level scheduled `git maintenance`.
        //   3. Start `git fsmonitor--daemon` and capture its PID.
        // Each step is best-effort: fsmonitor in particular is allowed
        // to fail on unsupported filesystems and disable gracefully per
        // `design/02 §8`.
        let fs_monitor_pid = fsmonitor::bring_up_after_clone(&dest).await;

        // Task 301: measure the on-disk object DB post-clone (`git
        // count-objects -v`) and persist it to the repo-local
        // `concerto-state.json` (read-modify-write so future fields, e.g.
        // 304's `prefetch_cursor`, are preserved). Best-effort — a state
        // write failure must not fail the clone.
        let (size_bytes, object_count) = match measure_object_db(&dest).await {
            Ok(measured) => measured,
            Err(e) => {
                tracing::debug!(repo_id = %id, error = %e, "count-objects probe failed; recording zeros");
                (0, 0)
            }
        };
        if let Err(e) = repo_state::record_size(&dest, size_bytes, object_count).await {
            tracing::warn!(repo_id = %id, error = %e, "failed to write concerto-state.json");
        }

        // Task 301: `repo.size_warning` (design/02 §5.3) — a > 10 GB repo
        // cloned non-sparse should prompt the user toward sparse. There is
        // no repo-event broadcast subject wired through the streams handler
        // yet (Task 28 deferred the equivalent `repo.fsmonitor_restarted`
        // broadcast to a Phase-3 follow-on for the same reason); emit the
        // same `tracing` audit-line shape until that channel lands.
        if size_bytes > SIZE_WARNING_THRESHOLD_BYTES {
            tracing::warn!(
                repo_id = %id,
                size_bytes,
                strategy = %strategy,
                "repo.size_warning: repository exceeds 10 GB and was cloned non-sparse; recommend sparse checkout"
            );
        }

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let mut writer = self.persistence.writer().await;
        concerto_persist::repositories::update_last_fetch(&mut writer, id, now_ms).await?;
        concerto_persist::repositories::update_fs_monitor_pid(
            &mut writer,
            id,
            fs_monitor_pid.map(|p| p as i64),
        )
        .await?;
        Ok(())
    }

    /// Pre-fetch the lazy blobs reachable in `cones` at `commit` for a
    /// blobless clone (Task 304, `design/02 §3.3`/`§5.1`/`§6.3`). FROZEN
    /// signature.
    ///
    /// Returns a [`PrewarmHandle`] carrying a [`CancellationToken`] + the
    /// spawned [`tokio::task::JoinHandle`]; dropping/cancelling the handle
    /// stops the fetch promptly (between cone chunks).
    ///
    /// Concurrency (`design/02 §6.1`), all held for the whole fetch:
    /// 1. **global 2-concurrent** — acquire one [`Semaphore`] permit across
    ///    all repos;
    /// 2. **per-repo write lock** — serialize against a concurrent
    ///    clone/fetch of the same repo;
    /// 3. **per-repo bandwidth cap** — consult the [`BandwidthLimiter`].
    ///
    /// Emits `repo.prefetch_started` / `repo.prefetch_finished` (the
    /// broadcast subject is not yet wired through the streams handler — see
    /// the Task 302/301 `repo.*` precedent — so this emits the same
    /// `tracing` audit-line shape the Tray will later render). On the
    /// repo-local `concerto-state.json`, the prewarmed `commit` is recorded
    /// as `prefetch_cursor` (read-modify-write so Task 301's
    /// `size_bytes`/`object_count` are never clobbered).
    pub async fn prewarm_blobs(
        &self,
        repo: &RepositoryId,
        cones: &[ConePath],
        commit: &str,
    ) -> Result<PrewarmHandle> {
        let row = self
            .get(repo)
            .await?
            .ok_or_else(|| Error::Internal(format!("repository {repo} not found")))?;
        let repo_dir = PathBuf::from(&row.local_path);

        let token = CancellationToken::new();
        let task_token = token.clone();
        let cancel_check_token = token.clone();
        let sem = Arc::clone(&self.prewarm_sem);
        let bandwidth = self.bandwidth.clone();
        let write_lock = self.write_lock_for(repo).await;
        let persistence = Arc::clone(&self.persistence);
        let repo_id = repo.clone();
        let cones = cones.to_vec();
        let commit = commit.to_string();

        let join = tokio::spawn(async move {
            // 1. Global 2-concurrent cap (held for the whole fetch).
            let _permit = match sem.acquire_owned().await {
                Ok(p) => p,
                Err(_) => return, // semaphore closed → Core shutting down
            };
            // Cancelled while waiting for a permit?
            if task_token.is_cancelled() {
                return;
            }
            // 2. Per-repo write lock — serialize against clone/fetch.
            let _guard = write_lock.lock().await;
            // 3. Per-repo bandwidth cap (always consulted; real throttle TBD).
            bandwidth.acquire().await;

            tracing::info!(repo_id = %repo_id, commit = %commit, cones = cones.len(), "repo.prefetch_started");

            let should_cancel = move || cancel_check_token.is_cancelled();
            let progress: Option<tokio::sync::mpsc::Sender<PrewarmProgressEvent>> = None;
            let result =
                gixw::prewarm_blobs_in_cone(&repo_dir, &commit, &cones, should_cancel, progress)
                    .await;

            match result {
                Ok(fetched) => {
                    // Record the cursor only on a clean (non-cancelled) finish
                    // so a cancelled partial fetch doesn't advance it.
                    if !task_token.is_cancelled() {
                        if let Err(e) = repo_state::record_prefetch_cursor(&repo_dir, &commit).await
                        {
                            tracing::warn!(repo_id = %repo_id, error = %e, "failed to record prefetch_cursor");
                        }
                    }
                    tracing::info!(repo_id = %repo_id, blobs_fetched = fetched, cancelled = task_token.is_cancelled(), "repo.prefetch_finished");
                }
                Err(e) => {
                    tracing::warn!(repo_id = %repo_id, error = %e, "repo.prefetch_finished: prewarm fetch failed");
                }
            }
            let _ = persistence; // reserved for a future broadcast/audit emit
        });

        Ok(PrewarmHandle::new(token, join))
    }

    /// Prewarm with a live progress sink (the gRPC `PrewarmBlobs` streaming
    /// handler, Task 304). Same concurrency discipline as [`prewarm_blobs`]
    /// but forwards each [`PrewarmProgressEvent`] to `progress` so the
    /// handler can reshape it onto the gRPC stream. Returns the
    /// [`PrewarmHandle`] so the handler can cancel on client disconnect.
    pub async fn prewarm_blobs_streaming(
        &self,
        repo: &RepositoryId,
        cones: &[ConePath],
        commit: &str,
        progress: tokio::sync::mpsc::Sender<PrewarmProgressEvent>,
    ) -> Result<PrewarmHandle> {
        let row = self
            .get(repo)
            .await?
            .ok_or_else(|| Error::Internal(format!("repository {repo} not found")))?;
        let repo_dir = PathBuf::from(&row.local_path);

        let token = CancellationToken::new();
        let task_token = token.clone();
        let cancel_check_token = token.clone();
        let sem = Arc::clone(&self.prewarm_sem);
        let bandwidth = self.bandwidth.clone();
        let write_lock = self.write_lock_for(repo).await;
        let repo_id = repo.clone();
        let cones = cones.to_vec();
        let commit = commit.to_string();

        let join = tokio::spawn(async move {
            let _permit = match sem.acquire_owned().await {
                Ok(p) => p,
                Err(_) => return,
            };
            if task_token.is_cancelled() {
                return;
            }
            let _guard = write_lock.lock().await;
            bandwidth.acquire().await;

            tracing::info!(repo_id = %repo_id, commit = %commit, "repo.prefetch_started");
            let should_cancel = move || cancel_check_token.is_cancelled();
            match gixw::prewarm_blobs_in_cone(
                &repo_dir,
                &commit,
                &cones,
                should_cancel,
                Some(progress),
            )
            .await
            {
                Ok(fetched) => {
                    if !task_token.is_cancelled() {
                        let _ = repo_state::record_prefetch_cursor(&repo_dir, &commit).await;
                    }
                    tracing::info!(repo_id = %repo_id, blobs_fetched = fetched, "repo.prefetch_finished");
                }
                Err(e) => {
                    tracing::warn!(repo_id = %repo_id, error = %e, "repo.prefetch_finished: prewarm fetch failed");
                }
            }
        });

        Ok(PrewarmHandle::new(token, join))
    }

    /// Run one idle-scheduler prewarm pass (Task 304). Walks every blobless
    /// repo, resolves its tracked-branch HEAD, and prewarms its whole-tree
    /// cone at that HEAD; returns the spawned [`PrewarmHandle`]s so the
    /// scheduler can cancel them all when `idle_signal` flips to active.
    ///
    /// "Which cones to walk" is owned by Task 302's resolver; the
    /// background pass uses the empty-cone (= whole tracked tree) default
    /// because a repo's *union* of workarea cones is the correct
    /// background-prewarm scope and computing it per-workarea is the
    /// Task-302-consuming follow-on. Each repo's prewarm is independently
    /// gated by the global-2-concurrent semaphore inside `prewarm_blobs`.
    pub async fn run_prewarm_pass(&self) -> Vec<PrewarmHandle> {
        let repos = match concerto_persist::repositories::list_all(self.persistence.readers()).await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "prewarm pass: list_all failed");
                return Vec::new();
            }
        };
        let mut handles = Vec::new();
        for repo in repos {
            // Only blobless clones have lazy blobs worth prewarming.
            if repo.clone_strategy != "blobless" {
                continue;
            }
            let repo_dir = PathBuf::from(&repo.local_path);
            let head = match gixw::rev_parse_head(&repo_dir).await {
                Ok(h) => h,
                Err(e) => {
                    tracing::debug!(repo_id = %repo.id, error = %e, "prewarm pass: skip repo with no HEAD");
                    continue;
                }
            };
            match self.prewarm_blobs(&repo.id, &[], &head).await {
                Ok(h) => handles.push(h),
                Err(e) => {
                    tracing::warn!(repo_id = %repo.id, error = %e, "prewarm pass: prewarm_blobs failed");
                }
            }
        }
        handles
    }

    /// Eager worktree-create trigger (Task 304, `design/02 §3.3` trigger 1,
    /// default ON). After Task 302 sets a `(workarea, repo)` cone, the
    /// workarea-create path calls this to prewarm the new cone @ HEAD.
    /// Best-effort: returns the handle so the caller can track/cancel it,
    /// or logs + returns `None` if the repo has no HEAD / no row yet.
    ///
    /// Non-blobless repos are skipped (their blobs are already on disk).
    pub async fn prewarm_on_worktree_create(
        &self,
        repo: &RepositoryId,
        cones: &[ConePath],
    ) -> Option<PrewarmHandle> {
        let row = self.get(repo).await.ok().flatten()?;
        if row.clone_strategy != "blobless" {
            return None;
        }
        let repo_dir = PathBuf::from(&row.local_path);
        let head = gixw::rev_parse_head(&repo_dir).await.ok()?;
        self.prewarm_blobs(repo, cones, &head).await.ok()
    }

    /// Eager HEAD-update trigger (Task 304, `design/02 §3.3` trigger 2,
    /// default ON). When a repo's tracked branch advances to `new_head`,
    /// prewarm the in-cone blobs at the new commit. Best-effort; non-blobless
    /// repos are skipped.
    pub async fn prewarm_on_head_update(
        &self,
        repo: &RepositoryId,
        cones: &[ConePath],
        new_head: &str,
    ) -> Option<PrewarmHandle> {
        let row = self.get(repo).await.ok().flatten()?;
        if row.clone_strategy != "blobless" {
            return None;
        }
        self.prewarm_blobs(repo, cones, new_head).await.ok()
    }

    /// Set the sparse cone for a `(workarea, repo)` pair, applying it to the
    /// on-disk worktree AND persisting it to `workarea_repos.sparse_cones_json`
    /// (Task 302, `design/02 §3.2`/§5.1).
    ///
    /// Sequence:
    /// 1. Resolve the `(workarea, repo)` worktree path from
    ///    `workarea_repos.worktree_path`. Missing pair → [`Error::Internal`]
    ///    (the handler maps it to a clear status).
    /// 2. **`§8` correctness:** if the worktree's `core.sparseCheckoutCone`
    ///    is `false` (a manually-cloned non-cone sparse config), force it to
    ///    `true` and emit an [`AuditKind::SparseConfigForcedToCone`] audit
    ///    event. The non-cone path is never invoked.
    /// 3. `sparse_init_cone` (idempotent) to ensure cone-mode + sparse-index
    ///    are on.
    /// 4. `sparse_set` — replaces the cone with `cones`, **rejecting any
    ///    cone path absent from `HEAD`** with a clean [`Error::Git`] (so the
    ///    handler returns `INVALID_ARGUMENT`) BEFORE applying — nothing is
    ///    half-applied. `sparse_set` reapplies `--sparse-index` internally.
    /// 5. Persist the cone via
    ///    [`concerto_persist::workareas::update_workarea_repo_cones`] — the
    ///    writer that closes the "`sparse_cones_json` never written" gap.
    ///
    /// When `cones` is empty the resolver is **not** consulted — an explicit
    /// empty set means "cone down to top-level files only" (a legitimate
    /// choice). Callers wanting inheritance pass the
    /// [`cones::resolve_cones`] output (306/307 do this at workarea create);
    /// `resolve_for_workarea_repo` exposes that resolution for them.
    ///
    /// Bad cone path → returns before any persist; the on-disk worktree is
    /// left at its prior cone (git rejects the set atomically once the probe
    /// fails, since the probe runs before `git sparse-checkout set` is
    /// invoked).
    pub async fn set_workarea_repo_cones(
        &self,
        workarea: &WorkareaId,
        repo: &RepositoryId,
        cones: &[ConePath],
    ) -> Result<()> {
        // 1. Resolve the per-(workarea, repo) worktree path.
        let worktree = concerto_persist::workareas::get_workarea_repo_worktree_path(
            self.persistence.readers(),
            workarea,
            repo,
        )
        .await?
        .ok_or_else(|| {
            Error::Internal(format!(
                "no workarea_repos row for workarea {workarea} + repository {repo}"
            ))
        })?;
        let worktree = PathBuf::from(worktree);

        // Serialize cone ops for this repo behind the per-repo write lock so
        // two concurrent SetCones for the same repo can't interleave git
        // sparse-checkout mutations.
        let lock = self.write_lock_for(repo).await;
        let _guard = lock.lock().await;

        // 2. §8 correctness: force a pre-existing non-cone sparse config to
        // cone mode + audit. `is_cone_mode` returns false both for a
        // non-cone sparse config AND for a worktree that never enabled
        // sparse-checkout; forcing the key true in the latter case is
        // harmless (init below sets it anyway), but we only emit the audit
        // event + log line when sparse-checkout was actually active in a
        // non-cone configuration, to avoid noise on a plain full clone.
        if !gixw::is_cone_mode(&worktree).await? {
            // Probe whether sparse-checkout is enabled at all
            // (`core.sparseCheckout`); only a *non-cone sparse* config is the
            // §8 failure mode worth auditing.
            let sparse_enabled = sparse_checkout_enabled(&worktree).await;
            gixw::force_cone_mode(&worktree).await?;
            if sparse_enabled {
                tracing::warn!(
                    workarea = %workarea,
                    repo = %repo,
                    worktree = %worktree.display(),
                    "forced non-cone sparse config to cone mode (design/02 §8)"
                );
                self.emit_force_cone_audit(repo, &worktree);
            }
        }

        // 3. Ensure cone-mode + sparse-index are initialized (idempotent).
        gixw::sparse_init_cone(&worktree).await?;

        // 4. Apply the cone (validates bad paths first, reapplies
        // --sparse-index). A bad path returns Err here, before any persist.
        gixw::sparse_set(&worktree, cones).await?;

        // 5. Persist — the writer that closes the "sparse_cones_json never
        // written" gap.
        let mut writer = self.persistence.writer().await;
        concerto_persist::workareas::update_workarea_repo_cones(&mut writer, workarea, repo, cones)
            .await?;
        Ok(())
    }

    /// Resolve the effective cone set for a `(workarea, repo)` from the three
    /// inheritance layers (Task 302, `design/02 §3.2`) — repository
    /// `cone_defaults_json` → workspace `settings_json.cone_defaults[repo]`
    /// → workarea `sparse_cones_json`, most-specific wins.
    ///
    /// Reads the three raw JSON strings from persistence and delegates the
    /// precedence logic to the pure [`cones::resolve_cones`]. The
    /// workarea-create path (306/307) calls this to seed the resolved cone;
    /// `set_workarea_repo_cones` callers that want inheritance pass its
    /// output as `cones`.
    ///
    /// Returns an [`Error::Internal`] when the repo row or the workspace
    /// owning `workarea` cannot be found.
    pub async fn resolve_for_workarea_repo(
        &self,
        workspace_id: &concerto_persist::WorkspaceId,
        workarea: &WorkareaId,
        repo: &RepositoryId,
    ) -> Result<Vec<ConePath>> {
        let readers = self.persistence.readers();

        let repo_row = concerto_persist::repositories::get(readers, repo)
            .await?
            .ok_or_else(|| Error::Internal(format!("repository {repo} not found")))?;

        let ws_settings = concerto_persist::workspaces::get_settings_json(readers, workspace_id)
            .await?
            // A missing workspace settings row → an empty object so the
            // resolver simply skips the workspace layer.
            .unwrap_or_else(|| "{}".to_string());

        let wa_cones =
            concerto_persist::workareas::get_workarea_repo_cones(readers, workarea, repo)
                .await?
                // No junction row yet → an empty-array string so the
                // workarea layer is "present but empty" only if a row
                // exists; absence means fall through.
                .unwrap_or_else(|| "null".to_string());

        Ok(cones::resolve_cones(
            &repo_row.cone_defaults_json,
            &ws_settings,
            &wa_cones,
            repo.as_str(),
        ))
    }

    /// Emit the `§8` "forced non-cone sparse config to cone mode" audit
    /// event (Task 302). No-op when no [`AuditWriter`] is attached.
    fn emit_force_cone_audit(&self, repo: &RepositoryId, worktree: &std::path::Path) {
        if let Some(audit) = &self.audit {
            audit.append(
                AuditEvent::new(AuditKind::SparseConfigForcedToCone, AuditActor::System)
                    .with_subject(EntityKind::Repository, repo.as_str())
                    .with_details(serde_json::json!({
                        "worktree": worktree.display().to_string(),
                        "reason": "core.sparseCheckoutCone was false on a sparse worktree",
                    })),
            );
        }
    }
}

/// True iff `core.sparseCheckout` is enabled at `worktree` (Task 302). Used
/// to distinguish the `§8` non-cone-sparse failure mode (audit-worthy) from
/// a plain full clone with no sparse config (force the key silently). A
/// read failure (unset key) is treated as "not sparse".
async fn sparse_checkout_enabled(worktree: &std::path::Path) -> bool {
    match concerto_gix_wrap::cmd::run(
        &["config", "--get", "--bool", "core.sparseCheckout"],
        worktree,
    )
    .await
    {
        Ok(out) => out.stdout.trim() == "true",
        Err(_) => false,
    }
}

/// Measure a freshly-cloned repo's object DB via `git count-objects -v`
/// (Task 301). Returns `(size_bytes, object_count)`.
///
/// `git count-objects -v` reports loose + packed counts and a `size-pack`
/// (in KiB). We use `count + in-pack` for the object total and
/// `(size + size-pack) * 1024` for the byte total — the on-disk footprint
/// of `.git/objects`. A blobless clone's lazy blobs are intentionally not
/// counted (they aren't on disk yet); `concerto-state.json` records the
/// actual materialized size.
async fn measure_object_db(dest: &std::path::Path) -> Result<(u64, u64)> {
    let out = concerto_gix_wrap::cmd::run(&["count-objects", "-v"], dest).await?;
    let mut count: u64 = 0;
    let mut in_pack: u64 = 0;
    let mut size_kib: u64 = 0;
    let mut size_pack_kib: u64 = 0;
    for line in out.stdout.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().parse::<u64>().unwrap_or(0);
        match key.trim() {
            "count" => count = value,
            "in-pack" => in_pack = value,
            "size" => size_kib = value,
            "size-pack" => size_pack_kib = value,
            _ => {}
        }
    }
    let object_count = count.saturating_add(in_pack);
    let size_bytes = size_kib.saturating_add(size_pack_kib).saturating_mul(1024);
    Ok((size_bytes, object_count))
}

/// Supervised actor wrapper. Holds the shared [`RepoManager`] handle.
///
/// The supervisor's factory clones the handle on each restart, so the
/// per-repo lock map and persistence handle survive a wrapper panic.
pub struct RepoManagerActor {
    handle: RepoManager,
}

impl RepoManagerActor {
    /// Build a new actor. The handle is constructed eagerly so callers
    /// can grab a clone before spawning.
    pub fn new(persistence: Arc<Persistence>, repos_root: PathBuf) -> Self {
        Self {
            handle: RepoManager::new(persistence, repos_root),
        }
    }

    /// Cheap clone of the shared handle. Used by the gRPC handler so it
    /// can run without going through the actor's mailbox.
    pub fn handle(&self) -> RepoManager {
        self.handle.clone()
    }
}

#[async_trait]
impl Actor for RepoManagerActor {
    const NAME: &'static str = "repo-manager";
    type Config = RepoManagerConfig;

    async fn run(self, ctx: ActorContext<Self::Config>) -> Result<()> {
        // Ensure the repos root exists; the gix-wrap clone path creates
        // the parent directory on demand, but doing it once at boot keeps
        // surprise filesystem errors out of the per-clone path.
        let repos_root = {
            let cfg = ctx.config.read().await;
            cfg.repos_root.clone()
        };
        tokio::fs::create_dir_all(&repos_root).await?;
        tracing::info!(
            repos_root = %repos_root.display(),
            "RepoManager ready"
        );

        // Task 28: spawn the fsmonitor supervisor loop. It walks every
        // `repositories` row every 30s and restarts a dead daemon up to
        // 3 times in 60s before disabling for that repo. The handle is
        // dropped on shutdown — the spawned task observes the same
        // CancellationToken and exits cleanly.
        let supervisor_handle = fsmonitor::spawn_supervisor(
            self.handle.persistence(),
            self.handle.fsmonitor_history(),
            ctx.shutdown.clone(),
        );

        // Task 304: spawn the idle blob prewarm scheduler right next to the
        // fsmonitor supervisor. It mirrors the supervisor's shape (a 30s
        // interval loop + a CancellationToken) but is fully independent: it
        // gates prewarm passes on the injected idle/power/net signals
        // (`design/02 §6.3`). With the default `never_prewarm` signals it is
        // inert; `boot.rs` injects the host bundle.
        let prefetch_handle = prefetch::spawn_prefetch_scheduler(
            self.handle.clone(),
            self.handle.prewarm_signals(),
            ctx.shutdown.clone(),
        );

        ctx.shutdown.cancelled().await;
        tracing::debug!("RepoManager actor shutting down");
        // The supervisor + scheduler honour the cancellation token; aborting
        // is a safe backstop in case a tick() is mid-sleep.
        supervisor_handle.abort();
        prefetch_handle.abort();
        Ok(())
    }
}
