//! `SkillsRegistryActor` + cloneable `SkillsRegistryHandle` (Task 39).
//!
//! Follows the same actor pattern as the other Core managers — the
//! actor's `run` parks on shutdown; all meaningful work flows through
//! the cheap-to-clone handle.
//!
//! ## V0.1 surface
//!
//! - [`SkillsRegistryHandle::list`] reads the persisted
//!   `skills_index` rows filtered on `(scope, project_id, enabled_only)`.
//! - [`SkillsRegistryHandle::toggle`] flips a row's `enabled` column
//!   and returns the updated row.
//! - [`SkillsRegistryHandle::refresh`] walks `~/.claude/skills/` and the
//!   per-project `<repo.local_path>/.claude/skills/` directories,
//!   upserting every well-formed SKILL.md into `skills_index`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use concerto_error::{Error, Result};
use concerto_persist::{Persistence, ProjectId, SkillFilter, SkillId, SkillRow};

use super::discovery::{discover, SkillsRefreshReport};
use crate::supervisor::{Actor, ActorContext};

/// Config for the actor's `run` loop. V0.1 has no knobs — the actor
/// parks on shutdown.
#[derive(Clone, Debug, Default)]
pub struct SkillsRegistryConfig;

/// Supervised actor that owns the skills registry handle. The
/// meaningful work flows through [`SkillsRegistryHandle`]; `run` just
/// parks on shutdown.
pub struct SkillsRegistryActor {
    handle: SkillsRegistryHandle,
}

/// Cheap-cloneable, shareable handle to the Skills Registry. Frozen
/// per Task 39 §"Public interface this task locks".
#[derive(Clone)]
pub struct SkillsRegistryHandle {
    persistence: Arc<Persistence>,
    /// `~/` resolved at boot. Overridden in tests so the personal
    /// scope can be redirected to a `tempdir`.
    home_dir: PathBuf,
}

impl SkillsRegistryHandle {
    /// Build a fresh handle. Production callers pass the real
    /// `home::home_dir()`; tests pass a `tempdir` so the personal
    /// scope walk does not touch the developer's actual
    /// `~/.claude/skills/`.
    pub fn new(persistence: Arc<Persistence>, home_dir: PathBuf) -> Self {
        Self {
            persistence,
            home_dir,
        }
    }

    /// Borrow the shared persistence handle. Used by the gRPC
    /// `Skills` handler so it does not need a separate
    /// `Arc<Persistence>` plumbed through `api_server`.
    pub fn persistence(&self) -> Arc<Persistence> {
        Arc::clone(&self.persistence)
    }

    /// List skills, optionally filtered on `(scope, project_id,
    /// enabled_only)`. Pure read.
    pub async fn list(&self, filter: SkillFilter) -> Result<Vec<SkillRow>> {
        concerto_persist::skills::list(self.persistence.readers(), &filter).await
    }

    /// Set `enabled` on a row. Returns the updated row. `NOT_FOUND`
    /// is surfaced as `Error::NotFound` when the id is unknown.
    pub async fn toggle(&self, skill_id: &SkillId, enable: bool) -> Result<SkillRow> {
        let updated = {
            let mut writer = self.persistence.writer().await;
            concerto_persist::skills::set_enabled(&mut writer, skill_id, enable).await?
        };
        if !updated {
            return Err(Error::NotFound(format!("skill {skill_id} not found")));
        }
        concerto_persist::skills::get(self.persistence.readers(), skill_id)
            .await?
            .ok_or_else(|| Error::Internal(format!("skill {skill_id} missing after toggle")))
    }

    /// Re-run the discovery walk. V0.1 walks personal + per-project
    /// scopes; V1.0 will add real marketplace fetch behind the same
    /// entry point.
    pub async fn refresh(&self, project_filter: Option<&ProjectId>) -> Result<SkillsRefreshReport> {
        discover(&self.persistence, &self.home_dir, project_filter).await
    }
}

impl SkillsRegistryActor {
    /// Build a new actor with a fresh handle. `home_dir` should be the
    /// user's home directory (the personal scope walks
    /// `<home_dir>/.claude/skills/`).
    pub fn new(persistence: Arc<Persistence>, home_dir: PathBuf) -> Self {
        Self {
            handle: SkillsRegistryHandle::new(persistence, home_dir),
        }
    }

    /// Cheap clone of the shared handle.
    pub fn handle(&self) -> SkillsRegistryHandle {
        self.handle.clone()
    }
}

#[async_trait]
impl Actor for SkillsRegistryActor {
    const NAME: &'static str = "skills-registry";
    type Config = SkillsRegistryConfig;

    async fn run(self, ctx: ActorContext<Self::Config>) -> Result<()> {
        tracing::info!("Skills registry ready");
        ctx.shutdown.cancelled().await;
        tracing::debug!("Skills registry actor shutting down");
        Ok(())
    }
}
