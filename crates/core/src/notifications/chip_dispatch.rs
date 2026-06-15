//! `ActOnChip` dispatch (Task 505; design/14 §3.5/§6.3, PHASE5_PLANNING D4).
//!
//! Tapping a notification's action chip fires `Notifications.ActOnChip(id,
//! chip_id, by_device)`. The chip is identified by its `rule_id` within the
//! notification's persisted `chips_json`; its free-form `action` token
//! (suggestions.proto `Chip.action`) classifies into a [`ChipDispatch`] the Core
//! then executes:
//!
//! - **approval** → `Sessions.ResolveApproval` with the chip's decision token,
//! - **message** → `Sessions.SendMessage` with the chip's prompt,
//! - **navigate** → a navigate event for the device's UI.
//!
//! [`act_on_chip`] resolves the chip + records the **denormalized** first-wins
//! marker (`notifications.action_taken`); the *real* first-wins guard is the
//! existing `tool_approvals`/`ResolveApproval` idempotency (D5), which the
//! caller (507) hits when it executes the dispatch. The execution itself lives
//! in 507 (it holds the supervisor handle); this module owns the classification
//! + the idempotent marker so they are unit-testable.

use concerto_error::{Error, Result};
use concerto_persist::{notifications, Persistence};
use concerto_proto::v1 as pb;

/// What a chip's `action` token resolves to (design/14 §6.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChipDispatch {
    /// Approval chip → `Sessions.ResolveApproval` with this decision token
    /// (`approve` | `approve_once` | `deny` | …).
    ResolveApproval { decision: String },
    /// Message chip → `Sessions.SendMessage` with this prompt.
    SendMessage { prompt: String },
    /// Navigate chip → emit a navigate event for the device's UI.
    Navigate { target: String },
}

/// Classify a chip `action` token (D4 mapping). Tokens are matched by prefix so
/// the free-form catalog (suggestions.proto) can grow without a wire break.
pub fn classify_action(action: &str) -> ChipDispatch {
    let a = action.trim();
    let lower = a.to_ascii_lowercase();
    if lower.starts_with("approve")
        || lower.starts_with("deny")
        || lower.starts_with("reject")
        || lower.starts_with("resolve")
    {
        ChipDispatch::ResolveApproval {
            decision: a.to_string(),
        }
    } else if lower.starts_with("send")
        || lower.starts_with("message")
        || lower.starts_with("reply")
        || lower.starts_with("resume")
    {
        ChipDispatch::SendMessage {
            prompt: a.to_string(),
        }
    } else {
        // open_diff / open / navigate / view / … → navigate event.
        ChipDispatch::Navigate {
            target: a.to_string(),
        }
    }
}

/// Outcome of acting on a chip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActOutcome {
    /// The dispatch the chip resolved to (the caller executes it).
    pub dispatch: ChipDispatch,
    /// True iff this device LOST the race — the notification was already acted on
    /// (the denormalized marker was already set). The caller surfaces
    /// `AlreadyResolved` and dismisses the UI.
    pub already_resolved: bool,
}

/// Look up a chip by `rule_id` in a notification's `chips_json`, classify its
/// action, and record the denormalized first-wins marker. `NotFound` if the
/// notification or chip is missing.
pub async fn act_on_chip(
    persist: &Persistence,
    notification_id: &str,
    chip_id: &str,
    by_device_id: &str,
    now: i64,
) -> Result<ActOutcome> {
    let row = notifications::get(persist.readers(), notification_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("notification.unknown: {notification_id}")))?;
    let chips: Vec<pb::Chip> = row
        .chips_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let chip = chips
        .into_iter()
        .find(|c| c.rule_id == chip_id)
        .ok_or_else(|| Error::NotFound(format!("chip.unknown: {chip_id}")))?;
    let dispatch = classify_action(&chip.action);
    let affected = {
        let mut w = persist.writer().await;
        notifications::set_action_taken(&mut w, notification_id, chip_id, now, Some(by_device_id))
            .await?
    };
    Ok(ActOutcome {
        dispatch,
        already_resolved: affected == 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_covers_the_three_dispatch_kinds() {
        assert_eq!(
            classify_action("approve"),
            ChipDispatch::ResolveApproval {
                decision: "approve".into()
            }
        );
        assert_eq!(
            classify_action("deny"),
            ChipDispatch::ResolveApproval {
                decision: "deny".into()
            }
        );
        assert_eq!(
            classify_action("resume"),
            ChipDispatch::SendMessage {
                prompt: "resume".into()
            }
        );
        assert_eq!(
            classify_action("open_diff"),
            ChipDispatch::Navigate {
                target: "open_diff".into()
            }
        );
    }
}
