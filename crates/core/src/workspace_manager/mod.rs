//! Workspace Manager subsystem (Task 19, design/03).
//!
//! Owns workspace creation / archival and the broadcast channel that
//! later tasks' `Streams` service will subscribe to for
//! `workspace.events`.
//!
//! V0.1 ships single-repo workspaces only — the request validation
//! rejects multi-repo requests with the wire code
//! `workspace.v0_single_repo_only`.

pub mod actor;
pub mod composers;
pub mod context_dir;
pub mod files_to_copy;
pub mod workarea;

pub use actor::{
    WorkspaceEvent, WorkspaceManager, WorkspaceManagerActor, WorkspaceManagerConfig,
    SINGLE_REPO_WIRE_CODE,
};
pub use composers::COMPOSERS;
pub use workarea::{WorkareaEvent, WorkareaManager, WorkareaManagerActor, WorkareaManagerConfig};
