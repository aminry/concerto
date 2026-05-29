//! Per-workarea [`WorkareaState`] aggregator (Task 40).
//!
//! Small struct summarizing recent agent events for a single workarea.
//! Rules consult it (rather than walking the whole event history) so
//! `SuggestionRule::applies` stays O(1) per event.
//!
//! Fields are deliberately minimal — only what the six V0.1 rules
//! consult. Adding a field is a non-breaking change because the type
//! is owned by the engine; the [`super::SuggestionRule`] trait does
//! not expose the type to external callers as a frozen contract.

/// Summarized recent-event view of one workarea, consulted by rules.
///
/// V0.1 fields:
///
/// - `last_context_pct` — last seen
///   [`crate::agent_supervisor::AgentEvent::ContextUsage`] percentage.
/// - `last_turn_complete_ms` — Unix epoch ms of the most recent
///   `TurnComplete`. Rules use it to gate (e.g.
///   `turn_complete_with_uncommitted` only fires inside a short window
///   after a turn finishes).
/// - `awaiting_approval_count` — number of distinct pending approvals
///   currently outstanding. Decremented on `ApprovalResolved`.
/// - `crashed` — true after a `Crashed` event for the workarea's
///   session; reset only when a new session starts.
/// - `last_message_content` — short suffix of the most recent
///   `Message.content` (last 4 KiB). The `tests_failed` rule's regex
///   scans this rather than the whole conversation.
#[derive(Debug, Clone, Default)]
pub struct WorkareaState {
    pub last_context_pct: Option<u8>,
    pub last_turn_complete_ms: Option<i64>,
    pub awaiting_approval_count: u32,
    pub crashed: bool,
    pub last_message_content: String,
}

/// Maximum number of bytes [`WorkareaState::last_message_content`] is
/// allowed to grow to before older content is truncated. Keeps the
/// per-workarea memory cost bounded under noisy agents.
pub const MAX_MESSAGE_BUFFER: usize = 4 * 1024;

impl WorkareaState {
    /// Truncate `last_message_content` to the last [`MAX_MESSAGE_BUFFER`]
    /// bytes. Called by the engine after appending fresh content.
    pub fn trim_message_buffer(&mut self) {
        if self.last_message_content.len() > MAX_MESSAGE_BUFFER {
            let cut = self.last_message_content.len() - MAX_MESSAGE_BUFFER;
            // Find the next char boundary at or after `cut` so we never
            // split a multi-byte UTF-8 code point.
            let mut boundary = cut;
            while boundary < self.last_message_content.len()
                && !self.last_message_content.is_char_boundary(boundary)
            {
                boundary += 1;
            }
            self.last_message_content = self.last_message_content[boundary..].to_string();
        }
    }
}
