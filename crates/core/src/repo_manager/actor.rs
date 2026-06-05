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
use concerto_gix_wrap::{self as gixw, CloneStrategy, SizeReport};
use concerto_persist::{NewRepository, Persistence, Repository, RepositoryId};
use tokio::sync::Mutex;

use crate::repo_manager::{fsmonitor, repo_state};
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
        }
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

        ctx.shutdown.cancelled().await;
        tracing::debug!("RepoManager actor shutting down");
        // The supervisor honours the cancellation token; aborting is
        // safe as a backstop in case its tick() is mid-sleep.
        supervisor_handle.abort();
        Ok(())
    }
}
