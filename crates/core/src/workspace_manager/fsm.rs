//! Workarea status FSM (Task 31, design/03 §3.1).
//!
//! The workarea is the central FSM in the Workspace Manager. Workspace
//! and session lifecycles fall out of it: a workspace is archived iff its
//! workareas are archived; a session's `Started`/`AwaitingApproval`/
//! `Finished` events drive workarea transitions.
//!
//! ## States (matches `workareas.status` CHECK constraint in migration 0001)
//!
//! - `Created` — row inserted, no on-disk worktree yet (transient inside
//!   `create_workarea`).
//! - `Active` — worktree is on disk, no live session.
//! - `Running` — at least one session is executing.
//! - `Awaiting` — a session is paused for user input (tool approval, etc.).
//! - `Paused` — user paused the workarea; all sessions stopped, state
//!   retained.
//! - `Finished` — all sessions ended cleanly; workarea kept around for
//!   restart or archive.
//! - `Crashed` — a session process crashed and Core has noticed (e.g. on
//!   reboot the worktree path is missing).
//! - `Archived` — `archived_at` set; soft-deleted.
//!
//! ## Events
//!
//! Events come from the Agent Supervisor (`Session*`) and the user
//! (`Pause`/`Resume`/`Archive`/`Restore`). `AdoptCrashed` fires from the
//! Workspace Manager's startup sweep (`design/03 §6.5`).
//!
//! ## Transition table
//!
//! See [`transition`]. Illegal transitions return
//! `Err(Error::Validation("workarea.fsm.invalid_transition: <state> + <event>"))`.
//!
//! The pure function shape (`transition(state, event) -> Result<state>`)
//! is the locked surface; a table-driven test in
//! `crates/core/tests/fsm_table.rs` iterates every (state, event) pair.

use concerto_error::{Error, Result};

/// Wire-code prefix used in [`Error::Validation`] when [`transition`]
/// rejects an event. Clients can switch on this prefix in
/// `ConcertoError.message`.
pub const INVALID_TRANSITION_WIRE_CODE: &str = "workarea.fsm.invalid_transition";

/// Workarea state. Matches the lowercase strings the migration 0001
/// CHECK constraint allows for `workareas.status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkareaState {
    Created,
    Active,
    Running,
    Awaiting,
    Paused,
    Finished,
    Crashed,
    Archived,
}

impl WorkareaState {
    /// SQL form for `workareas.status`.
    pub fn as_sql(&self) -> &'static str {
        match self {
            WorkareaState::Created => "created",
            WorkareaState::Active => "active",
            WorkareaState::Running => "running",
            WorkareaState::Awaiting => "awaiting",
            WorkareaState::Paused => "paused",
            WorkareaState::Finished => "finished",
            WorkareaState::Crashed => "crashed",
            WorkareaState::Archived => "archived",
        }
    }

    /// Parse a lowercase SQL form back to a [`WorkareaState`]. Returns
    /// `None` for unknown values (so a malformed DB row surfaces as a
    /// typed error to the caller, not a silent default).
    pub fn from_sql(s: &str) -> Option<Self> {
        Some(match s {
            "created" => WorkareaState::Created,
            "active" => WorkareaState::Active,
            "running" => WorkareaState::Running,
            "awaiting" => WorkareaState::Awaiting,
            "paused" => WorkareaState::Paused,
            "finished" => WorkareaState::Finished,
            "crashed" => WorkareaState::Crashed,
            "archived" => WorkareaState::Archived,
            _ => return None,
        })
    }

    /// Every variant — used by the table-driven test.
    pub const ALL: [WorkareaState; 8] = [
        WorkareaState::Created,
        WorkareaState::Active,
        WorkareaState::Running,
        WorkareaState::Awaiting,
        WorkareaState::Paused,
        WorkareaState::Finished,
        WorkareaState::Crashed,
        WorkareaState::Archived,
    ];
}

/// Events that drive workarea state changes.
///
/// `Session*` events derive from `AgentEvent`s the Agent Supervisor
/// publishes (Task 22). `Pause`/`Resume`/`Archive`/`Restore` come from
/// user actions through the gRPC surface. `AdoptCrashed` fires once at
/// Core boot if the worktree directory has disappeared (`design/03 §6.5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkareaEvent {
    /// A session started executing (workarea has live agent work).
    SessionStarted,
    /// A session paused for user input (e.g. tool approval).
    SessionAwaiting,
    /// User responded; session continues executing.
    SessionResumed,
    /// All sessions ended cleanly.
    SessionFinished,
    /// A session process crashed (host PID died unexpectedly).
    SessionCrashed,
    /// User paused the workarea.
    Pause,
    /// User resumed the workarea.
    Resume,
    /// User archived the workarea.
    Archive,
    /// User restored an archived workarea.
    Restore,
    /// Boot-time crash adoption (`design/03 §6.5`): worktree gone from
    /// disk so Core marks the workarea crashed.
    AdoptCrashed,
}

impl WorkareaEvent {
    /// Every variant — used by the table-driven test.
    pub const ALL: [WorkareaEvent; 10] = [
        WorkareaEvent::SessionStarted,
        WorkareaEvent::SessionAwaiting,
        WorkareaEvent::SessionResumed,
        WorkareaEvent::SessionFinished,
        WorkareaEvent::SessionCrashed,
        WorkareaEvent::Pause,
        WorkareaEvent::Resume,
        WorkareaEvent::Archive,
        WorkareaEvent::Restore,
        WorkareaEvent::AdoptCrashed,
    ];
}

/// Apply `event` to `state`, returning the new state.
///
/// The transition table below is the authoritative state graph
/// (`design/03 §3.1`). Anything not enumerated is rejected with
/// [`INVALID_TRANSITION_WIRE_CODE`].
///
/// ## Notes
///
/// - `Archive` is allowed from every non-archived state (the design's
///   §3.7 archive semantics stop sessions and tear down the worktree
///   regardless of prior state). Idempotent: `Archived + Archive` stays
///   `Archived`.
/// - `Restore` returns the workarea to `Active` (the design's §3.7
///   security stance resets permission_mode to the workspace default;
///   the FSM only owns state, not permission inheritance).
/// - `AdoptCrashed` is only valid from non-archived states; calling it on
///   `Archived` is rejected.
/// - `Created` → `Active` happens once at the end of `create_workarea`
///   (no event; the transition is internal). The FSM rejects events
///   targeting `Created` other than `Archive` (which a fresh workarea
///   would not realistically receive).
pub fn transition(state: WorkareaState, event: WorkareaEvent) -> Result<WorkareaState> {
    use WorkareaEvent::*;
    use WorkareaState::*;

    let next = match (state, event) {
        // From Active.
        (Active, SessionStarted) => Running,
        (Active, Pause) => Paused,
        (Active, Archive) => Archived,
        (Active, AdoptCrashed) => Crashed,

        // From Running.
        (Running, SessionAwaiting) => Awaiting,
        (Running, SessionFinished) => Finished,
        (Running, SessionCrashed) => Crashed,
        (Running, Pause) => Paused,
        (Running, Archive) => Archived,
        (Running, AdoptCrashed) => Crashed,

        // From Awaiting.
        (Awaiting, SessionResumed) => Running,
        (Awaiting, SessionFinished) => Finished,
        (Awaiting, SessionCrashed) => Crashed,
        (Awaiting, Pause) => Paused,
        (Awaiting, Archive) => Archived,
        (Awaiting, AdoptCrashed) => Crashed,

        // From Paused.
        (Paused, Resume) => Active,
        (Paused, Archive) => Archived,
        (Paused, AdoptCrashed) => Crashed,

        // From Finished.
        (Finished, SessionStarted) => Running,
        (Finished, Archive) => Archived,
        (Finished, AdoptCrashed) => Crashed,

        // From Crashed.
        (Crashed, SessionStarted) => Running,
        (Crashed, Archive) => Archived,

        // From Created (the freshly-inserted row; create_workarea drives
        // the transition to Active inside the same tx, so the FSM only
        // sees Archive here as a defensive case).
        (Created, Archive) => Archived,

        // From Archived: only Restore is legal; Archive is idempotent.
        (Archived, Restore) => Active,
        (Archived, Archive) => Archived,

        _ => {
            return Err(Error::Validation(format!(
                "{INVALID_TRANSITION_WIRE_CODE}: cannot apply {:?} from {:?}",
                event, state
            )))
        }
    };
    Ok(next)
}

/// True iff [`transition`] would succeed for `(state, event)`. Useful
/// for cheap predicate checks where the caller doesn't need the new
/// state (e.g. validation paths in handlers).
pub fn is_allowed(state: WorkareaState, event: WorkareaEvent) -> bool {
    transition(state, event).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_roundtrip() {
        for s in WorkareaState::ALL {
            assert_eq!(WorkareaState::from_sql(s.as_sql()), Some(s));
        }
    }

    #[test]
    fn active_to_running_on_session_started() {
        assert_eq!(
            transition(WorkareaState::Active, WorkareaEvent::SessionStarted).unwrap(),
            WorkareaState::Running
        );
    }

    #[test]
    fn running_to_awaiting_on_session_awaiting() {
        assert_eq!(
            transition(WorkareaState::Running, WorkareaEvent::SessionAwaiting).unwrap(),
            WorkareaState::Awaiting
        );
    }

    #[test]
    fn archive_is_idempotent() {
        assert_eq!(
            transition(WorkareaState::Archived, WorkareaEvent::Archive).unwrap(),
            WorkareaState::Archived
        );
    }

    #[test]
    fn restore_resets_to_active() {
        assert_eq!(
            transition(WorkareaState::Archived, WorkareaEvent::Restore).unwrap(),
            WorkareaState::Active
        );
    }

    #[test]
    fn illegal_session_event_from_paused_is_rejected() {
        let err = transition(WorkareaState::Paused, WorkareaEvent::SessionStarted).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(INVALID_TRANSITION_WIRE_CODE),
            "expected wire code in error, got: {msg}"
        );
    }

    #[test]
    fn archive_allowed_from_every_non_archived_state() {
        for s in WorkareaState::ALL {
            if matches!(s, WorkareaState::Archived) {
                continue;
            }
            assert_eq!(
                transition(s, WorkareaEvent::Archive).unwrap(),
                WorkareaState::Archived,
                "Archive should be allowed from {s:?}"
            );
        }
    }
}
