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
use concerto_gix_wrap as gixw;
use concerto_persist::{NewRepository, Persistence, Repository, RepositoryId};
use tokio::sync::Mutex;

use crate::supervisor::{Actor, ActorContext};

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
        }
    }

    /// Persist a new `repositories` row.
    ///
    /// `default_branch` falls back to `"main"` when empty. The on-disk
    /// path is `<repos_root>/<id>/` (`design/02 §4`).
    pub async fn add_repository(
        &self,
        project_id: &str,
        name: &str,
        url: &str,
        default_branch: &str,
    ) -> Result<Repository> {
        let id = RepositoryId(uuid::Uuid::now_v7().to_string());
        let local_path = self.repos_root.join(id.as_str());
        let default_branch = if default_branch.is_empty() {
            "main".to_string()
        } else {
            default_branch.to_string()
        };
        let row = NewRepository {
            id: id.clone(),
            project_id: project_id.to_string(),
            name: name.to_string(),
            url: url.to_string(),
            local_path: local_path.to_string_lossy().into_owned(),
            // V0.1 ships full clone only (design/02 §2). Sparse +
            // blobless land at V1.0 (Task 28+).
            clone_strategy: "full".to_string(),
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
            clone_strategy: "full".to_string(),
            default_branch,
            last_fetch_at: None,
        })
    }

    /// Look up a repository by id.
    pub async fn get(&self, id: &RepositoryId) -> Result<Option<Repository>> {
        concerto_persist::repositories::get(self.persistence.readers(), id).await
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

    /// Full clone of the repository identified by `id`.
    ///
    /// Named `clone_repo` rather than `clone` so it doesn't shadow the
    /// `Clone::clone` blanket impl (the type derives `Clone`).
    ///
    /// Locks the per-repo mutex for the duration. Two clones of
    /// different repos can proceed in parallel; two clones of the same
    /// repo serialize. On success updates `last_fetch_at` to "now".
    pub async fn clone_repo(
        &self,
        id: &RepositoryId,
        progress: Option<gixw::ProgressSink>,
    ) -> Result<()> {
        let row = self
            .get(id)
            .await?
            .ok_or_else(|| Error::Internal(format!("repository {id} not found")))?;
        let lock = self.write_lock_for(id).await;
        let _guard = lock.lock().await;

        let dest = PathBuf::from(&row.local_path);
        gixw::clone_full(&row.url, &dest, progress).await?;

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let mut writer = self.persistence.writer().await;
        concerto_persist::repositories::update_last_fetch(&mut writer, id, now_ms).await?;
        Ok(())
    }
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

        // V0.1 has no background loop yet (fsmonitor + maintenance land
        // in Task 28). Park on shutdown.
        ctx.shutdown.cancelled().await;
        tracing::debug!("RepoManager actor shutting down");
        Ok(())
    }
}
