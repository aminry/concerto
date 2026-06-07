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
pub mod archive;
pub mod composers;
pub mod context_dir;
pub mod edit_mutex;
pub mod files_to_copy;
pub mod fsm;
pub mod workarea;

pub use actor::{
    WorkspaceEvent, WorkspaceManager, WorkspaceManagerActor, WorkspaceManagerConfig,
    DUPLICATE_REPO_WIRE_CODE, NO_REPOS_WIRE_CODE,
};
// Task 306 retired `SINGLE_REPO_WIRE_CODE` as an active rejection but
// keeps it defined for one release of client back-compat; re-export it
// (allowing the deprecation) so existing string-matching clients still
// resolve the symbol.
#[allow(deprecated)]
pub use actor::SINGLE_REPO_WIRE_CODE;
pub use archive::ArchiveOpts;
pub use composers::COMPOSERS;
pub use edit_mutex::{
    is_write_class, EditBlocked, EditGuard, EditMutexRegistry, DEFAULT_EDIT_MUTEX_TIMEOUT,
    EDIT_MUTEX_BLOCKED_WIRE_CODE,
};
pub use fsm::{transition, WorkareaEvent as WorkareaFsmEvent, WorkareaState};
pub use workarea::{WorkareaEvent, WorkareaManager, WorkareaManagerActor, WorkareaManagerConfig};
