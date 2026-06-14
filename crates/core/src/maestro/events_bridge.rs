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

    // =========================================================================
    // Task 9 — conversation SEAM integration test
    //
    // Proves the real parser → real accumulator → MaestroEvent shape end-to-end
    // without a live Claude session. All units are real; no mocks.
    //
    // Placement rationale: `compose_user_envelope` is `pub(crate)` (handle.rs)
    // and `TurnAccumulator`/`TurnMessage` live here — an in-crate test module
    // reaches both without widening any public surface.
    // =========================================================================

    /// Feed the fixture through the real `MaestroStreamJsonPack` (in 7-byte
    /// chunks to exercise partial-line buffering), then pipe the resulting
    /// `ParseEvent`s through the real `TurnAccumulator`, and finally assert the
    /// completed turn round-trips to a `maestro.message` frame with
    /// `role="assistant"` carrying the expected reply text.
    #[test]
    fn conversation_seam_fixture_produces_assistant_message_frame() {
        use crate::agent_supervisor::parsers::maestro_stream_json::MaestroStreamJsonPack;
        use crate::agent_supervisor::parsers::{MsgRole, ParseEvent, ParserPack};
        use crate::maestro::events::MaestroEvent;
        use crate::maestro::handle::compose_user_envelope;

        // ── 1. Input half: compose_user_envelope produces a parseable line ──
        let envelope = compose_user_envelope("hi");
        assert!(envelope.ends_with('\n'), "envelope must end with newline");
        let parsed: serde_json::Value =
            serde_json::from_str(envelope.trim_end()).expect("envelope is valid JSON");
        assert_eq!(parsed["type"], "user");
        assert_eq!(parsed["message"]["role"], "user");
        assert_eq!(parsed["message"]["content"][0]["type"], "text");
        assert_eq!(parsed["message"]["content"][0]["text"], "hi");

        // ── 2. Output half: feed fixture through parser → accumulator ────────
        let pack = MaestroStreamJsonPack::new();
        let data = include_bytes!("../../tests/fixtures/maestro_stream_json/turn.jsonl");
        let mut buf = Vec::new();
        let mut parse_events: Vec<ParseEvent> = Vec::new();
        // 7-byte chunks exercise partial-line buffering in the real parser.
        for chunk in data.chunks(7) {
            buf.extend_from_slice(chunk);
            parse_events.extend(pack.parse_chunk(&mut buf));
        }

        // Feed events into the real TurnAccumulator.
        let mut acc = TurnAccumulator::default();
        let mut completed: Option<TurnMessage> = None;
        for event in &parse_events {
            match event {
                ParseEvent::Message {
                    role: MsgRole::Assistant,
                    content,
                } => {
                    acc.on_message(content);
                }
                ParseEvent::TurnComplete => {
                    completed = acc.on_turn_complete();
                }
                _ => {}
            }
        }

        // ── 3. Assert the accumulated turn carries the fixture's reply text ──
        let turn = completed.expect("fixture must produce a completed TurnMessage");
        assert!(
            turn.text.contains("1 workspace"),
            "accumulated text must contain the fixture reply; got: {:?}",
            turn.text
        );
        assert!(!turn.message_id.is_empty(), "message_id must be non-empty");

        // ── 4. Assert the MaestroEvent frame round-trips correctly ───────────
        let event = MaestroEvent::Message {
            text: turn.text.clone(),
            message_id: turn.message_id.clone(),
            role: "assistant".to_string(),
        };
        assert_eq!(event.kind(), "maestro.message");
        let frame = event.to_frame();
        let v: serde_json::Value =
            serde_json::from_slice(&frame).expect("to_frame must produce valid JSON");
        assert_eq!(
            v["kind"], "maestro.message",
            "frame kind must be maestro.message"
        );
        assert_eq!(v["role"], "assistant", "frame role must be assistant");
        assert!(
            v["text"].as_str().unwrap_or("").contains("1 workspace"),
            "frame text must carry the reply; got: {:?}",
            v["text"]
        );
        assert_eq!(
            v["message_id"].as_str(),
            Some(turn.message_id.as_str()),
            "frame message_id must match the turn's id"
        );
    }
}
