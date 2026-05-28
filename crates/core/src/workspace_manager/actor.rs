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
//! ## V0.1 contract (locked by Task 19)
//!
//! - `create_workspace` enforces `repository_ids.len() == 1` and
//!   returns [`SINGLE_REPO_WIRE_CODE`] inside an [`Error::Validation`]
//!   if violated. The gRPC handler maps this to `INVALID_ARGUMENT`.
//! - `name` must derive to a non-empty slug.
//! - `project_id` must exist in `projects`.
//! - Every `repository_id` must exist and belong to that `project_id`.
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

/// Wire-code surfaced inside the [`Error::Validation`] payload when a
/// caller requests a multi-repo workspace in V0.1. The handler maps
/// this to `INVALID_ARGUMENT` and ships it in
/// `ConcertoError.code` for clients to switch on.
pub const SINGLE_REPO_WIRE_CODE: &str = "workspace.v0_single_repo_only";

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
}

impl WorkspaceManager {
    /// Build a fresh handle. Normally callers go through
    /// [`WorkspaceManagerActor::new`]; this is `pub` so tests can
    /// construct one without the supervisor.
    pub fn new(persistence: Arc<Persistence>) -> Self {
        let (events, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            persistence,
            events,
        }
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
        if repository_ids.len() != 1 {
            return Err(Error::Validation(format!(
                "{SINGLE_REPO_WIRE_CODE}: V0.1 supports exactly one repository per workspace; got {}",
                repository_ids.len()
            )));
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

        // Repositories must exist and belong to the project.
        let repos =
            concerto_persist::repositories::list_by_project(self.persistence.readers(), project_id)
                .await?;
        let mut repo_ids = Vec::with_capacity(repository_ids.len());
        for rid in repository_ids {
            let row = repos.iter().find(|r: &&Repository| r.id.as_str() == rid);
            match row {
                Some(r) => repo_ids.push(r.id.clone()),
                None => {
                    return Err(Error::NotFound(format!(
                        "repository {rid} not found in project {project_id}"
                    )));
                }
            }
        }

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
        Ok(workspace)
    }

    /// Look up a workspace by id.
    pub async fn get(&self, id: &WorkspaceId) -> Result<Option<Workspace>> {
        concerto_persist::workspaces::get(self.persistence.readers(), id).await
    }

    /// List workspaces in a project.
    pub async fn list_by_project(&self, project_id: &str) -> Result<Vec<Workspace>> {
        concerto_persist::workspaces::list_by_project(self.persistence.readers(), project_id).await
    }

    /// Mark a workspace archived. Idempotent.
    pub async fn archive(&self, id: &WorkspaceId) -> Result<()> {
        // Sanity-check existence so we return NotFound instead of a
        // silent UPDATE-zero-rows.
        if self.get(id).await?.is_none() {
            return Err(Error::NotFound(format!("workspace {id} not found")));
        }
        let now_ms = now_unix_ms();
        let mut writer = self.persistence.writer().await;
        concerto_persist::workspaces::archive(&mut writer, id, now_ms).await?;
        drop(writer);
        let _ = self.events.send(WorkspaceEvent::Archived(id.clone()));
        Ok(())
    }

    /// List repository ids attached to a workspace.
    pub async fn list_repos(&self, workspace_id: &WorkspaceId) -> Result<Vec<RepositoryId>> {
        concerto_persist::workspaces::list_repos(self.persistence.readers(), workspace_id).await
    }
}

/// Supervised actor wrapper. Holds the shared [`WorkspaceManager`] handle.
pub struct WorkspaceManagerActor {
    handle: WorkspaceManager,
}

impl WorkspaceManagerActor {
    /// Build a new actor. The handle is constructed eagerly so callers
    /// can grab a clone before spawning.
    pub fn new(persistence: Arc<Persistence>) -> Self {
        Self {
            handle: WorkspaceManager::new(persistence),
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
