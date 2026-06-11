//! The five Maestro stream events + their opaque-JSON wire frame (Task 414,
//! `design/08 §5.4`, PHASE4_PLANNING §4.2 — the event-payload arm of D7).
//!
//! The Maestro publishes its lifecycle on the `maestro.events` subject so the
//! Desktop chat top bar (Task 415) renders live messages, routing receipts,
//! digests, and budget/policy state. Each event is serialized to an **opaque
//! JSON frame** (`{"kind": "...", ...}`) carried on the non-oneof
//! `Event.checks_opaque = 17` field — **NEVER** a new `Event.body` oneof arm
//! (the oneof is FROZEN through field 16, D7). The frame is the only wire
//! shape; 415 parses the `{"kind": ...}` envelope.
//!
//! ## The two `MaestroEvent` types (and why)
//!
//! - [`MaestroEvent`] (this module) is the **domain** event — a typed enum the
//!   Maestro handle/handler emits. It is FROZEN here.
//! - [`crate::handlers::streams::MaestroEvent`] (Task 401.5) is the **carrier**
//!   — a `{ frame: Vec<u8> }` newtype the streams layer wraps into
//!   `Event.checks_opaque`. 401.5 froze the carrier + the
//!   `StreamsHandler::with_maestro_events` producer setter + the
//!   `Subject::MaestroEvents` arm.
//!
//! This module bridges the two: [`MaestroEvent::to_frame`] produces the JSON
//! bytes, and [`MaestroEventSender`] wraps a `broadcast::Sender` of the carrier
//! so the handle can `emit(domain_event)` and the streams layer receives a
//! frame it can wrap verbatim.

use tokio::sync::broadcast;

use crate::handlers::streams::MaestroEvent as MaestroEventFrame;

/// Capacity of the `maestro.events` broadcast channel. Mirrors the streams
/// layer's `LIVE_BROADCAST_CAP` order-of-magnitude: a slow subscriber that
/// lags past this is mapped to end-of-stream by the per-subject pump, exactly
/// as `checks.*`/`transport.events` already behave.
const MAESTRO_EVENTS_CHANNEL_CAP: usize = 1024;

/// The five Maestro stream events (`design/08 §5.4`). Serialized to an opaque
/// JSON frame and carried on `Event.checks_opaque = 17` — NEVER a new
/// `Event.body` oneof arm (oneof FROZEN through field 16, D7). 415 parses these
/// frames off the `maestro.events` subject.
///
/// Field sets are minimal + append-friendly; the **`{"kind": ...}` envelope is
/// FROZEN** because 415 parses it (mirrors `design/13 §5.3`'s `checks.*` frame
/// discipline).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaestroEvent {
    /// `maestro.message` — streamed assistant output from the Maestro session.
    Message {
        /// The assistant text chunk.
        text: String,
        /// A stable id for the message this chunk belongs to.
        message_id: String,
    },
    /// `maestro.routing_executed` — a deterministic `@workarea` route fired
    /// (408's `pre_parse` → resolve → dispatch).
    RoutingExecuted {
        /// The resolved composer targets the body was routed to.
        targets: Vec<String>,
        /// The routed body (verbatim user text after the target span).
        body: String,
    },
    /// `maestro.digest_generated` — a return-from-absence digest was produced
    /// (409's `generate_digest`).
    DigestGenerated {
        /// Unix-ms the digest was generated.
        at_ms: i64,
        /// How many workareas the digest covered.
        n_workareas: u32,
    },
    /// `maestro.budget_exhausted` — the daily token budget tripped inert (412
    /// owns the counting; emitted here only when the handle reports exhaustion).
    BudgetExhausted {
        /// Unix-ms the budget resets (the user sees "resets at …").
        resets_at_ms: i64,
    },
    /// `maestro.disabled_by_policy` — the Maestro LLM is disabled because of the
    /// `enterpriseDataPrivacy` + external-model gate (design/08 §3.10, D1).
    DisabledByPolicy {
        /// The machine-readable reason (e.g. `"enterprise_data_privacy"`).
        reason: String,
    },
}

impl MaestroEvent {
    /// The exact wire `kind` string for this event (the `"kind"` field of the
    /// opaque JSON envelope). FROZEN — 415 matches on these.
    pub fn kind(&self) -> &'static str {
        match self {
            MaestroEvent::Message { .. } => "maestro.message",
            MaestroEvent::RoutingExecuted { .. } => "maestro.routing_executed",
            MaestroEvent::DigestGenerated { .. } => "maestro.digest_generated",
            MaestroEvent::BudgetExhausted { .. } => "maestro.budget_exhausted",
            MaestroEvent::DisabledByPolicy { .. } => "maestro.disabled_by_policy",
        }
    }

    /// Serialize to the opaque `{"kind": "...", ...}` JSON frame carried on
    /// `Event.checks_opaque = 17`. The `kind` key is always present and FROZEN;
    /// the remaining keys are this event's payload. Mirrors `design/13 §5.3`'s
    /// `checks.*` frame discipline.
    pub fn to_frame(&self) -> Vec<u8> {
        let value = match self {
            MaestroEvent::Message { text, message_id } => serde_json::json!({
                "kind": self.kind(),
                "text": text,
                "message_id": message_id,
            }),
            MaestroEvent::RoutingExecuted { targets, body } => serde_json::json!({
                "kind": self.kind(),
                "targets": targets,
                "body": body,
            }),
            MaestroEvent::DigestGenerated { at_ms, n_workareas } => serde_json::json!({
                "kind": self.kind(),
                "at_ms": at_ms,
                "n_workareas": n_workareas,
            }),
            MaestroEvent::BudgetExhausted { resets_at_ms } => serde_json::json!({
                "kind": self.kind(),
                "resets_at_ms": resets_at_ms,
            }),
            MaestroEvent::DisabledByPolicy { reason } => serde_json::json!({
                "kind": self.kind(),
                "reason": reason,
            }),
        };
        // `serde_json::to_vec` on a `Value` cannot fail; fall back to an empty
        // object rather than panic (the streams layer tolerates any frame).
        serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec())
    }
}

/// The Maestro events producer: a thin wrapper over the
/// `broadcast::Sender<`[`MaestroEventFrame`]`>` the streams layer subscribes to
/// via [`crate::handlers::streams::StreamsHandler::with_maestro_events`].
///
/// The [`MaestroHandle`](crate::maestro::MaestroHandle) owns one of these;
/// `boot.rs` hands `handle.events_sender()` into `with_maestro_events` so the
/// `maestro.events` subject has a live producer. `emit` converts a domain
/// [`MaestroEvent`] to its frame and broadcasts it; a send with no live
/// subscribers is a no-op (the events are fire-and-forget telemetry, never an
/// error path).
#[derive(Clone)]
pub struct MaestroEventSender {
    tx: broadcast::Sender<MaestroEventFrame>,
}

impl MaestroEventSender {
    /// Construct a fresh producer with its own bounded broadcast channel.
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(MAESTRO_EVENTS_CHANNEL_CAP);
        Self { tx }
    }

    /// The underlying carrier sender the streams layer attaches as the
    /// `maestro.events` producer (`StreamsHandler::with_maestro_events`).
    pub fn frame_sender(&self) -> broadcast::Sender<MaestroEventFrame> {
        self.tx.clone()
    }

    /// Emit a domain [`MaestroEvent`]: serialize to its opaque frame and
    /// broadcast. Returns the number of live subscribers reached (0 when none
    /// are attached — a no-op, never an error).
    pub fn emit(&self, event: MaestroEvent) -> usize {
        let frame = MaestroEventFrame {
            frame: event.to_frame(),
        };
        self.tx.send(frame).unwrap_or(0)
    }
}

impl Default for MaestroEventSender {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_frame(ev: &MaestroEvent) -> serde_json::Value {
        serde_json::from_slice(&ev.to_frame()).expect("frame is valid JSON")
    }

    #[test]
    fn kind_strings_are_frozen() {
        assert_eq!(
            MaestroEvent::Message {
                text: String::new(),
                message_id: String::new()
            }
            .kind(),
            "maestro.message"
        );
        assert_eq!(
            MaestroEvent::RoutingExecuted {
                targets: vec![],
                body: String::new()
            }
            .kind(),
            "maestro.routing_executed"
        );
        assert_eq!(
            MaestroEvent::DigestGenerated {
                at_ms: 0,
                n_workareas: 0
            }
            .kind(),
            "maestro.digest_generated"
        );
        assert_eq!(
            MaestroEvent::BudgetExhausted { resets_at_ms: 0 }.kind(),
            "maestro.budget_exhausted"
        );
        assert_eq!(
            MaestroEvent::DisabledByPolicy {
                reason: String::new()
            }
            .kind(),
            "maestro.disabled_by_policy"
        );
    }

    #[test]
    fn frame_round_trips_kind_and_payload() {
        let ev = MaestroEvent::Message {
            text: "hello".into(),
            message_id: "m-1".into(),
        };
        let v = parse_frame(&ev);
        assert_eq!(v["kind"], "maestro.message");
        assert_eq!(v["text"], "hello");
        assert_eq!(v["message_id"], "m-1");

        let ev = MaestroEvent::RoutingExecuted {
            targets: vec!["bach".into(), "mozart".into()],
            body: "go".into(),
        };
        let v = parse_frame(&ev);
        assert_eq!(v["kind"], "maestro.routing_executed");
        assert_eq!(v["targets"][0], "bach");
        assert_eq!(v["targets"][1], "mozart");
        assert_eq!(v["body"], "go");

        let ev = MaestroEvent::DigestGenerated {
            at_ms: 123,
            n_workareas: 6,
        };
        let v = parse_frame(&ev);
        assert_eq!(v["kind"], "maestro.digest_generated");
        assert_eq!(v["at_ms"], 123);
        assert_eq!(v["n_workareas"], 6);

        let ev = MaestroEvent::BudgetExhausted { resets_at_ms: 999 };
        let v = parse_frame(&ev);
        assert_eq!(v["kind"], "maestro.budget_exhausted");
        assert_eq!(v["resets_at_ms"], 999);

        let ev = MaestroEvent::DisabledByPolicy {
            reason: "enterprise_data_privacy".into(),
        };
        let v = parse_frame(&ev);
        assert_eq!(v["kind"], "maestro.disabled_by_policy");
        assert_eq!(v["reason"], "enterprise_data_privacy");
    }

    #[tokio::test]
    async fn emit_broadcasts_frame_to_subscriber() {
        let sender = MaestroEventSender::new();
        let mut rx = sender.frame_sender().subscribe();
        let reached = sender.emit(MaestroEvent::DigestGenerated {
            at_ms: 7,
            n_workareas: 2,
        });
        assert_eq!(reached, 1, "one live subscriber");
        let frame = rx.recv().await.expect("frame received");
        let v: serde_json::Value = serde_json::from_slice(&frame.frame).expect("json");
        assert_eq!(v["kind"], "maestro.digest_generated");
        assert_eq!(v["n_workareas"], 2);
    }

    #[test]
    fn emit_with_no_subscriber_is_noop() {
        let sender = MaestroEventSender::new();
        // No subscriber attached → send returns 0, never errors.
        let reached = sender.emit(MaestroEvent::Message {
            text: "x".into(),
            message_id: "m".into(),
        });
        assert_eq!(reached, 0);
    }
}
