//! Suggestion Engine subsystem (Task 40, design/07).
//!
//! V0.1 ships a rule-only engine — six built-in rules listen to
//! `session.events` (per workarea) and emit [`Chip`]s. The chips
//! surface to clients via:
//!
//! - The gRPC `Suggestions.GetSuggestions(workarea_id)` RPC, which
//!   returns the current chip set for a workarea (recent emissions
//!   within the dedup TTL).
//! - The `suggestion.events` stream subject, which broadcasts each
//!   chip as it is emitted.
//!
//! The learning loop (`RecordSuggestionOutcome` → `suggestion_learn`)
//! is V1.0; V0.1's RPC stub logs the outcome and returns empty.
//!
//! ## Module layout
//!
//! - [`actor`] — [`SuggestionEngineActor`] +
//!   [`SuggestionEngineHandle`]. The actor subscribes to
//!   [`crate::workspace_manager::WorkareaManager::subscribe`] and per
//!   live session to
//!   [`crate::agent_supervisor::AgentSupervisorHandle::subscribe_events_with_replay`],
//!   feeds events into the rule pipeline, deduplicates within a 60s
//!   window, and publishes chips on the in-process broadcast channel
//!   the gRPC `Streams` handler consumes.
//! - [`chip`] — the [`Chip`] type returned by rules.
//! - [`state`] — per-workarea [`WorkareaState`] aggregator (small
//!   summary of recent events) consulted by each rule's `applies`.
//! - [`rules`] — the six V0.1 built-in rules.
//!
//! ## Public surface (FROZEN per Task 40)
//!
//! - [`SuggestionEngineHandle::list_for_workarea`]
//! - [`SuggestionEngineHandle::record_outcome`]
//! - [`SuggestionEngineHandle::subscribe`]
//! - The [`SuggestionRule`] trait
//! - The six V0.1 rule ids (reserved namespace):
//!   `context_window_50`, `context_window_80`, `tests_failed`,
//!   `turn_complete_with_uncommitted`, `awaiting_approval`,
//!   `agent_crashed`.

#![cfg(unix)]

pub mod actor;
pub mod chip;
pub mod rules;
pub mod state;

pub use actor::{
    PersistenceWorktreeResolver, SuggestionEngineActor, SuggestionEngineConfig,
    SuggestionEngineHandle, WorktreeResolver, DEDUP_TTL,
};
pub use chip::{Chip, ChipAction};
pub use rules::{builtin_rules, SuggestionRule};
pub use state::WorkareaState;
