//! `WorkspaceManagerActor` — supervised owner of workspace creation.
//!
//! The actor is thin in V0.1: it parks on shutdown. The meaningful
//! surface is the cheap-to-clone [`WorkspaceManager`] handle, which the
//! gRPC `WorkspacesHandler` calls into. The handle owns:
//!
//! - An `Arc<Persistence>` for read/write access to the
//!   `workspaces` and `workspace_repos` tables.
//! - A `tokio::sync::broadcast::Sender<WorkspaceEvent>` (capacity 256)
//!   that emits `workspace.events: created / archived` for the future
//!   `Streams` service to subscribe to (Task 24).
//!
//! ## Contract (Task 19, relaxed to 1..N by Task 306; registry by Phase 3)
//!
//! - `create_workspace` accepts **1..N** repositories from the global
//!   registry (the Project→Workspace collapse dropped project scoping).
//!   It rejects an empty set with [`NO_REPOS_WIRE_CODE`] and a repeated id
//!   with [`DUPLICATE_REPO_WIRE_CODE`], both inside an [`Error::Validation`]
//!   the gRPC handler maps to `INVALID_ARGUMENT`.
//! - `name` must derive to a non-empty slug.
//! - Every `repository_id` must exist in the global `repositories`
//!   registry (no project membership check — repos are global, D9).
//! - Per-(workspace, repo) sparse cones are **snapshots** seeded at attach
//!   from the repo's `cone_defaults_json` when the caller passes an empty
//!   `sparse_cones` (D4); editing the repo default never mutates an
//!   existing workspace snapshot.
//! - `update_workspace_repos` re-validates + re-positions the set, seeds
//!   cones (preserving an existing per-repo snapshot), and emits
//!   `workspace.events: repos updated` (`design/03 §5.1`, §6.1).
//! - Slug collisions auto-suffix `-2`, `-3`, … with a bound on retries to
//!   stop a runaway loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use concerto_error::{Error, Result};
use concerto_persist::{
    NewWorkspace, Persistence, RepositoryId, Workspace, WorkspaceId, WorkspaceRepoCones,
};
use sqlx::Connection;
use tokio::sync::broadcast;

use crate::supervisor::{Actor, ActorContext};

/// Manager-level spec for one repository's attachment to a workspace at
/// create / update time. Distinct from the proto `WorkspaceRepoSpec`: the
/// handler maps the wire shape onto this. An empty `sparse_cones` means
/// "seed the per-(workspace, repo) snapshot from the repository's
/// `cone_defaults_json`" (D4); a non-empty list is used verbatim.
#[derive(Debug, Clone)]
pub struct WorkspaceRepoSpec {
    pub repository_id: RepositoryId,
    /// Empty → seed from repo `cone_defaults_json` (snapshot, D4).
    pub sparse_cones: Vec<String>,
}

/// Wire-code surfaced inside the [`Error::Validation`] payload when a
/// caller requests a workspace with an **empty** repository set (Task
/// 306). The handler maps this to `INVALID_ARGUMENT`.
pub const NO_REPOS_WIRE_CODE: &str = "workspace.no_repos";

/// Wire-code surfaced inside the [`Error::Validation`] payload when a
/// caller lists the **same** repository id twice (Task 306). The handler
/// maps this to `INVALID_ARGUMENT`.
pub const DUPLICATE_REPO_WIRE_CODE: &str = "workspace.duplicate_repo";

/// Maximum number of slug-suffix retries before giving up. 100 keeps
/// runaway loops bounded; the user-visible UI would never realistically
/// hit anywhere near this.
const MAX_SLUG_ATTEMPTS: u32 = 100;

/// Channel capacity for the in-process broadcast of `WorkspaceEvent`s.
/// The future `Streams` service (Task 24) consumes from a subscriber.
/// Sized to match the Task 19 spec.
const BROADCAST_CAPACITY: usize = 256;

/// Config for the actor's `run` loop. V0.1 has no knobs yet; the unit
/// struct keeps the `Actor::Config` slot occupied.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceManagerConfig;

/// Events published on workspace-state changes. Subscribers receive
/// these via [`WorkspaceManager::subscribe`]. V0.1 covers `Created` and
/// `Archived`; later events (e.g. permission changes) land at their
/// owning tasks.
#[derive(Debug, Clone)]
pub enum WorkspaceEvent {
    /// A new workspace was created. Payload is the persisted row.
    Created(Workspace),
    /// A workspace was archived. Payload is the workspace id.
    Archived(WorkspaceId),
    /// A workspace was restored from archive (Task 31). Payload is the
    /// post-restore row. Per `design/03 §3.7`, restoring a workspace
    /// only clears `workspaces.archived_at`; workareas remain
    /// individually archived.
    Restored(Workspace),
    /// A workspace's repository set was edited (Task 306,
    /// `design/03 §5.3` "repos updated"). Payload is the workspace row;
    /// subscribers re-read `workspace_repos` (ordered by `position`) for
    /// the new set.
    ReposUpdated(Workspace),
    /// A workspace's editable metadata (name/icon/description) changed.
    /// Payload is the post-update row. Repo-set edits use `ReposUpdated`.
    Updated(Workspace),
}

/// Cloneable handle to the Workspace Manager's shared state.
///
/// All meaningful work flows through this struct. The actor's `run`
/// merely parks on shutdown so future watchdog / config-reload hooks
/// have somewhere to land.
#[derive(Clone)]
pub struct WorkspaceManager {
    persistence: Arc<Persistence>,
    events: broadcast::Sender<WorkspaceEvent>,
    /// Optional Workarea Manager handle (Task 31). When `Some`, the
    /// cascading [`archive_workspace`] path drives each workarea's FS
    /// side effects (session stop + worktree teardown) through the
    /// workarea manager before stamping `workspaces.archived_at`. `None`
    /// in the in-process unit tests that don't need the cascade.
    workarea_manager: Option<crate::workspace_manager::WorkareaManager>,
    /// `<config_dir>` — used by Task 32's
    /// [`update_workspace_settings`] path to read `managed.json`.
    config_dir: Arc<PathBuf>,
    /// Task 44 audit writer. `None` in legacy callers (the tracing-only
    /// emission path stays in place for back-compat). When `Some`,
    /// state-changing methods also append typed events to the JSONL
    /// audit log.
    audit: Option<crate::audit::AuditWriter>,
}

impl WorkspaceManager {
    /// Build a fresh handle. Normally callers go through
    /// [`WorkspaceManagerActor::new`]; this is `pub` so tests can
    /// construct one without the supervisor.
    ///
    /// `config_dir` is `<config_dir>` (the directory hosting `core.pid`,
    /// `core.sock`, and `managed.json`). Task 32 uses it for the
    /// managed-policy cap.
    pub fn new(persistence: Arc<Persistence>, config_dir: Arc<PathBuf>) -> Self {
        let (events, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            persistence,
            events,
            workarea_manager: None,
            config_dir,
            audit: None,
        }
    }

    /// Attach a Task 44 [`crate::audit::AuditWriter`] so state-changing
    /// methods also flow through the JSONL audit log. Production wires
    /// this in `main.rs`; tests that don't care about audit emission
    /// can keep `audit = None`.
    pub fn with_audit(mut self, audit: crate::audit::AuditWriter) -> Self {
        self.audit = Some(audit);
        self
    }

    /// Attach a [`crate::workspace_manager::WorkareaManager`] so the
    /// cascading [`archive_workspace`] path can drive workarea FS side
    /// effects (session stop + worktree teardown). Production wires
    /// this in `main.rs`; tests can construct without it when they only
    /// exercise the workspace-level surface.
    pub fn with_workarea_manager(mut self, wam: crate::workspace_manager::WorkareaManager) -> Self {
        self.workarea_manager = Some(wam);
        self
    }

    /// Subscribe to `workspace.events`. The receiver lives in-process
    /// until the `Streams` gRPC service (Task 24) lands. Drop the
    /// receiver to unsubscribe.
    pub fn subscribe(&self) -> broadcast::Receiver<WorkspaceEvent> {
        self.events.subscribe()
    }

    /// Create a workspace.
    ///
    /// Validates the request, seeds per-(workspace, repo) cone snapshots
    /// (D4), persists `workspaces` + `workspace_repos` rows in one
    /// transaction, emits a [`WorkspaceEvent::Created`] on success, and
    /// returns the persisted row.
    pub async fn create_workspace(
        &self,
        name: &str,
        repos: &[WorkspaceRepoSpec],
        permission_mode: Option<String>,
        description: Option<String>,
        icon: Option<String>,
    ) -> Result<Workspace> {
        // ---- Validation ----------------------------------------------------
        if name.is_empty() {
            return Err(Error::Validation("name is required".into()));
        }
        let base_slug = derive_slug(name);
        if base_slug.is_empty() {
            return Err(Error::Validation(format!(
                "name {name:?} derives to an empty slug; pick a name with at least one ASCII alphanumeric"
            )));
        }
        if let Some(mode) = permission_mode.as_deref() {
            validate_permission_mode(mode)?;
        }

        // Resolve + seed the cone snapshots for the 1..N repo set. Validates
        // non-empty / no-dups / each exists in the global registry, and
        // seeds each per-(workspace, repo) snapshot from the repo's
        // `cone_defaults_json` when the spec's `sparse_cones` is empty (D4).
        // `seeded` preserves the caller's order (= the declaration order
        // persisted as `workspace_repos.position`).
        let seeded = self.seed_repo_cones(repos).await?;

        // ---- Persistence: one transaction, slug-collision retry ------------
        let now_ms = now_unix_ms();
        let id = WorkspaceId(uuid::Uuid::now_v7().to_string());

        let mut attempt: u32 = 1;
        let workspace = loop {
            let slug = if attempt == 1 {
                base_slug.clone()
            } else {
                truncate_slug(&format!("{base_slug}-{attempt}"))
            };
            let new_ws = NewWorkspace {
                id: id.clone(),
                name: name.to_string(),
                slug: slug.clone(),
                icon: icon.clone(),
                description: description.clone(),
                permission_mode: permission_mode.clone(),
                created_at: now_ms,
            };

            // Single transaction scope: workspace row + junction rows
            // commit atomically. We open a sqlx transaction on top of
            // the shared writer connection so a mid-flight failure
            // rolls back both inserts.
            let mut writer = self.persistence.writer().await;
            let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;

            match concerto_persist::workspaces::insert(&mut tx, new_ws.clone()).await {
                Ok(_) => {
                    concerto_persist::workspaces::update_repos(&mut tx, &id, &seeded).await?;
                    tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
                    drop(writer);
                    break Workspace {
                        id: id.clone(),
                        name: name.to_string(),
                        slug,
                        icon: icon.clone(),
                        description: description.clone(),
                        permission_mode: permission_mode.clone(),
                        created_at: now_ms,
                        archived_at: None,
                    };
                }
                Err(Error::Sqlx(boxed))
                    if concerto_persist::workspaces::is_unique_violation(&boxed) =>
                {
                    // Roll back and try a fresh suffix.
                    let _ = tx.rollback().await;
                    drop(writer);
                    attempt += 1;
                    if attempt > MAX_SLUG_ATTEMPTS {
                        return Err(Error::Internal(format!(
                            "exhausted {MAX_SLUG_ATTEMPTS} slug-suffix attempts for {base_slug:?}"
                        )));
                    }
                    continue;
                }
                Err(other) => {
                    let _ = tx.rollback().await;
                    return Err(other);
                }
            }
        };

        // Best-effort event emit: a closed channel (no subscribers)
        // returns Err; that's not a workspace-creation failure.
        let _ = self.events.send(WorkspaceEvent::Created(workspace.clone()));

        // Task 44: structured audit emission. The `tracing` channel
        // (via the WorkspaceEvent broadcast) keeps working for legacy
        // subscribers; this is the typed path that lands in the JSONL
        // file on disk.
        if let Some(audit) = self.audit.as_ref() {
            let rid_strs: Vec<String> = seeded.iter().map(|r| r.repository_id.0.clone()).collect();
            let details = serde_json::json!({
                "name": workspace.name,
                "slug": workspace.slug,
                "repository_ids": rid_strs,
                "permission_mode": workspace.permission_mode,
            });
            audit.append(
                crate::audit::AuditEvent::new(
                    crate::audit::AuditKind::WorkspaceCreated,
                    crate::audit::AuditActor::System,
                )
                .with_subject(crate::audit::EntityKind::Workspace, workspace.id.0.clone())
                .with_details(details),
            );
        }
        Ok(workspace)
    }

    /// Validate a workspace's 1..N repo set against the global registry and
    /// seed each per-(workspace, repo) cone snapshot (D4/D9).
    ///
    /// Checks, in order (returning the first failure for a good error):
    /// 1. **non-empty** — an empty set is rejected with
    ///    [`NO_REPOS_WIRE_CODE`] (`INVALID_ARGUMENT`).
    /// 2. **no duplicates** — a repeated id is rejected with
    ///    [`DUPLICATE_REPO_WIRE_CODE`] (`INVALID_ARGUMENT`).
    /// 3. **exists** — every id must exist in the global `repositories`
    ///    registry (else `NotFound`).
    ///
    /// For each spec, resolves the seeded cone JSON: an empty `sparse_cones`
    /// snapshots the repo's `cone_defaults_json` (D4); a non-empty list is
    /// serialized verbatim. Returns the [`WorkspaceRepoCones`] in caller
    /// order (= `workspace_repos.position`). Shared by
    /// [`Self::create_workspace`].
    async fn seed_repo_cones(
        &self,
        repos: &[WorkspaceRepoSpec],
    ) -> Result<Vec<WorkspaceRepoCones>> {
        if repos.is_empty() {
            return Err(Error::Validation(format!(
                "{NO_REPOS_WIRE_CODE}: a workspace must declare at least one repository"
            )));
        }
        // Reject duplicates (a repo listed twice).
        let mut seen = std::collections::HashSet::with_capacity(repos.len());
        for spec in repos {
            if !seen.insert(spec.repository_id.as_str()) {
                return Err(Error::Validation(format!(
                    "{DUPLICATE_REPO_WIRE_CODE}: repository {} is listed more than once",
                    spec.repository_id
                )));
            }
        }
        let mut out = Vec::with_capacity(repos.len());
        for spec in repos {
            let repo = concerto_persist::repositories::get(
                self.persistence.readers(),
                &spec.repository_id,
            )
            .await?
            .ok_or_else(|| {
                Error::NotFound(format!(
                    "repository {} not found in the registry",
                    spec.repository_id
                ))
            })?;
            // D4: empty spec → snapshot the repo's current cone defaults;
            // else serialize the explicit list verbatim.
            let sparse_cones_json = if spec.sparse_cones.is_empty() {
                repo.cone_defaults_json.clone()
            } else {
                serde_json::to_string(&spec.sparse_cones).map_err(|e| {
                    Error::Internal(format!(
                        "serialize sparse_cones for {}: {e}",
                        spec.repository_id
                    ))
                })?
            };
            out.push(WorkspaceRepoCones {
                repository_id: spec.repository_id.clone(),
                sparse_cones_json,
            });
        }
        Ok(out)
    }

    /// Replace a workspace's repository set (`design/03 §5.1`, §6.1 —
    /// "edit the repo list").
    ///
    /// Re-validates the set (non-empty / no-dups / each exists in the global
    /// registry), then re-positions it: `workspace_repos.position` = the
    /// index of each id in `repos` (declaration order). For each repo it
    /// PRESERVES an existing per-(workspace, repo) snapshot when the repo was
    /// already attached (read via `get_repo_cones`); otherwise it seeds from
    /// the repo's `cone_defaults_json` (D4). Emits a `repos updated` event on
    /// the `workspace.events` broadcast.
    ///
    /// Note: this edits the *declared* repo set; it materializes nothing
    /// on disk. Existing workareas keep their already-materialized
    /// worktrees; the new set takes effect for workareas created after.
    pub async fn update_workspace_repos(
        &self,
        id: &WorkspaceId,
        repos: &[WorkspaceRepoSpec],
    ) -> Result<()> {
        let workspace = self
            .get(id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("workspace {id} not found")))?;

        // Seed the base (validate + per-spec snapshot from repo defaults).
        let mut seeded = self.seed_repo_cones(repos).await?;
        // Preserve any existing snapshot for a repo already attached to this
        // workspace (the snapshot is sticky once seeded, D4) — but only when
        // the caller did not pass an explicit cone list for that repo.
        for (idx, spec) in repos.iter().enumerate() {
            if spec.sparse_cones.is_empty() {
                if let Some(existing) = concerto_persist::workspaces::get_repo_cones(
                    self.persistence.readers(),
                    id,
                    &spec.repository_id,
                )
                .await?
                {
                    seeded[idx].sparse_cones_json = existing;
                }
            }
        }

        let mut writer = self.persistence.writer().await;
        let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        concerto_persist::workspaces::update_repos(&mut tx, id, &seeded).await?;
        tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        drop(writer);

        // `workspace.events: repos updated` (`design/03 §5.3`). The payload
        // carries the post-update workspace row, mirroring `Created` /
        // `Restored`.
        let _ = self.events.send(WorkspaceEvent::ReposUpdated(workspace));
        Ok(())
    }

    /// Edit a workspace's metadata (name/icon/description) and/or replace
    /// its repo set. `name`/`icon`/`description` use `Option`: `None` =
    /// leave unchanged. For icon/description the inner `Option` selects
    /// set-vs-clear (`Some(Some(v))` set, `Some(None)` clear). `repos`
    /// empty = leave the repo set unchanged; non-empty = replace via
    /// [`update_workspace_repos`].
    ///
    /// Slug is never re-derived (it is the stable handle from creation).
    /// Repo-set edits affect FUTURE workareas only — existing workareas keep
    /// their materialized worktrees (see [`update_workspace_repos`]).
    pub async fn update_workspace(
        &self,
        id: &WorkspaceId,
        name: Option<String>,
        icon: Option<Option<String>>,
        description: Option<Option<String>>,
        repos: &[WorkspaceRepoSpec],
    ) -> Result<Workspace> {
        if self.get(id).await?.is_none() {
            return Err(Error::NotFound(format!("workspace {id} not found")));
        }
        if let Some(n) = name.as_deref() {
            if n.is_empty() {
                return Err(Error::Validation("name must not be empty".into()));
            }
        }

        let has_metadata = name.is_some() || icon.is_some() || description.is_some();
        if has_metadata {
            let mut writer = self.persistence.writer().await;
            let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
            concerto_persist::workspaces::set_metadata(
                &mut tx,
                id,
                name.as_deref(),
                icon.as_ref().map(|o| o.as_deref()),
                description.as_ref().map(|o| o.as_deref()),
            )
            .await?;
            tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
            drop(writer);
        }

        let repos_changed = !repos.is_empty();
        if repos_changed {
            self.update_workspace_repos(id, repos).await?;
        }

        let updated = self
            .get(id)
            .await?
            .ok_or_else(|| Error::Internal(format!("workspace {id} vanished mid-update")))?;

        if has_metadata && !repos_changed {
            let _ = self.events.send(WorkspaceEvent::Updated(updated.clone()));
        }
        Ok(updated)
    }

    /// List a workspace's declared repos with their per-(workspace, repo)
    /// cone snapshots, position-ordered (for the edit form pre-fill).
    pub async fn list_workspace_repos(
        &self,
        id: &WorkspaceId,
    ) -> Result<Vec<WorkspaceRepoSpec>> {
        let pairs =
            concerto_persist::workspaces::list_repo_cones(self.persistence.readers(), id).await?;
        let mut out = Vec::with_capacity(pairs.len());
        for (repo_id, cones_json) in pairs {
            let sparse_cones: Vec<String> = serde_json::from_str(&cones_json).map_err(|e| {
                Error::Internal(format!("parse sparse_cones for {}: {e}", repo_id.0))
            })?;
            out.push(WorkspaceRepoSpec { repository_id: repo_id, sparse_cones });
        }
        Ok(out)
    }

    /// Look up a workspace by id.
    pub async fn get(&self, id: &WorkspaceId) -> Result<Option<Workspace>> {
        concerto_persist::workspaces::get(self.persistence.readers(), id).await
    }

    /// List every workspace (the registry is global after the collapse).
    pub async fn list_all(&self) -> Result<Vec<Workspace>> {
        concerto_persist::workspaces::list_all(self.persistence.readers()).await
    }

    /// Archive a workspace + cascade to every non-archived workarea
    /// (Task 31, per `design/03 §3.7`).
    ///
    /// Steps:
    /// 1. List workareas with `archived_at IS NULL AND workspace_id = id`.
    /// 2. For each: drive the FS side effects (stop live sessions, keep
    ///    the worktree on disk per R-5).
    /// 3. In one writer transaction: stamp `archived_at = now` on every
    ///    listed workarea row AND on the workspace row.
    /// 4. Emit [`WorkspaceEvent::Archived`].
    ///
    /// Idempotent: archiving a workspace with no non-archived workareas
    /// only stamps the workspace row.
    ///
    /// Backwards-compatible: the previous Task 19 signature was
    /// `archive(id) -> Result<()>` with no cascade; the new behaviour is
    /// the cascading variant. Tests that wired Task 19 directly continue
    /// to work because the workspace row UPDATE is still issued.
    pub async fn archive(&self, id: &WorkspaceId) -> Result<()> {
        // Sanity-check existence so we return NotFound instead of a
        // silent UPDATE-zero-rows.
        if self.get(id).await?.is_none() {
            return Err(Error::NotFound(format!("workspace {id} not found")));
        }

        // Enumerate non-archived workareas in this workspace.
        let workareas =
            concerto_persist::workareas::list_non_archived_minimal(self.persistence.readers(), id)
                .await?;

        // Drive FS side effects per workarea (best-effort). The default
        // ArchiveOpts keep worktrees on disk per design R-5.
        if let Some(wam) = self.workarea_manager.as_ref() {
            for (wa_id, worktree_root, _branch) in &workareas {
                let path = std::path::PathBuf::from(worktree_root);
                if let Err(e) = wam
                    .archive_workarea_side_effects(
                        wa_id,
                        &path,
                        crate::workspace_manager::ArchiveOpts::default(),
                    )
                    .await
                {
                    tracing::warn!(
                        workarea = %wa_id,
                        error = %e,
                        "archive_workarea_side_effects failed during cascade; continuing"
                    );
                }
            }
        }

        // One transaction: every workarea row + the workspace row.
        let wa_ids: Vec<concerto_persist::WorkareaId> =
            workareas.into_iter().map(|(id, _, _)| id).collect();
        let now_ms = now_unix_ms();
        crate::workspace_manager::archive::archive_workspace_tx(
            &self.persistence,
            id,
            &wa_ids,
            now_ms,
        )
        .await?;

        // Re-emit per-workarea archived events from the cascade so
        // streams subscribers see the same shape as a single-archive.
        if let Some(wam) = self.workarea_manager.as_ref() {
            for wa_id in &wa_ids {
                let _ = wam.publish_archived(wa_id.clone());
            }
        }

        let _ = self.events.send(WorkspaceEvent::Archived(id.clone()));
        Ok(())
    }

    /// Restore an archived workspace (Task 31).
    ///
    /// Clears `workspaces.archived_at` only. Workareas remain
    /// individually archived per `design/03 §3.7`; the user restores
    /// each one explicitly.
    pub async fn restore_workspace(&self, id: &WorkspaceId) -> Result<Workspace> {
        if self.get(id).await?.is_none() {
            return Err(Error::NotFound(format!("workspace {id} not found")));
        }
        let mut writer = self.persistence.writer().await;
        let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        concerto_persist::workspaces::restore(&mut tx, id).await?;
        tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        drop(writer);
        let restored = self
            .get(id)
            .await?
            .ok_or_else(|| Error::Internal(format!("workspace {id} vanished mid-restore")))?;
        let _ = self.events.send(WorkspaceEvent::Restored(restored.clone()));
        Ok(restored)
    }

    /// List repository ids attached to a workspace.
    pub async fn list_repos(&self, workspace_id: &WorkspaceId) -> Result<Vec<RepositoryId>> {
        concerto_persist::workspaces::list_repos(self.persistence.readers(), workspace_id).await
    }

    /// Task 32: patch the mutable subset of `workspaces.*` (V0.1:
    /// `permission_mode`).
    ///
    /// `permission_mode = Some(mode)` sets the column to the lowercase
    /// SQL string; `Some` of an empty string or `None` clears the column
    /// (inherit-from-workspace-defaults). The managed-policy cap (`managed.json`)
    /// is enforced: requesting `yolo` when the cap is `auto` returns
    /// [`Error::PolicyLocked`] / `PERMISSION_DENIED`.
    ///
    /// Emits a `tracing::info!` audit event with
    /// `audit.kind = "permission_mode_changed"`, `audit.scope =
    /// "workspace"`, and the from→to transition. Task 44 promotes the
    /// emission to the JSONL audit log; the field set is the same.
    pub async fn update_workspace_settings(
        &self,
        id: &WorkspaceId,
        permission_mode: Option<Option<String>>,
    ) -> Result<Workspace> {
        let existing = self
            .get(id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("workspace {id} not found")))?;

        if let Some(req) = permission_mode.as_ref() {
            // Validate + cap.
            let new_mode_str: Option<String> = match req {
                Some(s) => {
                    let parsed = crate::security::parse_permission_mode(s)?;
                    let managed = crate::security::load_managed_policy(&self.config_dir)?;
                    let _capped =
                        crate::security::permission::enforce_managed_cap(parsed, &managed)?;
                    Some(parsed.as_str().to_string())
                }
                None => None,
            };

            let mut writer = self.persistence.writer().await;
            let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
            concerto_persist::workspaces::set_permission_mode(&mut tx, id, new_mode_str.as_deref())
                .await?;
            tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
            drop(writer);

            tracing::info!(
                audit.kind = "permission_mode_changed",
                audit.scope = "workspace",
                audit.workspace_id = %id,
                audit.from = %existing.permission_mode.as_deref().unwrap_or("inherit"),
                audit.to = %new_mode_str.as_deref().unwrap_or("inherit"),
                "workspace permission_mode changed"
            );

            // Task 44: typed audit event mirrors the tracing emission.
            if let Some(audit) = self.audit.as_ref() {
                let details = serde_json::json!({
                    "scope": "workspace",
                    "from": existing.permission_mode.as_deref().unwrap_or("inherit"),
                    "to": new_mode_str.as_deref().unwrap_or("inherit"),
                });
                audit.append(
                    crate::audit::AuditEvent::new(
                        crate::audit::AuditKind::PermissionModeChanged,
                        crate::audit::AuditActor::System,
                    )
                    .with_subject(crate::audit::EntityKind::Workspace, id.0.clone())
                    .with_details(details),
                );
            }
        }

        self.get(id)
            .await?
            .ok_or_else(|| Error::Internal(format!("workspace {id} vanished mid-update")))
    }
}

/// Supervised actor wrapper. Holds the shared [`WorkspaceManager`] handle.
pub struct WorkspaceManagerActor {
    handle: WorkspaceManager,
}

impl WorkspaceManagerActor {
    /// Build a new actor. The handle is constructed eagerly so callers
    /// can grab a clone before spawning.
    pub fn new(persistence: Arc<Persistence>, config_dir: Arc<PathBuf>) -> Self {
        Self {
            handle: WorkspaceManager::new(persistence, config_dir),
        }
    }

    /// Cheap clone of the shared handle.
    pub fn handle(&self) -> WorkspaceManager {
        self.handle.clone()
    }
}

#[async_trait]
impl Actor for WorkspaceManagerActor {
    const NAME: &'static str = "workspace-manager";
    type Config = WorkspaceManagerConfig;

    async fn run(self, ctx: ActorContext<Self::Config>) -> Result<()> {
        tracing::info!("WorkspaceManager ready");
        ctx.shutdown.cancelled().await;
        tracing::debug!("WorkspaceManager actor shutting down");
        Ok(())
    }
}

/// Derive a URL-safe slug from `name`.
///
/// Algorithm (frozen by Task 19): lowercase ASCII, replace whitespace
/// runs with `-`, strip every byte outside `[a-z0-9-]`, collapse
/// consecutive `-`, trim leading/trailing `-`, truncate to 64 chars.
///
/// This is deliberately inlined; the design note about
/// `slug::slugify` being overkill applies — 20 lines, no extra dep.
pub fn derive_slug(name: &str) -> String {
    let lowered = name.to_ascii_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut last_was_dash = false;
    for c in lowered.chars() {
        let mapped = if c.is_ascii_alphanumeric() {
            Some(c)
        } else if c.is_whitespace() || c == '_' || c == '-' || c == '.' || c == '/' {
            Some('-')
        } else {
            None
        };
        match mapped {
            Some('-') if !last_was_dash && !out.is_empty() => {
                out.push('-');
                last_was_dash = true;
            }
            Some('-') => {}
            Some(c) => {
                out.push(c);
                last_was_dash = false;
            }
            None => {}
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    truncate_slug(&out)
}

/// Truncate a slug to 64 bytes and strip any trailing `-`. Safe because
/// the byte set is ASCII.
fn truncate_slug(s: &str) -> String {
    let mut t = if s.len() > 64 {
        s[..64].to_string()
    } else {
        s.to_string()
    };
    while t.ends_with('-') {
        t.pop();
    }
    t
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `WorkspaceManager` over a fresh tempdir SQLite DB plus N
    /// repos inserted directly into the global registry, and return the
    /// manager + the repo ids.
    async fn seed_manager(
        repo_names: &[&str],
    ) -> (tempfile::TempDir, WorkspaceManager, Vec<String>) {
        use concerto_persist::{NewRepository, Persistence, PersistenceConfig, RepositoryId};

        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persistence::open(PersistenceConfig {
            db_path: dir.path().join("test.db"),
            max_readers: 2,
        })
        .await
        .expect("open");

        let mut repo_ids = Vec::new();
        {
            let mut w = persist.writer().await;
            for name in repo_names {
                let rid = format!("repo-{name}");
                concerto_persist::repositories::insert(
                    &mut w,
                    NewRepository {
                        id: RepositoryId(rid.clone()),
                        name: name.to_string(),
                        url: format!("file:///tmp/{name}.git"),
                        local_path: format!("/tmp/repos/{name}"),
                        clone_strategy: "full".to_string(),
                        default_branch: "main".to_string(),
                    },
                )
                .await
                .expect("insert repo");
                repo_ids.push(rid);
            }
        }

        let manager = WorkspaceManager::new(Arc::new(persist), Arc::new(dir.path().to_path_buf()));
        (dir, manager, repo_ids)
    }

    /// Build a `WorkspaceRepoSpec` set with empty cones (seed-from-defaults)
    /// for the given repo ids, in order.
    fn specs(repo_ids: &[String]) -> Vec<WorkspaceRepoSpec> {
        repo_ids
            .iter()
            .map(|rid| WorkspaceRepoSpec {
                repository_id: RepositoryId(rid.clone()),
                sparse_cones: Vec::new(),
            })
            .collect()
    }

    #[tokio::test]
    async fn create_workspace_accepts_multi_repo_in_declaration_order() {
        let (_dir, mgr, repos) = seed_manager(&["api", "android", "ios"]).await;
        let ws = mgr
            .create_workspace("Cross Platform", &specs(&repos), None, None, None)
            .await
            .expect("multi-repo create");
        let listed = mgr.list_repos(&ws.id).await.expect("list_repos");
        let listed_strs: Vec<String> = listed.iter().map(|r| r.0.clone()).collect();
        assert_eq!(listed_strs, repos, "repos returned in declaration order");
    }

    #[tokio::test]
    async fn create_workspace_rejects_empty_dup_and_foreign() {
        let (_dir, mgr, repos) = seed_manager(&["api"]).await;

        // Empty.
        let empty = mgr
            .create_workspace("Empty", &[], None, None, None)
            .await
            .expect_err("empty set rejected");
        assert!(matches!(empty, Error::Validation(m) if m.contains(NO_REPOS_WIRE_CODE)));

        // Duplicate.
        let dup = mgr
            .create_workspace(
                "Dup",
                &specs(&[repos[0].clone(), repos[0].clone()]),
                None,
                None,
                None,
            )
            .await
            .expect_err("duplicate rejected");
        assert!(matches!(dup, Error::Validation(m) if m.contains(DUPLICATE_REPO_WIRE_CODE)));

        // Foreign / unknown.
        let foreign = mgr
            .create_workspace("Foreign", &specs(&["nope".to_string()]), None, None, None)
            .await
            .expect_err("foreign rejected");
        assert!(matches!(foreign, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn update_workspace_repos_revalidates_and_repositions() {
        let (_dir, mgr, repos) = seed_manager(&["api", "android", "ios"]).await;
        let ws = mgr
            .create_workspace("WS", &specs(&repos), None, None, None)
            .await
            .expect("create");

        // Reorder + reduce to [ios, api].
        let reordered = vec![repos[2].clone(), repos[0].clone()];
        mgr.update_workspace_repos(&ws.id, &specs(&reordered))
            .await
            .expect("update_workspace_repos");
        let listed: Vec<String> = mgr
            .list_repos(&ws.id)
            .await
            .expect("list_repos")
            .iter()
            .map(|r| r.0.clone())
            .collect();
        assert_eq!(listed, reordered, "set re-positioned in new order");

        // Re-validation still applies: empty + dup rejected.
        assert!(matches!(
            mgr.update_workspace_repos(&ws.id, &[]).await,
            Err(Error::Validation(_))
        ));
        assert!(matches!(
            mgr.update_workspace_repos(&ws.id, &specs(&[repos[0].clone(), repos[0].clone()]))
                .await,
            Err(Error::Validation(_))
        ));
    }

    #[tokio::test]
    async fn update_workspace_changes_metadata_and_repos() {
        fn spec(repo_id: &str) -> WorkspaceRepoSpec {
            WorkspaceRepoSpec {
                repository_id: RepositoryId(repo_id.to_string()),
                sparse_cones: vec![],
            }
        }

        let (_dir, mgr, repos) = seed_manager(&["a", "b"]).await;
        let repo_a = repos[0].clone();
        let repo_b = repos[1].clone();

        let ws = mgr
            .create_workspace("Original", &[spec(&repo_a)], None, None, None)
            .await
            .expect("create");

        let updated = mgr
            .update_workspace(
                &ws.id,
                Some("Renamed".to_string()),
                Some(Some("🚀".to_string())),
                None,
                &[spec(&repo_a), spec(&repo_b)],
            )
            .await
            .unwrap();
        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.icon.as_deref(), Some("🚀"));
        assert_eq!(updated.slug, ws.slug); // slug immutable

        let repos = mgr.list_workspace_repos(&ws.id).await.unwrap();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].repository_id.0, repo_a);
        assert_eq!(repos[1].repository_id.0, repo_b);
    }

    #[tokio::test]
    async fn create_workspace_seeds_repo_cone_defaults_as_snapshot() {
        // A repo with cone_defaults=["api/"], attached with an empty spec,
        // seeds the workspace snapshot ["api/"]; editing the repo default
        // afterward does NOT mutate the existing workspace snapshot (D4).
        let (_dir, mgr, repos) = seed_manager(&["api"]).await;
        let repo_id = RepositoryId(repos[0].clone());

        // Set the repo's cone_defaults to ["api/"].
        {
            let mut w = mgr.persistence.writer().await;
            concerto_persist::repositories::set_cone_defaults(
                &mut w,
                &repo_id,
                &["api/".to_string()],
            )
            .await
            .expect("set cone defaults");
        }

        // Attach with an empty spec → snapshot seeded from the repo default.
        let ws = mgr
            .create_workspace("Snap", &specs(&repos), None, None, None)
            .await
            .expect("create");
        let snap = concerto_persist::workspaces::get_repo_cones(
            mgr.persistence.readers(),
            &ws.id,
            &repo_id,
        )
        .await
        .expect("get_repo_cones")
        .expect("snapshot present");
        assert_eq!(snap, r#"["api/"]"#, "snapshot seeded from repo defaults");

        // Edit the repo default to ["web/"] — the workspace snapshot must NOT
        // change (D4: snapshots are sticky).
        {
            let mut w = mgr.persistence.writer().await;
            concerto_persist::repositories::set_cone_defaults(
                &mut w,
                &repo_id,
                &["web/".to_string()],
            )
            .await
            .expect("edit cone defaults");
        }
        let snap_after = concerto_persist::workspaces::get_repo_cones(
            mgr.persistence.readers(),
            &ws.id,
            &repo_id,
        )
        .await
        .expect("get_repo_cones")
        .expect("snapshot present");
        assert_eq!(
            snap_after, r#"["api/"]"#,
            "editing the repo default does not mutate an existing workspace snapshot (D4)"
        );
    }

    #[test]
    fn slug_basic() {
        assert_eq!(derive_slug("Hello World"), "hello-world");
    }

    #[test]
    fn slug_strips_punct() {
        assert_eq!(derive_slug("Fix: Login bug!!!"), "fix-login-bug");
    }

    #[test]
    fn slug_collapses_runs() {
        assert_eq!(derive_slug("hello   world"), "hello-world");
        assert_eq!(derive_slug("hello---world"), "hello-world");
    }

    #[test]
    fn slug_truncates_to_64() {
        let s = derive_slug(&"a".repeat(100));
        assert_eq!(s.len(), 64);
    }

    #[test]
    fn slug_empty_for_punct_only() {
        assert_eq!(derive_slug("!!!"), "");
    }

    #[test]
    fn slug_handles_underscores_and_slashes() {
        assert_eq!(derive_slug("feature/login_v2"), "feature-login-v2");
    }
}
