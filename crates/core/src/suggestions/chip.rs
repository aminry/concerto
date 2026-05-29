//! [`Chip`] — the in-process value type a [`super::SuggestionRule`]
//! returns when it decides to emit a suggestion (Task 40).
//!
//! Mirrors the wire `concerto.v1.Chip` proto; the gRPC handler maps
//! between them. Keeping the in-process type separate keeps the
//! suggestion engine independent of `concerto-proto` so the engine can
//! be unit-tested without spinning up the gRPC layer.

use concerto_persist::WorkareaId;

/// Coarse-grained action a chip suggests the user take. V0.1 surfaces
/// the action as a free-form string on the wire (`Chip.action`) so V1.0
/// can refine the catalog without a wire-format break; the in-process
/// type uses a typed enum so rules can't typo an action token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChipAction {
    /// `context_window_50` — compress the conversation context.
    Compress,
    /// `context_window_80` — start a fresh session with a summary.
    NewSession,
    /// `tests_failed` — open the most recent failure surface.
    OpenTestFailure,
    /// `turn_complete_with_uncommitted` — commit + push.
    CommitAndPush,
    /// `awaiting_approval` — review the pending tool call.
    ReviewTool,
    /// `agent_crashed` — resume the agent (or start a new session).
    Resume,
}

impl ChipAction {
    /// Wire string. V0.1 callers (the gRPC mapper) use this directly.
    pub fn as_wire_str(&self) -> &'static str {
        match self {
            ChipAction::Compress => "compress",
            ChipAction::NewSession => "new_session",
            ChipAction::OpenTestFailure => "open_test_failure",
            ChipAction::CommitAndPush => "commit_and_push",
            ChipAction::ReviewTool => "review_tool",
            ChipAction::Resume => "resume",
        }
    }
}

/// One actionable suggestion produced by a rule. Carried in-process
/// over the engine's broadcast channel and across the
/// `Suggestions.GetSuggestions` / `suggestion.events` RPC surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    /// Stable rule identifier — one of the six V0.1 reserved ids (see
    /// [`super::rules`]) or a future rule's id.
    pub rule_id: String,
    /// The workarea this chip is scoped to.
    pub workarea_id: WorkareaId,
    /// Short human-readable label rendered on the chip.
    pub title: String,
    /// Rule-supplied priority — higher wins. V0.1 rules use 1..=100.
    pub priority: i32,
    /// Unix epoch milliseconds the chip was emitted.
    pub created_at: i64,
    /// Action the chip suggests when the user clicks it.
    pub action: ChipAction,
}
