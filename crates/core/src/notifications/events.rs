//! `notification.events` producer bridge (Task 507; design/14 §5.3).
//!
//! Bridges [`NotificationHandle`](crate::notifications::handle::NotificationHandle)'s
//! abstract [`NotificationEvent`]s onto the `notification.events` streams subject
//! as opaque JSON frames carried on `Event.checks_opaque = 17` (the maestro /
//! checks precedent — NO new oneof arm). The Desktop/web inbox UI (523) parses
//! these frames off the subject.

use tokio::sync::broadcast;

use crate::handlers::streams::NotificationStreamEvent;
use crate::notifications::handle::{NotificationEvent, NotificationEvents};

/// Capacity of the `notification.events` broadcast channel (mirrors the other
/// lifecycle-event channels).
pub const NOTIFICATION_EVENTS_CAP: usize = 256;

/// Build the opaque JSON frame for a notification event (design/14 §5.3). Shape:
/// `{"kind": "notification.created"|"updated"|"read"|"acted", "id": "..."}`
/// (+ `chip_id`/`by_device_id` for `acted`). **FROZEN** — 523 parses it.
pub fn to_frame(ev: &NotificationEvent) -> Vec<u8> {
    let v = match ev {
        NotificationEvent::Created(id) => {
            serde_json::json!({ "kind": "notification.created", "id": id })
        }
        NotificationEvent::Updated(id) => {
            serde_json::json!({ "kind": "notification.updated", "id": id })
        }
        NotificationEvent::Read(id) => {
            serde_json::json!({ "kind": "notification.read", "id": id })
        }
        NotificationEvent::Acted {
            id,
            chip_id,
            by_device_id,
        } => serde_json::json!({
            "kind": "notification.acted",
            "id": id,
            "chip_id": chip_id,
            "by_device_id": by_device_id,
        }),
    };
    serde_json::to_vec(&v).unwrap_or_default()
}

/// A [`NotificationEvents`] sink that publishes onto the `notification.events`
/// streams subject. Hand the paired sender to
/// [`StreamsHandler::with_notification_events`](crate::handlers::streams::StreamsHandler::with_notification_events).
#[derive(Clone)]
pub struct NotificationEventSender {
    tx: broadcast::Sender<NotificationStreamEvent>,
}

impl NotificationEventSender {
    pub fn new(tx: broadcast::Sender<NotificationStreamEvent>) -> Self {
        Self { tx }
    }
}

impl NotificationEvents for NotificationEventSender {
    fn emit(&self, event: NotificationEvent) {
        // Best-effort: a send error just means no live subscribers.
        let _ = self.tx.send(NotificationStreamEvent {
            frame: to_frame(&event),
        });
    }
}

/// Create the `notification.events` broadcast channel; the sender feeds both the
/// `NotificationEventSender` (producer) and `with_notification_events` (subject).
pub fn channel() -> (
    broadcast::Sender<NotificationStreamEvent>,
    broadcast::Receiver<NotificationStreamEvent>,
) {
    broadcast::channel(NOTIFICATION_EVENTS_CAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_shapes_are_stable() {
        let f = to_frame(&NotificationEvent::Created("n-1".into()));
        let v: serde_json::Value = serde_json::from_slice(&f).unwrap();
        assert_eq!(v["kind"], "notification.created");
        assert_eq!(v["id"], "n-1");

        let f = to_frame(&NotificationEvent::Acted {
            id: "n-1".into(),
            chip_id: "approve".into(),
            by_device_id: "dev-1".into(),
        });
        let v: serde_json::Value = serde_json::from_slice(&f).unwrap();
        assert_eq!(v["kind"], "notification.acted");
        assert_eq!(v["chip_id"], "approve");
        assert_eq!(v["by_device_id"], "dev-1");
    }

    #[tokio::test]
    async fn sender_publishes_frames() {
        let (tx, mut rx) = channel();
        let sink = NotificationEventSender::new(tx);
        sink.emit(NotificationEvent::Read("n-9".into()));
        let got = rx.recv().await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&got.frame).unwrap();
        assert_eq!(v["kind"], "notification.read");
        assert_eq!(v["id"], "n-9");
    }
}
