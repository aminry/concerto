//! Workspace Manager subsystem (Task 19, design/03).
//!
//! Owns workspace creation / archival and the broadcast channel that
//! later tasks' `Streams` service will subscribe to for
//! `workspace.events`.

pub mod actor;
pub mod archive;
pub mod composers;
pub mod context_dir;
pub mod edit_mutex;
pub mod files_to_copy;
pub mod fsm;
pub mod pr_compose;
pub mod workarea;

pub use actor::{
    WorkspaceEvent, WorkspaceManager, WorkspaceManagerActor, WorkspaceManagerConfig,
    WorkspaceRepoSpec, DUPLICATE_REPO_WIRE_CODE, NO_REPOS_WIRE_CODE,
};
pub use archive::ArchiveOpts;
pub use composers::COMPOSERS;
pub use edit_mutex::{
    is_write_class, EditBlocked, EditGuard, EditMutexRegistry, DEFAULT_EDIT_MUTEX_TIMEOUT,
    EDIT_MUTEX_BLOCKED_WIRE_CODE,
};
pub use fsm::{transition, WorkareaEvent as WorkareaFsmEvent, WorkareaState};
pub use pr_compose::PrComposeContext;
pub use workarea::{
    FailureKind, MergeOpts, MergePlan, MergeProgress, MergeReport, MergeStep, PrSetVcs,
    ProgressSink, RenameReport, RepoRenameOutcome, RepoRenameStep, RevertOpts, RevertOutcome,
    RevertReport, RevertStep, WorkareaEvent, WorkareaManager, WorkareaManagerActor,
    WorkareaManagerConfig, DEFAULT_MERGE_CHECK_TIMEOUT,
};
