//! Bridges the Maestro session's parsed conversation onto the `maestro.events`
//! stream the chat UI renders. The supervisor publishes the Maestro session's
//! `AgentEvent::Message` (assistant text deltas) + `AgentEvent::TurnComplete`;
//! this accumulates a turn's text and emits one `MaestroEvent::Message` per
//! completed turn. (M1: one bubble per turn; delta streaming is later polish.)

use std::sync::Arc;

use crate::agent_supervisor::events::{AgentEvent, MessageRole};
use crate::agent_supervisor::AgentSupervisorHandle;
use crate::maestro::events::{MaestroEvent, MaestroEventSender};
use concerto_persist::{Persistence, SessionId};

/// A completed assistant turn ready to publish.
pub struct TurnMessage {
    pub text: String,
    pub message_id: String,
}

/// Accumulates assistant text within a turn. Pure + unit-testable.
#[derive(Default)]
pub struct TurnAccumulator {
    buf: String,
    seq: u64,
}

impl TurnAccumulator {
    /// Append an assistant text delta. Returns `None` (M1 emits at turn end).
    pub fn on_message(&mut self, text: &str) -> Option<TurnMessage> {
        self.buf.push_str(text);
        None
    }

    /// Close the turn; emit the accumulated text (or `None` if empty).
    pub fn on_turn_complete(&mut self) -> Option<TurnMessage> {
        if self.buf.trim().is_empty() {
            self.buf.clear();
            return None;
        }
        self.seq += 1;
        Some(TurnMessage {
            text: std::mem::take(&mut self.buf),
            message_id: format!("m-{}", self.seq),
        })
    }
}

/// Spawn the bridge for the given Maestro session. Runs until the session's
/// event channel closes (session end / Core shutdown).
///
/// On each completed assistant turn the bridge BOTH emits a
/// `MaestroEvent::Message` (the live bubble) AND persists the assistant text as
/// a `{"text":...}` `chat_messages` row on `chat_id` (Task 8), so the
/// conversation survives a reload. The checkpoint/turn system also writes an
/// assistant `v0_1_turn_marker` row (no text) — that is intentionally separate;
/// the history reader skips the markers and reads this text row instead.
pub fn spawn_maestro_events_bridge(
    supervisor: AgentSupervisorHandle,
    events: MaestroEventSender,
    session_id: SessionId,
    persistence: Arc<Persistence>,
    chat_id: String,
) {
    tokio::spawn(async move {
        let Some(mut rx) = supervisor.subscribe_events(&session_id).await else {
            tracing::warn!(
                target: "concerto::maestro",
                session = %session_id.0,
                "events bridge: no such session to subscribe"
            );
            return;
        };
        let mut acc = TurnAccumulator::default();
        loop {
            match rx.recv().await {
                Ok(AgentEvent::Message {
                    session_id: s,
                    role,
                    content,
                }) if s == session_id => {
                    // Only assistant text becomes a bubble here; the user turn is
                    // published directly by send_to_maestro (Task 7).
                    if matches!(role, MessageRole::Assistant) {
                        acc.on_message(&content);
                    }
                }
                Ok(AgentEvent::TurnComplete { session_id: s }) if s == session_id => {
                    if let Some(m) = acc.on_turn_complete() {
                        // Persist the assistant turn TEXT before emitting (the
                        // checkpoint system's marker row carries none), so the
                        // history reader can rebuild this turn after a reload.
                        // Best-effort: a persistence hiccup must not break the
                        // bubble or the loop.
                        if let Err(e) = concerto_persist::chat_messages::insert_assistant_message(
                            &persistence,
                            &chat_id,
                            &m.text,
                        )
                        .await
                        {
                            tracing::warn!(
                                target: "concerto::maestro",
                                error = %e,
                                "failed to persist maestro assistant turn"
                            );
                        }
                        events.emit(MaestroEvent::Message {
                            text: m.text,
                            message_id: m.message_id,
                            role: "assistant".to_string(),
                        });
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(target: "concerto::maestro", session = %session_id.0, skipped = n, "events bridge lagged; a reply bubble may be truncated");
                    continue;
                }
                Err(_) => break,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulator_emits_one_message_per_turn() {
        let mut acc = TurnAccumulator::default();
        assert!(acc.on_message("Let me check. ").is_none());
        assert!(acc.on_message("You have 1 workspace.").is_none());
        let done = acc.on_turn_complete().expect("a turn produces a message");
        assert_eq!(done.text, "Let me check. You have 1 workspace.");
        assert!(!done.message_id.is_empty());
        // A turn with no assistant text produces nothing.
        assert!(acc.on_turn_complete().is_none());
    }

    #[test]
    fn accumulator_message_id_increments_per_turn() {
        let mut acc = TurnAccumulator::default();
        acc.on_message("first");
        let a = acc.on_turn_complete().expect("turn 1");
        assert_eq!(a.message_id, "m-1");
        acc.on_message("second");
        let b = acc.on_turn_complete().expect("turn 2");
        assert_eq!(b.message_id, "m-2");
        assert_eq!(b.text, "second");
    }
}
