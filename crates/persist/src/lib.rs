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
//! Task 19 adds the `projects` and `workspaces`/`workspace_repos`
//! helpers under [`projects`] and [`workspaces`]; the tables themselves
//! ship in migration 0001.
//!
//! Task 20 adds the `workareas`/`workarea_repos` helpers under
//! [`workareas`]; the tables themselves ship in migration 0001.

pub mod api;
pub mod projects;
pub mod repositories;
pub mod workareas;
pub mod workspaces;

pub use api::{
    NewProject, NewRepository, NewWorkarea, NewWorkareaRepo, NewWorkspace, Persistence,
    PersistenceConfig, Project, ProjectId, Repository, RepositoryId, Workarea, WorkareaId,
    Workspace, WorkspaceId, WriterGuard,
};
