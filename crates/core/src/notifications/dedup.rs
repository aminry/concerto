//! De-duplication window + retention policy (Task 502; design/14 §3.7, §3.9 R-9).
//!
//! When the same logical event would fire repeatedly (e.g. a check_run flipping
//! fail→pass→fail), the Core de-dups by `(workarea_id|workspace_id, kind,
//! subject_id)`: if a prior UNREAD notification for that key exists within
//! [`DEDUP_WINDOW_MS`], the Core refreshes its body + timestamp instead of
//! inserting a new row, and does NOT re-send a wakeup. The persist-side query
//! ([`concerto_persist::notifications::find_unread_for_dedup_key`]) returns the
//! candidate; this module holds the pure decision + the retention floor so they
//! are unit-testable with synthetic time. The actual `notify()` that wires these
//! together is Task 507.

use concerto_persist::notifications::NotificationRow;

/// De-dup window (`design/14 §3.7 R-2`: 5 min default, per-workspace
/// configurable via `settings_json` — the override is applied by 505/507).
pub const DEDUP_WINDOW_MS: i64 = 5 * 60 * 1000;

/// Inbox retention default (`design/14 §3.9 R-9`: 90 days; older auto-archived,
/// kept-not-deleted in V1.0). The scheduler hook is a P6 note.
pub const RETENTION_DAYS: i64 = 90;

/// The retention floor: notifications older than this are archival candidates.
pub fn retention_floor_ms(now_ms: i64) -> i64 {
    now_ms - RETENTION_DAYS * 24 * 60 * 60 * 1000
}

/// What to do with an incoming notify for a given de-dup key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DedupDecision {
    /// No live duplicate — insert a fresh row + fan out a wakeup.
    InsertNew,
    /// A live duplicate exists — refresh its body + `at`, no new wakeup.
    UpdateExisting(String),
}

/// Decide insert-vs-update given the de-dup-key lookup result. `existing` is the
/// most-recent unread, non-superseded row for the key (or `None`); `window_ms`
/// is the effective de-dup window (default [`DEDUP_WINDOW_MS`], or a
/// per-workspace override). A hit only counts if it is within the window.
pub fn decide(existing: Option<&NotificationRow>, now_ms: i64, window_ms: i64) -> DedupDecision {
    match existing {
        Some(row) if now_ms.saturating_sub(row.created_at) <= window_ms => {
            DedupDecision::UpdateExisting(row.id.clone())
        }
        _ => DedupDecision::InsertNew,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, created_at: i64) -> NotificationRow {
        NotificationRow {
            id: id.into(),
            kind: "check_run_failed".into(),
            subject_kind: "workarea".into(),
            subject_id: "wa-1".into(),
            workspace_id: Some("ws-1".into()),
            workarea_id: Some("wa-1".into()),
            session_id: None,
            title: "t".into(),
            body: "b".into(),
            chips_json: None,
            approval_json: None,
            severity: "low".into(),
            created_at,
            read_at: None,
            superseded_by: None,
            action_taken: None,
            action_taken_at: None,
            action_taken_by_device_id: None,
        }
    }

    #[test]
    fn no_existing_inserts_new() {
        assert_eq!(
            decide(None, 1_000, DEDUP_WINDOW_MS),
            DedupDecision::InsertNew
        );
    }

    #[test]
    fn within_window_updates_existing() {
        let r = row("n-1", 1_000);
        // 2 minutes later — inside the 5-min window.
        assert_eq!(
            decide(Some(&r), 1_000 + 2 * 60 * 1000, DEDUP_WINDOW_MS),
            DedupDecision::UpdateExisting("n-1".into())
        );
    }

    #[test]
    fn outside_window_inserts_new() {
        let r = row("n-1", 1_000);
        // 6 minutes later — past the 5-min window.
        assert_eq!(
            decide(Some(&r), 1_000 + 6 * 60 * 1000, DEDUP_WINDOW_MS),
            DedupDecision::InsertNew
        );
    }

    #[test]
    fn retention_floor_is_90_days_back() {
        let now = 1_000_000_000_000_i64;
        assert_eq!(retention_floor_ms(now), now - 90 * 24 * 60 * 60 * 1000);
    }
}
