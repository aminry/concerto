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
//! ## Contract (Task 19, relaxed to 1..N by Task 306)
//!
//! - `create_workspace` accepts **1..N** repositories (Task 306 dropped
//!   the V0.1 single-repo guard). It rejects an empty set with
//!   [`NO_REPOS_WIRE_CODE`] and a repeated id with
//!   [`DUPLICATE_REPO_WIRE_CODE`], both inside an [`Error::Validation`]
//!   the gRPC handler maps to `INVALID_ARGUMENT`. [`SINGLE_REPO_WIRE_CODE`]
//!   is retired as an active rejection (kept defined for one release for
//!   client back-compat; no code path emits it).
//! - `name` must derive to a non-empty slug.
//! - `project_id` must exist in `projects`.
//! - Every `repository_id` must exist and belong to that `project_id`.
//! - `update_workspace_repos` re-validates + re-positions the set and
//!   emits `workspace.events: repos updated` (`design/03 §5.1`, §6.1).
//! - Slug collisions inside a project auto-suffix `-2`, `-3`, … with a
//!   bound on retries to stop a runaway loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use concerto_error::{Error, Result};
use concerto_persist::{
    NewWorkspace, Persistence, Repository, RepositoryId, Workspace, WorkspaceId,
};
use sqlx::Connection;
use tokio::sync::broadcast;

use crate::supervisor::{Actor, ActorContext};

/// Wire-code the V0.1 Core surfaced when a caller requested a multi-repo
/// workspace. **Retired by Task 306** — multi-repo workspaces are now
/// supported, so no code path emits this. Kept defined for one release so
/// clients still switching on the string compile; remove once every
/// client has migrated.
#[deprecated(
    since = "1.0.0",
    note = "multi-repo workspaces are supported as of Task 306; this rejection no longer fires"
)]
pub const SINGLE_REPO_WIRE_CODE: &str = "workspace.v0_single_repo_only";

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
    /// Validates the request, persists `workspaces` + `workspace_repos`
    /// rows in one transaction, emits a [`WorkspaceEvent::Created`] on
    /// success, and returns the persisted row.
    pub async fn create_workspace(
        &self,
        project_id: &str,
        name: &str,
        repository_ids: &[String],
        permission_mode: Option<String>,
        description: Option<String>,
    ) -> Result<Workspace> {
        // ---- Validation ----------------------------------------------------
        if project_id.is_empty() {
            return Err(Error::Validation("project_id is required".into()));
        }
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

        // Project must exist.
        let project = concerto_persist::projects::get(
            self.persistence.readers(),
            &concerto_persist::ProjectId(project_id.to_string()),
        )
        .await?;
        if project.is_none() {
            return Err(Error::NotFound(format!("project {project_id} not found")));
        }

        // Validate the 1..N repo set: non-empty, no dups, each exists +
        // belongs to the project. `repo_ids` preserves the caller's order
        // (= the declaration order persisted as `workspace_repos.position`).
        let repo_ids = self
            .validate_workspace_repos(project_id, repository_ids)
            .await?;

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
                project_id: project_id.to_string(),
                name: name.to_string(),
                slug: slug.clone(),
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
                    concerto_persist::workspaces::update_repos(&mut tx, &id, &repo_ids).await?;
                    tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
                    drop(writer);
                    break Workspace {
                        id: id.clone(),
                        project_id: project_id.to_string(),
                        name: name.to_string(),
                        slug,
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
                            "exhausted {MAX_SLUG_ATTEMPTS} slug-suffix attempts for {base_slug:?} in project {project_id}"
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
            let rid_strs: Vec<String> = repo_ids.iter().map(|r| r.0.clone()).collect();
            let details = serde_json::json!({
                "name": workspace.name,
                "slug": workspace.slug,
                "project_id": workspace.project_id,
                "repository_ids": rid_strs,
                "permission_mode": workspace.permission_mode,
            });
            audit.append(
                crate::audit::AuditEvent::new(
                    crate::audit::AuditKind::WorkspaceCreated,
                    crate::audit::AuditActor::System,
                )
                .with_subject(crate::audit::EntityKind::Workspace, workspace.id.0.clone())
                .with_subject(
                    crate::audit::EntityKind::Project,
                    workspace.project_id.clone(),
                )
                .with_details(details),
            );
        }
        Ok(workspace)
    }

    /// Validate a workspace's 1..N repository set (Task 306).
    ///
    /// Checks, in order (returning the first failure for a good error):
    /// 1. **non-empty** — an empty set is rejected with
    ///    [`NO_REPOS_WIRE_CODE`] (`INVALID_ARGUMENT`).
    /// 2. **no duplicates** — a repeated id is rejected with
    ///    [`DUPLICATE_REPO_WIRE_CODE`] (`INVALID_ARGUMENT`).
    /// 3. **exists + belongs** — every id must exist in `repositories`
    ///    AND have `project_id == project_id` (else `NotFound`).
    ///
    /// Returns the resolved [`RepositoryId`]s **in caller order**, which
    /// is the declaration order persisted as `workspace_repos.position`.
    /// Shared by [`Self::create_workspace`] and
    /// [`Self::update_workspace_repos`].
    async fn validate_workspace_repos(
        &self,
        project_id: &str,
        repository_ids: &[String],
    ) -> Result<Vec<RepositoryId>> {
        if repository_ids.is_empty() {
            return Err(Error::Validation(format!(
                "{NO_REPOS_WIRE_CODE}: a workspace must declare at least one repository"
            )));
        }
        // Reject duplicates (a repo listed twice).
        let mut seen = std::collections::HashSet::with_capacity(repository_ids.len());
        for rid in repository_ids {
            if !seen.insert(rid.as_str()) {
                return Err(Error::Validation(format!(
                    "{DUPLICATE_REPO_WIRE_CODE}: repository {rid} is listed more than once"
                )));
            }
        }
        // Each must exist and belong to the project.
        let repos =
            concerto_persist::repositories::list_by_project(self.persistence.readers(), project_id)
                .await?;
        let mut repo_ids = Vec::with_capacity(repository_ids.len());
        for rid in repository_ids {
            match repos.iter().find(|r: &&Repository| r.id.as_str() == rid) {
                Some(r) => repo_ids.push(r.id.clone()),
                None => {
                    return Err(Error::NotFound(format!(
                        "repository {rid} not found in project {project_id}"
                    )));
                }
            }
        }
        Ok(repo_ids)
    }

    /// Replace a workspace's repository set (`design/03 §5.1`, §6.1 —
    /// "edit the repo list"; Task 306).
    ///
    /// Re-validates the set (non-empty / no-dups / each exists + belongs
    /// to the workspace's project) via [`Self::validate_workspace_repos`],
    /// then re-positions it: `workspace_repos.position` = the index of
    /// each id in `repository_ids` (declaration order). Emits a
    /// [`WorkspaceEvent::Created`]-sibling `repos updated` event on the
    /// `workspace.events` broadcast.
    ///
    /// Note: this edits the *declared* repo set; it materializes nothing
    /// on disk. Existing workareas keep their already-materialized
    /// worktrees; the new set takes effect for workareas created after.
    pub async fn update_workspace_repos(
        &self,
        id: &WorkspaceId,
        repository_ids: &[String],
    ) -> Result<()> {
        let workspace = self
            .get(id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("workspace {id} not found")))?;

        let repo_ids = self
            .validate_workspace_repos(&workspace.project_id, repository_ids)
            .await?;

        let mut writer = self.persistence.writer().await;
        let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        concerto_persist::workspaces::update_repos(&mut tx, id, &repo_ids).await?;
        tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        drop(writer);

        // `workspace.events: repos updated` (`design/03 §5.3`). The
        // payload carries the post-update workspace row, mirroring
        // `Created` / `Restored`. (A typed audit-log `AuditKind` for repo
        // edits is deferred — `audit/event.rs` is out of this task's
        // Outputs; the broadcast event is the §5.3 surface.)
        let _ = self.events.send(WorkspaceEvent::ReposUpdated(workspace));
        Ok(())
    }

    /// Look up a workspace by id.
    pub async fn get(&self, id: &WorkspaceId) -> Result<Option<Workspace>> {
        concerto_persist::workspaces::get(self.persistence.readers(), id).await
    }

    /// List workspaces in a project.
    pub async fn list_by_project(&self, project_id: &str) -> Result<Vec<Workspace>> {
        concerto_persist::workspaces::list_by_project(self.persistence.readers(), project_id).await
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
    /// (inherit-from-project). The managed-policy cap (`managed.json`)
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

// Suppress an unused-import lint when no callers reach for PathBuf in
// V0.1 (the type is referenced in design but not yet wired through).
#[allow(dead_code)]
fn _path_marker(_p: PathBuf) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `WorkspaceManager` over a fresh tempdir SQLite DB plus a
    /// seeded project + N repos, and return the manager, the project id,
    /// and the repo ids (Task 306 helper).
    async fn seed_manager(
        repo_names: &[&str],
    ) -> (tempfile::TempDir, WorkspaceManager, String, Vec<String>) {
        use concerto_persist::{
            NewProject, NewRepository, Persistence, PersistenceConfig, ProjectId, RepositoryId,
        };

        let dir = tempfile::tempdir().expect("tempdir");
        let persist = Persistence::open(PersistenceConfig {
            db_path: dir.path().join("test.db"),
            max_readers: 2,
        })
        .await
        .expect("open");

        let project_id = "proj-1".to_string();
        let mut repo_ids = Vec::new();
        {
            let mut w = persist.writer().await;
            concerto_persist::projects::insert(
                &mut w,
                NewProject {
                    id: ProjectId(project_id.clone()),
                    name: "Test".to_string(),
                    icon: None,
                    created_at: 1,
                },
            )
            .await
            .expect("insert project");
            for name in repo_names {
                let rid = format!("repo-{name}");
                concerto_persist::repositories::insert(
                    &mut w,
                    NewRepository {
                        id: RepositoryId(rid.clone()),
                        project_id: project_id.clone(),
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
        (dir, manager, project_id, repo_ids)
    }

    #[tokio::test]
    async fn create_workspace_accepts_multi_repo_in_declaration_order() {
        let (_dir, mgr, project_id, repos) = seed_manager(&["api", "android", "ios"]).await;
        let ws = mgr
            .create_workspace(&project_id, "Cross Platform", &repos, None, None)
            .await
            .expect("multi-repo create");
        let listed = mgr.list_repos(&ws.id).await.expect("list_repos");
        let listed_strs: Vec<String> = listed.iter().map(|r| r.0.clone()).collect();
        assert_eq!(listed_strs, repos, "repos returned in declaration order");
    }

    #[tokio::test]
    async fn create_workspace_rejects_empty_dup_and_foreign() {
        let (_dir, mgr, project_id, repos) = seed_manager(&["api"]).await;

        // Empty.
        let empty = mgr
            .create_workspace(&project_id, "Empty", &[], None, None)
            .await
            .expect_err("empty set rejected");
        assert!(matches!(empty, Error::Validation(m) if m.contains(NO_REPOS_WIRE_CODE)));

        // Duplicate.
        let dup = mgr
            .create_workspace(
                &project_id,
                "Dup",
                &[repos[0].clone(), repos[0].clone()],
                None,
                None,
            )
            .await
            .expect_err("duplicate rejected");
        assert!(matches!(dup, Error::Validation(m) if m.contains(DUPLICATE_REPO_WIRE_CODE)));

        // Foreign / unknown.
        let foreign = mgr
            .create_workspace(&project_id, "Foreign", &["nope".to_string()], None, None)
            .await
            .expect_err("foreign rejected");
        assert!(matches!(foreign, Error::NotFound(_)));
    }

    #[tokio::test]
    async fn update_workspace_repos_revalidates_and_repositions() {
        let (_dir, mgr, project_id, repos) = seed_manager(&["api", "android", "ios"]).await;
        let ws = mgr
            .create_workspace(&project_id, "WS", &repos, None, None)
            .await
            .expect("create");

        // Reorder + reduce to [ios, api].
        let reordered = vec![repos[2].clone(), repos[0].clone()];
        mgr.update_workspace_repos(&ws.id, &reordered)
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
            mgr.update_workspace_repos(&ws.id, &[repos[0].clone(), repos[0].clone()])
                .await,
            Err(Error::Validation(_))
        ));
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
