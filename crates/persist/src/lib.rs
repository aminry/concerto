//! Concerto SQLite persistence and migration runner.
//!
//! Owned in design by `09_Persistence.md`. Task 08 ships the migration
//! runner + connect pragmas + reader/writer separation; the actual schema
//! arrives in Task 09 (and forward-only thereafter).
//!
//! The public API lives in [`api`]. Other crates depend on the re-exports
//! at the crate root.
//!
//! Task 18 adds the `repositories` table CRUD under [`repositories`];
//! the table itself is part of migration 0001.
//!
//! Task 19 adds the `workspaces`/`workspace_repos` helpers under
//! [`workspaces`]; the tables themselves ship in migration 0001.
//!
//! Task 20 adds the `workareas`/`workarea_repos` helpers under
//! [`workareas`]; the tables themselves ship in migration 0001.

pub mod api;
pub mod chat_messages;
pub mod checkpoints;
pub mod maestro_state;
pub mod notifications;
pub mod pull_requests;
pub mod repositories;
pub mod schedule_runs;
pub mod schedules;
pub mod sessions;
pub mod skills;
pub mod suggestion_learn;
pub mod tool_approvals;
pub mod vcs_credentials;
pub mod webhook_deliveries;
pub mod workareas;
pub mod workspaces;

pub use api::{
    MaestroState, NewChat, NewPullRequest, NewRepository, NewSchedule, NewScheduleRun, NewSession,
    NewSkill, NewSuggestionLearn, NewVcsCredential, NewWorkarea, NewWorkareaRepo, NewWorkspace,
    Persistence, PersistenceConfig, PullRequest, PullRequestId, Repository, RepositoryId, Schedule,
    ScheduleId, ScheduleRun, ScheduleRunId, Session, SessionId, SkillFilter, SkillId, SkillRow,
    SkillScope, SuggestionLearn, SuggestionLearnId, VcsCredential, VcsCredentialId, Workarea,
    WorkareaId, Workspace, WorkspaceId, WorkspaceRepoCones, WriterGuard,
};

/// Reserved id of the hidden system workspace that hosts the global Maestro
/// session. Excluded from user-facing list queries.
pub const MAESTRO_SYSTEM_WORKSPACE_ID: &str = "__maestro_system__";
/// Reserved id of the hidden system workarea the Maestro session FKs to.
/// Excluded from user-facing list queries.
pub const MAESTRO_SYSTEM_WORKAREA_ID: &str = "__maestro_system_wa__";
