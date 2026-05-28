//! Table-driven FSM tests (Task 31).
//!
//! Iterates every (state, event) pair in [`WorkareaState::ALL`] ×
//! [`WorkareaEvent::ALL`] and asserts the outcome matches the locked
//! transition table from `crates/core/src/workspace_manager/fsm.rs`
//! (which is the executable form of `design/03 §3.1`).
//!
//! Any change to the transition table requires updating the
//! [`expected`] function below — that's the point of the table-driven
//! test: every legal/illegal pair is enumerated in one place so reviewing
//! a diff against this table = reviewing the FSM contract change.

use concerto_core::workspace_manager::fsm::INVALID_TRANSITION_WIRE_CODE;
use concerto_core::workspace_manager::fsm::{transition, WorkareaEvent, WorkareaState};

/// Expected result of `transition(state, event)`:
/// `Some(next)` if legal, `None` if illegal.
fn expected(state: WorkareaState, event: WorkareaEvent) -> Option<WorkareaState> {
    use WorkareaEvent::*;
    use WorkareaState::*;
    match (state, event) {
        // From Active.
        (Active, SessionStarted) => Some(Running),
        (Active, Pause) => Some(Paused),
        (Active, Archive) => Some(Archived),
        (Active, AdoptCrashed) => Some(Crashed),

        // From Running.
        (Running, SessionAwaiting) => Some(Awaiting),
        (Running, SessionFinished) => Some(Finished),
        (Running, SessionCrashed) => Some(Crashed),
        (Running, Pause) => Some(Paused),
        (Running, Archive) => Some(Archived),
        (Running, AdoptCrashed) => Some(Crashed),

        // From Awaiting.
        (Awaiting, SessionResumed) => Some(Running),
        (Awaiting, SessionFinished) => Some(Finished),
        (Awaiting, SessionCrashed) => Some(Crashed),
        (Awaiting, Pause) => Some(Paused),
        (Awaiting, Archive) => Some(Archived),
        (Awaiting, AdoptCrashed) => Some(Crashed),

        // From Paused.
        (Paused, Resume) => Some(Active),
        (Paused, Archive) => Some(Archived),
        (Paused, AdoptCrashed) => Some(Crashed),

        // From Finished.
        (Finished, SessionStarted) => Some(Running),
        (Finished, Archive) => Some(Archived),
        (Finished, AdoptCrashed) => Some(Crashed),

        // From Crashed.
        (Crashed, SessionStarted) => Some(Running),
        (Crashed, Archive) => Some(Archived),

        // From Created.
        (Created, Archive) => Some(Archived),

        // From Archived.
        (Archived, Restore) => Some(Active),
        (Archived, Archive) => Some(Archived),

        _ => None,
    }
}

#[test]
fn every_pair_matches_expected_table() {
    let mut legal = 0;
    let mut illegal = 0;
    for &state in &WorkareaState::ALL {
        for &event in &WorkareaEvent::ALL {
            let actual = transition(state, event);
            match (expected(state, event), actual) {
                (Some(want), Ok(got)) => {
                    assert_eq!(
                        got, want,
                        "transition({state:?}, {event:?}) = {got:?}, expected {want:?}"
                    );
                    legal += 1;
                }
                (None, Err(err)) => {
                    let msg = err.to_string();
                    assert!(
                        msg.contains(INVALID_TRANSITION_WIRE_CODE),
                        "illegal transition error message must contain {INVALID_TRANSITION_WIRE_CODE}; got: {msg}"
                    );
                    illegal += 1;
                }
                (Some(want), Err(err)) => panic!(
                    "transition({state:?}, {event:?}) was expected to succeed -> {want:?}, but failed: {err}"
                ),
                (None, Ok(got)) => panic!(
                    "transition({state:?}, {event:?}) was expected to be REJECTED, but succeeded -> {got:?}"
                ),
            }
        }
    }
    // Cardinality sanity: 8 states × 10 events = 80 pairs.
    assert_eq!(legal + illegal, 80, "must cover every (state, event) pair");
    // Sanity: the design's `§3.1` graph allows a small number of legal
    // transitions; if this number changes accidentally (e.g. a typo
    // gives an event a new edge), the test surfaces it.
    // 4 (Active) + 6 (Running) + 6 (Awaiting) + 3 (Paused) + 3 (Finished)
    // + 2 (Crashed) + 1 (Created) + 2 (Archived) = 27 legal pairs.
    assert_eq!(
        legal, 27,
        "legal transition count drifted; review the table"
    );
}

#[test]
fn invalid_transition_wire_code_is_stable() {
    // Sanity-check that the locked wire code remains the documented
    // value — clients switch on this prefix and a rename is a wire
    // break.
    assert_eq!(
        INVALID_TRANSITION_WIRE_CODE,
        "workarea.fsm.invalid_transition"
    );
}

#[test]
fn sql_form_is_stable() {
    // The SQL form is the migration-0001 CHECK-constraint set. A change
    // here without a forward migration is a schema bug.
    let pairs = [
        (WorkareaState::Created, "created"),
        (WorkareaState::Active, "active"),
        (WorkareaState::Running, "running"),
        (WorkareaState::Awaiting, "awaiting"),
        (WorkareaState::Paused, "paused"),
        (WorkareaState::Finished, "finished"),
        (WorkareaState::Crashed, "crashed"),
        (WorkareaState::Archived, "archived"),
    ];
    for (state, sql) in pairs {
        assert_eq!(state.as_sql(), sql);
        assert_eq!(WorkareaState::from_sql(sql), Some(state));
    }
}
