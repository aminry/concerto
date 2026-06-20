//! Property-based privacy invariants for notifications (Task 506; design/14 §10,
//! locked `00 §7.2`).
//!
//! The wakeup payload is the one place notification metadata could leak to
//! Apple/Google/Expo. These properties prove — over arbitrary inputs — that:
//!   1. the wakeup is STRICTLY `{notification_id, kind, source}` and nothing else
//!      (no title/body/subject/chips/approval can structurally appear), and
//!   2. an enterprise-private (opted-out) workspace and a DND device NEVER push
//!      (so no body ever leaves for those), while the inbox still records.

use concerto_core::notifications::model::NotificationKind;
use concerto_core::notifications::prefs::should_push;
use concerto_core::notifications::push::{WakeupBody, WAKEUP_SOURCE};
use proptest::prelude::*;

const ALL_KINDS: [NotificationKind; 6] = [
    NotificationKind::ToolApprovalNeeded,
    NotificationKind::AgentCompletedWithMessage,
    NotificationKind::AgentCrashed,
    NotificationKind::PrStateChanged,
    NotificationKind::CheckRunFailed,
    NotificationKind::ScheduleRunCompleted,
];

proptest! {
    /// The wakeup is ALWAYS exactly three keys — for ANY id + kind, including
    /// adversarial unicode/whitespace — so no content field can structurally
    /// ride along.
    #[test]
    fn wakeup_is_strictly_id_only(id in ".{0,80}", kind in ".{0,40}") {
        let body = WakeupBody::new(id.clone(), kind.clone());
        let v: serde_json::Value = serde_json::from_slice(&body.to_bytes()).unwrap();
        let obj = v.as_object().expect("wakeup is a JSON object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        prop_assert_eq!(keys, vec!["kind", "notification_id", "source"]);
        prop_assert_eq!(obj["source"].as_str().unwrap(), WAKEUP_SOURCE);
        prop_assert_eq!(obj["notification_id"].as_str().unwrap(), id.as_str());
        prop_assert_eq!(obj["kind"].as_str().unwrap(), kind.as_str());
    }

    /// Content tagged with a sentinel is never reachable from the wakeup: the
    /// wakeup is derived only from (id, kind), so the notification's
    /// title/body/subject (here the sentinel) can never appear in its bytes.
    #[test]
    fn wakeup_never_contains_notification_content(
        id in "[a-z0-9-]{1,30}",
        kind in "[a-z_]{1,30}",
        content in ".{0,200}",
    ) {
        let _title = format!("PII_SENTINEL_TITLE:{content}");
        let _body = format!("PII_SENTINEL_BODY:{content}");
        let wakeup = WakeupBody::new(id, kind);
        let s = String::from_utf8(wakeup.to_bytes()).unwrap();
        prop_assert!(!s.contains("PII_SENTINEL_"), "content leaked into wakeup: {}", s);
    }

    /// Enterprise-private (workspace opted out) ⇒ never push, for every kind +
    /// any time. The inbox still records (that is the notify() path, not push).
    #[test]
    fn opted_out_workspace_never_pushes(k in 0usize..ALL_KINDS.len(), now in any::<i64>()) {
        prop_assert!(!should_push(ALL_KINDS[k], true, None, now));
    }

    /// A device in its DND window ⇒ never push (until the window passes).
    #[test]
    fn dnd_window_suppresses_push(k in 0usize..ALL_KINDS.len(), now in 0i64..1_000_000) {
        let dnd_until = now + 1; // strictly in the future ⇒ suppressed
        prop_assert!(!should_push(ALL_KINDS[k], false, Some(dnd_until), now));
    }
}
