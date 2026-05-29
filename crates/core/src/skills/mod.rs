//! Skills Registry subsystem (Task 39, design/06).
//!
//! Owns discovery + per-(scope, project, name) enable/disable for
//! skills across the four scopes. V0.1 actively discovers `personal`
//! (`~/.claude/skills/*/SKILL.md`) and `project`
//! (`<repo.local_path>/.claude/skills/*/SKILL.md`); `plugin` and
//! `enterprise` are stubs reserved on the wire so V1.0 can land
//! marketplace install behind the same surface.
//!
//! ## Module layout
//!
//! - [`actor`] — [`SkillsRegistryActor`] + [`SkillsRegistryHandle`].
//!   The actor's `run` parks on shutdown; the cheap-to-clone handle is
//!   the meaningful surface.
//! - [`discovery`] — the walker that scans the SKILL.md fixtures and
//!   upserts them into `skills_index`. Hand-rolled YAML frontmatter
//!   parser (find first `---` line, second `---` line; the chunk
//!   between is YAML).
//!
//! ## Public surface
//!
//! [`SkillsRegistryHandle::list`], [`SkillsRegistryHandle::toggle`],
//! and [`SkillsRegistryHandle::refresh`] are FROZEN per Task 39.

pub mod actor;
pub mod discovery;

pub use actor::{SkillsRegistryActor, SkillsRegistryConfig, SkillsRegistryHandle};
pub use discovery::{discover, SkillFrontmatter, SkillsRefreshReport};
