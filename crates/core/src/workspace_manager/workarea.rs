//! Workarea-creation logic (Task 20).
//!
//! Sits alongside [`crate::workspace_manager::actor::WorkspaceManager`]:
//! the [`WorkareaManager`] handle owns workarea lifecycle (create / get /
//! list / archive), worktree setup, and the `.context/` skeleton.
//!
//! ## V0.1 contract (locked by Task 20)
//!
//! - `create_workarea` validates the workspace exists, is not archived,
//!   and has exactly one repository attached.
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
//!   └── <repo.name>/        ← git worktree add target
//!   ```
//! - `.context/` is appended to each worktree's `.git/info/exclude` so
//!   agent scratch is not tracked.
//! - Workarea row + `workarea_repos` row + `created → active` status
//!   transition all commit in one transaction.
//! - On success, [`WorkareaEvent::Created`] is published on the
//!   broadcast channel.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use concerto_error::{Error, Result};
use concerto_persist::{
    NewWorkarea, NewWorkareaRepo, Persistence, Repository, Workarea, WorkareaId, WorkspaceId,
};
use sqlx::Connection;
use tokio::sync::broadcast;

use crate::repo_manager::RepoManager;
use crate::supervisor::{Actor, ActorContext};
use crate::workspace_manager::COMPOSERS;

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
#[derive(Debug, Clone)]
pub enum WorkareaEvent {
    /// A new workarea was created. Payload is the persisted row (with
    /// `status == "active"`).
    Created(Workarea),
    /// A workarea was archived. Payload is the workarea id.
    Archived(WorkareaId),
}

/// Cloneable handle to the Workarea Manager's shared state.
#[derive(Clone)]
pub struct WorkareaManager {
    persistence: Arc<Persistence>,
    repo_manager: RepoManager,
    /// `<data_dir>` — the workarea root is computed as
    /// `<data_dir>/workspaces/<workspace.slug>/<composer>/`.
    data_dir: Arc<PathBuf>,
    events: broadcast::Sender<WorkareaEvent>,
}

impl WorkareaManager {
    /// Build a fresh handle. Normally callers go through
    /// [`WorkareaManagerActor::new`]; this is `pub` so tests can
    /// construct one without the supervisor.
    pub fn new(
        persistence: Arc<Persistence>,
        repo_manager: RepoManager,
        data_dir: Arc<PathBuf>,
    ) -> Self {
        let (events, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            persistence,
            repo_manager,
            data_dir,
            events,
        }
    }

    /// Subscribe to `workarea.events`.
    pub fn subscribe(&self) -> broadcast::Receiver<WorkareaEvent> {
        self.events.subscribe()
    }

    /// Create a workarea.
    ///
    /// Steps (per `design/03 §3.3` + §6.2):
    /// 1. Validate workspace exists + not archived; resolve its repo.
    /// 2. Ensure the repo is cloned on disk (via [`RepoManager`]).
    /// 3. Allocate a composer name + branch + worktree root path.
    /// 4. Run `git worktree add` into `<worktree_root>/<repo.name>/`.
    /// 5. Lay down `.context/{PROMPT.md, todos.md, scratch/}`.
    /// 6. Append `.context/` to the worktree's `.git/info/exclude`.
    /// 7. Persist `workareas` (status `"created"`) + `workarea_repos` +
    ///    transition to `"active"` in one transaction.
    /// 8. Emit [`WorkareaEvent::Created`].
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

        // V0.1: exactly one repository attached.
        let repo_ids =
            concerto_persist::workspaces::list_repos(self.persistence.readers(), &ws_id).await?;
        if repo_ids.len() != 1 {
            return Err(Error::Validation(format!(
                "workarea.v0_single_repo_only: V0.1 supports workspaces with exactly one repository; workspace {workspace_id} has {}",
                repo_ids.len()
            )));
        }
        let repo_id = &repo_ids[0];
        let repo: Repository =
            concerto_persist::repositories::get(self.persistence.readers(), repo_id)
                .await?
                .ok_or_else(|| {
                    Error::Internal(format!(
                        "workspace_repos points at non-existent repository {repo_id}"
                    ))
                })?;

        // Ensure the repo is cloned on disk. If `local_path/.git`
        // already exists, the prior clone is reused. Otherwise we clone
        // synchronously (no progress sink — workarea creation is a
        // single user-facing action, the gRPC reply is the progress).
        let repo_local = PathBuf::from(&repo.local_path);
        if !repo_local.join(".git").exists() && !repo_local.join("HEAD").exists() {
            // Not a clone yet. Drive the per-repo lock + clone via the
            // RepoManager so a concurrent create_workarea on the same
            // repo serializes. `clone_repo` is idempotent at the FS
            // layer (git refuses if dest exists & non-empty).
            self.repo_manager.clone_repo(&repo.id, None).await?;
        }

        // Allocate composer name with collision retry. The loop body
        // computes the candidate, builds on-disk artefacts, then opens
        // a transaction. On UNIQUE violation we roll back, clean up the
        // FS work, and try the next name.
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
            let repo_worktree = worktree_root.join(&repo.name);

            // 1. Ensure the workarea root directory exists. If a prior
            //    failed attempt left stuff behind we'll discover and
            //    remove it before re-trying.
            tokio::fs::create_dir_all(&worktree_root).await?;

            // 2. Run `git worktree add` for the repo. This is the
            //    expensive step.
            concerto_gix_wrap::worktree_add(&repo_local, &branch, &repo_worktree).await?;

            // 3. Create `.context/` skeleton.
            let context_dir = worktree_root.join(".context");
            tokio::fs::create_dir_all(context_dir.join("scratch")).await?;
            tokio::fs::write(context_dir.join("PROMPT.md"), b"").await?;
            tokio::fs::write(context_dir.join("todos.md"), b"").await?;

            // 4. Append `.context/` to the worktree's
            //    `.git/info/exclude`. Each worktree owns its own
            //    `.git/info/`; the worktree's `.git` is a pointer file,
            //    so we resolve the real `info/` via git's own layout.
            append_context_to_git_exclude(&repo_worktree).await?;

            // 5. Persist row + junction + status transition in one tx.
            let id = WorkareaId(uuid::Uuid::now_v7().to_string());
            let worktree_root_str = worktree_root.to_string_lossy().into_owned();
            let worktree_path_str = repo_worktree.to_string_lossy().into_owned();

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
                    concerto_persist::workareas::insert_workarea_repo(
                        &mut tx,
                        NewWorkareaRepo {
                            workarea_id: id.clone(),
                            repository_id: repo.id.clone(),
                            worktree_path: worktree_path_str.clone(),
                            branch_override: None,
                        },
                    )
                    .await?;
                    concerto_persist::workareas::update_status(&mut tx, &id, "active").await?;
                    tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
                    drop(writer);
                    break Workarea {
                        id,
                        workspace_id: ws_id.clone(),
                        composer_name: composer,
                        branch_name: branch,
                        worktree_root: worktree_root_str,
                        status: "active".to_string(),
                        permission_mode: permission_mode.clone(),
                        created_at: now_ms,
                        archived_at: None,
                        last_activity_at: None,
                    };
                }
                Err(Error::Sqlx(boxed))
                    if concerto_persist::workareas::is_unique_violation(&boxed) =>
                {
                    // Roll back the DB tx, undo the worktree, and pick
                    // the next composer.
                    let _ = tx.rollback().await;
                    drop(writer);
                    // Best-effort cleanup of the worktree we created.
                    // `gix-wrap` exposes `worktree_add` only; for the
                    // rare collision path we shell out directly to
                    // `git worktree remove --force` so the next
                    // attempt has a clean filesystem.
                    let _ = remove_worktree_best_effort(&repo_local, &repo_worktree).await;
                    let _ = tokio::fs::remove_dir_all(&worktree_root).await;
                    continue;
                }
                Err(other) => {
                    let _ = tx.rollback().await;
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

    /// Archive a workarea. Sets `archived_at` and transitions `status`
    /// to `"archived"`. Idempotent.
    pub async fn archive(&self, id: &WorkareaId) -> Result<()> {
        if self.get(id).await?.is_none() {
            return Err(Error::NotFound(format!("workarea {id} not found")));
        }
        let now_ms = now_unix_ms();
        let mut writer = self.persistence.writer().await;
        concerto_persist::workareas::archive(&mut writer, id, now_ms).await?;
        drop(writer);
        let _ = self.events.send(WorkareaEvent::Archived(id.clone()));
        Ok(())
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
    ) -> Self {
        Self {
            handle: WorkareaManager::new(persistence, repo_manager, data_dir),
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
