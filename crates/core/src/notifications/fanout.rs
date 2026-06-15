//! Multi-device wakeup fan-out + post-wakeup fetch (Task 504; design/14 §3.4,
//! §6.1/§6.2).
//!
//! When a notification warrants a push, the Core computes the **eligible device
//! set** (active, push-token-bearing, not in DND — [`concerto_persist::notifications::list_pushable_devices`]),
//! **subtracts the actively-viewing devices** ([`ActiveViewing`], design/14 §3.4),
//! and sends the **ID-only** [`WakeupBody`] to each remaining device with bounded
//! retry ([`deliver`]). The phone then pulls the body over its E2EE tunnel
//! ([`fetch_for_device`], which records the per-device `fetched_at`).
//!
//! **First-to-approve-wins** (design/14 §3.4): the guard is the EXISTING
//! `tool_approvals` row / `Sessions.ResolveApproval` idempotency (PHASE5_PLANNING
//! D5) — `ActOnChip` (Task 505) delegates to it, and the loser's
//! `approval.cancelled` broadcast rides `session.events.<sid>` (wired in 507).
//! This module owns the *delivery* half; 505/507 own the *resolution* half.

use std::collections::HashSet;

use concerto_error::Result;
use concerto_persist::notifications::{self, NewDelivery};
use concerto_persist::Persistence;
use concerto_proto::v1 as pb;

use crate::notifications::model::row_to_proto;
use crate::notifications::push::{
    DeliveryReport, PushBackend, PushPlatform, PushTarget, WakeupBody,
};

/// Max wakeup send attempts per device before giving up (design/14 §8: retry
/// with backoff, max 3; the inbox catches up regardless).
pub const MAX_SEND_ATTEMPTS: u32 = 3;

/// "Which devices are actively viewing this workarea" oracle (design/14 §3.4): a
/// device subscribed to `workarea.events{id=X}` or any `session.events.<sid>` for
/// a session in that workarea within the last 30s is excluded from the fan-out
/// (no need to buzz a desktop the user is staring at). The real implementation
/// reads the streams subscription registry tagged with device identity — a seam
/// that needs auth-tagged subscriptions (see the 504 handoff). Until that lands,
/// [`NoActiveViewing`] suppresses nothing (conservative: never miss a wakeup).
pub trait ActiveViewing: Send + Sync {
    /// The set of `device_id`s actively viewing `workarea_id` right now.
    fn actively_viewing(&self, workarea_id: Option<&str>) -> HashSet<String>;
}

/// Default oracle: no device is considered actively-viewing.
pub struct NoActiveViewing;

impl ActiveViewing for NoActiveViewing {
    fn actively_viewing(&self, _workarea_id: Option<&str>) -> HashSet<String> {
        HashSet::new()
    }
}

/// Map persist `(id, token, platform)` rows to typed [`PushTarget`]s, dropping
/// any row whose stored platform is not a known [`PushPlatform`].
pub fn eligible_targets(rows: Vec<(String, String, String)>) -> Vec<PushTarget> {
    rows.into_iter()
        .filter_map(|(device_id, token, platform)| {
            PushPlatform::from_db(&platform).map(|platform| PushTarget {
                device_id,
                token,
                platform,
            })
        })
        .collect()
}

/// Subtract actively-viewing devices from the eligible set (design/14 §6.1).
pub fn plan_fanout(
    eligible: Vec<PushTarget>,
    actively_viewing: &HashSet<String>,
) -> Vec<PushTarget> {
    eligible
        .into_iter()
        .filter(|t| !actively_viewing.contains(&t.device_id))
        .collect()
}

/// Per-device outcome after the (retried) send.
#[derive(Debug, Clone)]
pub struct FanoutResult {
    pub device_id: String,
    pub report: DeliveryReport,
}

/// Send the ID-only wakeup to each planned target, retrying transport errors up
/// to [`MAX_SEND_ATTEMPTS`]. Returns per-device reports; the caller (507) records
/// `delivered_at` + nulls `devices.push_token` on `DeviceNotRegistered`
/// (design/14 §8). The body is ID-only, so no content ever leaves the Core.
pub async fn deliver(
    backend: &dyn PushBackend,
    targets: &[PushTarget],
    body: &WakeupBody,
) -> Vec<FanoutResult> {
    let mut out = Vec::with_capacity(targets.len());
    for t in targets {
        let mut report = DeliveryReport::failed("unsent");
        for attempt in 1..=MAX_SEND_ATTEMPTS {
            match backend.send_wakeup(t, body).await {
                Ok(r) => {
                    report = r;
                    break;
                }
                Err(e) => {
                    report = DeliveryReport::failed(format!("transport: {e}"));
                    if attempt == MAX_SEND_ATTEMPTS {
                        break;
                    }
                    // Backoff timing is a refinement; retry immediately.
                }
            }
        }
        out.push(FanoutResult {
            device_id: t.device_id.clone(),
            report,
        });
    }
    out
}

/// Post-wakeup fetch (design/14 §3.3/§6.2): load the notification, record the
/// per-device `fetched_at` delivery row, and return the wire payload. `Ok(None)`
/// when the id is unknown.
pub async fn fetch_for_device(
    persist: &Persistence,
    id: &str,
    device_id: &str,
    now: i64,
) -> Result<Option<pb::Notification>> {
    let Some(row) = notifications::get(persist.readers(), id).await? else {
        return Ok(None);
    };
    {
        let mut w = persist.writer().await;
        notifications::upsert_delivery(
            &mut w,
            NewDelivery {
                notification_id: id.to_string(),
                device_id: device_id.to_string(),
                delivered_at: None,
                fetched_at: Some(now),
            },
        )
        .await?;
    }
    Ok(Some(row_to_proto(row)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::push::mock::MockOutcome;
    use crate::notifications::push::MockPushBackend;

    fn tgt(id: &str) -> PushTarget {
        PushTarget {
            device_id: id.into(),
            token: format!("tok-{id}"),
            platform: PushPlatform::Expo,
        }
    }

    #[test]
    fn eligible_targets_drops_unknown_platform() {
        let rows = vec![
            ("d1".into(), "t1".into(), "expo".into()),
            ("d2".into(), "t2".into(), "telegram".into()),
        ];
        let t = eligible_targets(rows);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].device_id, "d1");
        assert_eq!(t[0].platform, PushPlatform::Expo);
    }

    #[test]
    fn plan_fanout_subtracts_active_viewing() {
        let eligible = vec![tgt("d1"), tgt("d2"), tgt("d3")];
        let viewing: HashSet<String> = ["d2".to_string()].into_iter().collect();
        let ids: Vec<String> = plan_fanout(eligible, &viewing)
            .into_iter()
            .map(|t| t.device_id)
            .collect();
        assert_eq!(ids, vec!["d1".to_string(), "d3".to_string()]);
    }

    #[test]
    fn no_active_viewing_suppresses_nothing() {
        let v = NoActiveViewing.actively_viewing(Some("wa-1"));
        assert!(v.is_empty());
    }

    #[tokio::test]
    async fn deliver_retries_transport_error_up_to_max() {
        let mock = MockPushBackend::new();
        mock.set_outcome(MockOutcome::TransportError);
        let res = deliver(&mock, &[tgt("d1")], &WakeupBody::new("n", "agent_crashed")).await;
        assert_eq!(res.len(), 1);
        assert!(!res[0].report.ok);
        assert_eq!(
            mock.send_count(),
            MAX_SEND_ATTEMPTS as usize,
            "a transport error retries up to MAX_SEND_ATTEMPTS"
        );
    }

    #[tokio::test]
    async fn deliver_reports_ok_and_stale_token() {
        let mock = MockPushBackend::new();
        let res = deliver(
            &mock,
            &[tgt("d1"), tgt("d2")],
            &WakeupBody::new("n", "agent_crashed"),
        )
        .await;
        assert!(res.iter().all(|r| r.report.ok));
        assert_eq!(mock.send_count(), 2);

        mock.set_outcome(MockOutcome::Report(DeliveryReport::failed(
            "DeviceNotRegistered",
        )));
        let res = deliver(&mock, &[tgt("d3")], &WakeupBody::new("n", "agent_crashed")).await;
        assert!(res[0].report.is_device_not_registered());
    }
}
