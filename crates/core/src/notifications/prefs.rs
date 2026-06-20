//! Notification preference resolution (Task 505; design/14 §3.8).
//!
//! Whether a notification PUSHES (vs inbox-only) is resolved through the
//! hierarchy: per-event-kind default → per-workspace opt-out → per-device DND.
//! (The per-schedule override, design/14 §3.8 item 4, is applied upstream by the
//! Scheduler.) **The inbox is always populated** regardless of these — they only
//! gate the wakeup push.

use crate::notifications::model::NotificationKind;

/// The conservative-push default per kind (design/14 §3.1): tool-approval,
/// agent-crashed, and agent-completed push by default; pr/check/schedule are
/// inbox-only by default (the user opts up).
pub fn default_push_for_kind(kind: NotificationKind) -> bool {
    matches!(
        kind,
        NotificationKind::ToolApprovalNeeded
            | NotificationKind::AgentCrashed
            | NotificationKind::AgentCompletedWithMessage
    )
}

/// Resolve whether to push, applying the §3.8 hierarchy. `workspace_opted_out`
/// is the per-workspace `notifications_opt_out` (enterprise-private workspaces
/// set this); `device_dnd_until` is the device's DND floor (push suppressed
/// while `now < dnd_until`).
pub fn should_push(
    kind: NotificationKind,
    workspace_opted_out: bool,
    device_dnd_until: Option<i64>,
    now: i64,
) -> bool {
    if workspace_opted_out {
        return false;
    }
    if device_dnd_until.is_some_and(|until| now < until) {
        return false;
    }
    default_push_for_kind(kind)
}

/// Parse the per-workspace notification opt-out from `workspaces.settings_json`
/// (the `exclude_from_maestro` RMW-key precedent, Task 413). Key:
/// `notifications_opt_out` (bool, default false). A missing/malformed value ⇒
/// not opted out.
pub fn parse_workspace_opt_out(settings_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(settings_json)
        .ok()
        .and_then(|v| {
            v.get("notifications_opt_out")
                .and_then(serde_json::Value::as_bool)
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_push_matches_design_3_1() {
        assert!(default_push_for_kind(NotificationKind::ToolApprovalNeeded));
        assert!(default_push_for_kind(NotificationKind::AgentCrashed));
        assert!(default_push_for_kind(
            NotificationKind::AgentCompletedWithMessage
        ));
        assert!(!default_push_for_kind(NotificationKind::PrStateChanged));
        assert!(!default_push_for_kind(NotificationKind::CheckRunFailed));
        assert!(!default_push_for_kind(
            NotificationKind::ScheduleRunCompleted
        ));
    }

    #[test]
    fn workspace_opt_out_suppresses_push() {
        assert!(!should_push(
            NotificationKind::AgentCrashed,
            true,
            None,
            1000
        ));
    }

    #[test]
    fn device_dnd_window_suppresses_push() {
        // DND active (now < dnd_until) → no push, even for a high-severity kind.
        assert!(!should_push(
            NotificationKind::ToolApprovalNeeded,
            false,
            Some(2000),
            1000
        ));
        // DND expired (now >= dnd_until) → default applies.
        assert!(should_push(
            NotificationKind::ToolApprovalNeeded,
            false,
            Some(500),
            1000
        ));
    }

    #[test]
    fn parse_opt_out_handles_all_shapes() {
        assert!(parse_workspace_opt_out(
            r#"{"notifications_opt_out": true}"#
        ));
        assert!(!parse_workspace_opt_out(
            r#"{"notifications_opt_out": false}"#
        ));
        assert!(!parse_workspace_opt_out(r#"{"other": 1}"#));
        assert!(!parse_workspace_opt_out("{}"));
        assert!(!parse_workspace_opt_out("not json"));
    }
}
