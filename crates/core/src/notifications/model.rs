//! Notification domain model (Task 501): the typed kind/subject/severity enums
//! with their DB-string ⇄ proto-enum mappings, the [`NotifyRequest`] input that
//! `04`/`05`/`13`/`507` build a notification from, and the persist-row →
//! `proto::Notification` projection the inbox reads (507) + UI (523) consume.
//!
//! The DB stores the snake_case string forms (the migration 0017 CHECKs); the
//! wire uses the proto enums; this module is the single mapping seam so no other
//! Phase-5 task re-derives a different spelling. FROZEN by Task 501
//! (PHASE5_PLANNING §4.1).

use concerto_persist::notifications::NotificationRow;
use concerto_proto::v1 as pb;

/// Typed notification id (the persist layer uses bare `String`; the core/handle
/// surface uses this newtype — the `tool_approvals` precedent of String-in-DB,
/// typed-at-the-edges).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NotificationId(pub String);

impl std::fmt::Display for NotificationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The six notification kinds (`design/14 §3.1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    ToolApprovalNeeded,
    AgentCompletedWithMessage,
    AgentCrashed,
    PrStateChanged,
    CheckRunFailed,
    ScheduleRunCompleted,
}

impl NotificationKind {
    /// The snake_case DB form (the 0017 `kind` CHECK).
    pub fn as_db(self) -> &'static str {
        match self {
            Self::ToolApprovalNeeded => "tool_approval_needed",
            Self::AgentCompletedWithMessage => "agent_completed_with_message",
            Self::AgentCrashed => "agent_crashed",
            Self::PrStateChanged => "pr_state_changed",
            Self::CheckRunFailed => "check_run_failed",
            Self::ScheduleRunCompleted => "schedule_run_completed",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        Some(match s {
            "tool_approval_needed" => Self::ToolApprovalNeeded,
            "agent_completed_with_message" => Self::AgentCompletedWithMessage,
            "agent_crashed" => Self::AgentCrashed,
            "pr_state_changed" => Self::PrStateChanged,
            "check_run_failed" => Self::CheckRunFailed,
            "schedule_run_completed" => Self::ScheduleRunCompleted,
            _ => return None,
        })
    }

    pub fn to_proto(self) -> pb::NotificationKind {
        match self {
            Self::ToolApprovalNeeded => pb::NotificationKind::ToolApprovalNeeded,
            Self::AgentCompletedWithMessage => pb::NotificationKind::AgentCompletedWithMessage,
            Self::AgentCrashed => pb::NotificationKind::AgentCrashed,
            Self::PrStateChanged => pb::NotificationKind::PrStateChanged,
            Self::CheckRunFailed => pb::NotificationKind::CheckRunFailed,
            Self::ScheduleRunCompleted => pb::NotificationKind::ScheduleRunCompleted,
        }
    }

    /// The conservative-push default severity (`design/14 §3.1`).
    pub fn default_severity(self) -> Severity {
        match self {
            Self::ToolApprovalNeeded | Self::AgentCrashed => Severity::High,
            Self::AgentCompletedWithMessage => Severity::Medium,
            Self::PrStateChanged | Self::CheckRunFailed | Self::ScheduleRunCompleted => {
                Severity::Low
            }
        }
    }
}

/// What a notification is about (PHASE5_PLANNING D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubjectKind {
    Workspace,
    Workarea,
    Session,
    PullRequest,
    ScheduleRun,
}

impl SubjectKind {
    pub fn as_db(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Workarea => "workarea",
            Self::Session => "session",
            Self::PullRequest => "pull_request",
            Self::ScheduleRun => "schedule_run",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        Some(match s {
            "workspace" => Self::Workspace,
            "workarea" => Self::Workarea,
            "session" => Self::Session,
            "pull_request" => Self::PullRequest,
            "schedule_run" => Self::ScheduleRun,
            _ => return None,
        })
    }

    pub fn to_proto(self) -> pb::NotificationSubjectKind {
        match self {
            Self::Workspace => pb::NotificationSubjectKind::Workspace,
            Self::Workarea => pb::NotificationSubjectKind::Workarea,
            Self::Session => pb::NotificationSubjectKind::Session,
            Self::PullRequest => pb::NotificationSubjectKind::PullRequest,
            Self::ScheduleRun => pb::NotificationSubjectKind::ScheduleRun,
        }
    }
}

/// Display severity (`design/14 §3.1`). Stored as a string in the DB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Low,
    Medium,
    High,
}

impl Severity {
    pub fn as_db(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        Some(match s {
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            _ => return None,
        })
    }
}

/// The input `04`/`05`/`13` (and the live `notify_user`, Task 507) hand to the
/// `NotificationHandle::notify` (added in 507). The handle allocates the id,
/// applies preferences + de-dup, persists, and fans out.
#[derive(Debug, Clone)]
pub struct NotifyRequest {
    pub kind: NotificationKind,
    pub subject_kind: SubjectKind,
    pub subject_id: String,
    pub workspace_id: Option<String>,
    pub workarea_id: Option<String>,
    pub session_id: Option<String>,
    pub title: String,
    pub body: String,
    /// Top suggestion chips (suggestions.proto `Chip`); empty when none.
    pub chips: Vec<pb::Chip>,
    /// Tool-approval context for `tool_approval_needed`; `None` otherwise.
    pub approval: Option<pb::ToolApprovalContext>,
    /// Override the kind's default severity, or `None` to use it.
    pub severity: Option<Severity>,
}

impl NotifyRequest {
    /// The effective severity (explicit override or the kind default).
    pub fn effective_severity(&self) -> Severity {
        self.severity
            .unwrap_or_else(|| self.kind.default_severity())
    }
}

/// Project a persisted [`NotificationRow`] into the wire [`pb::Notification`]
/// (the canonical shape for both `GetInbox` and `GetNotification`). Defensive:
/// an unparseable `chips_json`/`approval_json` degrades to empty rather than
/// failing the whole read.
pub fn row_to_proto(row: NotificationRow) -> pb::Notification {
    let chips = row
        .chips_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<Vec<pb::Chip>>(s).ok())
        .unwrap_or_default();
    let approval = row
        .approval_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<pb::ToolApprovalContext>(s).ok());
    pb::Notification {
        id: row.id,
        kind: NotificationKind::from_db(&row.kind)
            .map(NotificationKind::to_proto)
            .unwrap_or(pb::NotificationKind::Unspecified) as i32,
        subject_kind: SubjectKind::from_db(&row.subject_kind)
            .map(SubjectKind::to_proto)
            .unwrap_or(pb::NotificationSubjectKind::Unspecified) as i32,
        subject_id: row.subject_id,
        workspace_id: row.workspace_id,
        workarea_id: row.workarea_id,
        session_id: row.session_id,
        title: row.title,
        body: row.body,
        chips,
        severity: row.severity,
        created_at_ms: row.created_at,
        read_at_ms: row.read_at,
        superseded_by: row.superseded_by,
        action_taken: row.action_taken,
        action_taken_at_ms: row.action_taken_at,
        action_taken_by_device_id: row.action_taken_by_device_id,
        approval,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_db_roundtrips() {
        for k in [
            NotificationKind::ToolApprovalNeeded,
            NotificationKind::AgentCompletedWithMessage,
            NotificationKind::AgentCrashed,
            NotificationKind::PrStateChanged,
            NotificationKind::CheckRunFailed,
            NotificationKind::ScheduleRunCompleted,
        ] {
            assert_eq!(NotificationKind::from_db(k.as_db()), Some(k));
        }
        assert_eq!(NotificationKind::from_db("nope"), None);
    }

    #[test]
    fn subject_kind_db_roundtrips() {
        for s in [
            SubjectKind::Workspace,
            SubjectKind::Workarea,
            SubjectKind::Session,
            SubjectKind::PullRequest,
            SubjectKind::ScheduleRun,
        ] {
            assert_eq!(SubjectKind::from_db(s.as_db()), Some(s));
        }
    }

    #[test]
    fn severity_defaults_match_design() {
        assert_eq!(
            NotificationKind::ToolApprovalNeeded.default_severity(),
            Severity::High
        );
        assert_eq!(
            NotificationKind::AgentCrashed.default_severity(),
            Severity::High
        );
        assert_eq!(
            NotificationKind::AgentCompletedWithMessage.default_severity(),
            Severity::Medium
        );
        assert_eq!(
            NotificationKind::CheckRunFailed.default_severity(),
            Severity::Low
        );
    }
}
