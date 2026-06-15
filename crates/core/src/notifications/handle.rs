//! `NotificationHandle` — the public Rust API of sub-system 14 (Task 507;
//! design/14 §5.1, §6.1/§6.2).
//!
//! Wires the pieces 501–506 built into the `notify()` orchestration + the
//! caller/gRPC-facing reads (`get_inbox`/`get_notification`/`mark_read`/
//! `act_on_chip`). `04`/`05`/`13` call `notify()`; the gRPC `Notifications`
//! service + the `notification.events` streams subject + boot construction + the
//! live `notify_user`/`read_inbox_summary` wiring are the **service half** (see
//! the 507 handoff) — this module owns the logic, decoupled behind the
//! [`NotificationEvents`] + [`Clock`] seams so it is fully testable.

use std::sync::Arc;

use concerto_error::Result;
use concerto_persist::notifications::{self, NewDelivery, NewNotification};
use concerto_persist::{workspaces, Persistence, WorkspaceId};
use concerto_proto::v1 as pb;

use crate::notifications::chip_dispatch::{self, ActOutcome};
use crate::notifications::dedup::{self, DedupDecision, DEDUP_WINDOW_MS};
use crate::notifications::fanout::{self, ActiveViewing, NoActiveViewing};
use crate::notifications::model::{row_to_proto, NotifyRequest};
use crate::notifications::prefs;
use crate::notifications::push::{PushBackend, WakeupBody};

/// A `notification.events` broadcast (design/14 §5.3). The live impl publishes
/// onto the `notification.events` streams subject (the 507 service half); tests
/// use a recording double.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationEvent {
    Created(String),
    Updated(String),
    Read(String),
    Acted {
        id: String,
        chip_id: String,
        by_device_id: String,
    },
}

/// Sink for [`NotificationEvent`]s.
pub trait NotificationEvents: Send + Sync {
    fn emit(&self, event: NotificationEvent);
}

/// No-op events sink (used before the streams subject is wired).
pub struct NoEvents;
impl NotificationEvents for NoEvents {
    fn emit(&self, _event: NotificationEvent) {}
}

/// Clock seam so `notify()` is testable with synthetic time.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

/// Wall-clock.
pub struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// The notifications sub-system handle (design/14 §5.1).
#[derive(Clone)]
pub struct NotificationHandle {
    persist: Arc<Persistence>,
    push: Arc<dyn PushBackend>,
    active_viewing: Arc<dyn ActiveViewing>,
    events: Arc<dyn NotificationEvents>,
    clock: Arc<dyn Clock>,
    dedup_window_ms: i64,
}

impl NotificationHandle {
    pub fn new(
        persist: Arc<Persistence>,
        push: Arc<dyn PushBackend>,
        events: Arc<dyn NotificationEvents>,
    ) -> Self {
        Self {
            persist,
            push,
            active_viewing: Arc::new(NoActiveViewing),
            events,
            clock: Arc::new(SystemClock),
            dedup_window_ms: DEDUP_WINDOW_MS,
        }
    }

    /// Override the active-viewing oracle (wired once the device-tagged
    /// subscription seam lands, Task 504 handoff).
    pub fn with_active_viewing(mut self, oracle: Arc<dyn ActiveViewing>) -> Self {
        self.active_viewing = oracle;
        self
    }

    /// Override the clock (tests).
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Create a notification (design/14 §6.1): de-dup → insert-or-update →
    /// emit event → (if pushable) fan out the ID-only wakeup → record
    /// `delivered_at` + null stale tokens. Returns the (new or refreshed) id.
    pub async fn notify(&self, req: NotifyRequest) -> Result<String> {
        let now = self.clock.now_ms();

        // De-dup: a live unread duplicate within the window is refreshed, not
        // re-inserted, and no second wakeup is sent (design/14 §3.7).
        let existing = notifications::find_unread_for_dedup_key(
            self.persist.readers(),
            req.workspace_id.as_deref(),
            req.workarea_id.as_deref(),
            req.kind.as_db(),
            &req.subject_id,
            now - self.dedup_window_ms,
        )
        .await?;
        if let DedupDecision::UpdateExisting(id) =
            dedup::decide(existing.as_ref(), now, self.dedup_window_ms)
        {
            {
                let mut w = self.persist.writer().await;
                notifications::update_body_and_at(&mut w, &id, &req.body, now).await?;
            }
            self.events.emit(NotificationEvent::Updated(id.clone()));
            return Ok(id);
        }

        // Insert a fresh row.
        let id = uuid::Uuid::now_v7().to_string();
        let chips_json = if req.chips.is_empty() {
            None
        } else {
            serde_json::to_string(&req.chips).ok()
        };
        let approval_json = req
            .approval
            .as_ref()
            .and_then(|a| serde_json::to_string(a).ok());
        {
            let mut w = self.persist.writer().await;
            notifications::insert(
                &mut w,
                NewNotification {
                    id: id.clone(),
                    kind: req.kind.as_db().to_string(),
                    subject_kind: req.subject_kind.as_db().to_string(),
                    subject_id: req.subject_id.clone(),
                    workspace_id: req.workspace_id.clone(),
                    workarea_id: req.workarea_id.clone(),
                    session_id: req.session_id.clone(),
                    title: req.title.clone(),
                    body: req.body.clone(),
                    chips_json,
                    approval_json,
                    severity: req.effective_severity().as_db().to_string(),
                    created_at: now,
                },
            )
            .await?;
        }
        self.events.emit(NotificationEvent::Created(id.clone()));

        // Push decision (design/14 §3.8): per-workspace opt-out gate; the
        // eligible-device query already excludes DND + token-less devices.
        let opted_out = match &req.workspace_id {
            Some(ws) => {
                workspaces::get_settings_json(self.persist.readers(), &WorkspaceId(ws.clone()))
                    .await?
                    .map(|s| prefs::parse_workspace_opt_out(&s))
                    .unwrap_or(false)
            }
            None => false,
        };
        if prefs::should_push(req.kind, opted_out, None, now) {
            self.fan_out(&id, req.kind, req.workarea_id.as_deref(), now)
                .await?;
        }
        Ok(id)
    }

    /// Eligible-set → subtract active-viewing → deliver → record/clean up.
    async fn fan_out(
        &self,
        id: &str,
        kind: crate::notifications::model::NotificationKind,
        workarea_id: Option<&str>,
        now: i64,
    ) -> Result<()> {
        let rows = notifications::list_pushable_devices(self.persist.readers(), now).await?;
        let eligible = fanout::eligible_targets(rows);
        let viewing = self.active_viewing.actively_viewing(workarea_id);
        let targets = fanout::plan_fanout(eligible, &viewing);
        if targets.is_empty() {
            return Ok(());
        }
        let body = WakeupBody::new(id.to_string(), kind.as_db());
        let results = fanout::deliver(self.push.as_ref(), &targets, &body).await;
        for r in results {
            let mut w = self.persist.writer().await;
            if r.report.ok {
                notifications::upsert_delivery(
                    &mut w,
                    NewDelivery {
                        notification_id: id.to_string(),
                        device_id: r.device_id.clone(),
                        delivered_at: Some(now),
                        fetched_at: None,
                    },
                )
                .await?;
            } else if r.report.is_device_not_registered() {
                notifications::clear_push_token(&mut w, &r.device_id).await?;
            }
        }
        Ok(())
    }

    /// The chronological inbox feed (design/14 §5.1).
    pub async fn get_inbox(
        &self,
        workspace_id: Option<&str>,
        workarea_id: Option<&str>,
        unread_only: bool,
        limit: u32,
    ) -> Result<Vec<pb::Notification>> {
        let rows = notifications::list_inbox(
            self.persist.readers(),
            workspace_id,
            workarea_id,
            unread_only,
            limit,
        )
        .await?;
        Ok(rows.into_iter().map(row_to_proto).collect())
    }

    /// Post-wakeup fetch (records `fetched_at`).
    pub async fn get_notification(
        &self,
        id: &str,
        device_id: &str,
    ) -> Result<Option<pb::Notification>> {
        fanout::fetch_for_device(&self.persist, id, device_id, self.clock.now_ms()).await
    }

    /// Mark read (idempotent) + emit `read`.
    pub async fn mark_read(&self, id: &str) -> Result<()> {
        let now = self.clock.now_ms();
        let affected = {
            let mut w = self.persist.writer().await;
            notifications::mark_read(&mut w, id, now).await?
        };
        if affected > 0 {
            self.events.emit(NotificationEvent::Read(id.to_string()));
        }
        Ok(())
    }

    /// Act on a chip (design/14 §6.3). Returns the dispatch the caller (507
    /// service) executes against the supervisor; emits `acted` on a win.
    pub async fn act_on_chip(
        &self,
        id: &str,
        chip_id: &str,
        by_device_id: &str,
    ) -> Result<ActOutcome> {
        let now = self.clock.now_ms();
        let outcome =
            chip_dispatch::act_on_chip(&self.persist, id, chip_id, by_device_id, now).await?;
        if !outcome.already_resolved {
            self.events.emit(NotificationEvent::Acted {
                id: id.to_string(),
                chip_id: chip_id.to_string(),
                by_device_id: by_device_id.to_string(),
            });
        }
        Ok(outcome)
    }

    /// Set the per-workspace notification opt-out (design/14 §3.8 — the
    /// enterprise-private switch). RMW key on `workspaces.settings_json`
    /// (`notifications_opt_out`), the `exclude_from_maestro` precedent.
    pub async fn set_workspace_opt_out(&self, workspace_id: &str, opt_out: bool) -> Result<()> {
        let mut w = self.persist.writer().await;
        workspaces::set_settings_json_key(
            &mut w,
            &WorkspaceId(workspace_id.to_string()),
            "notifications_opt_out",
            serde_json::Value::Bool(opt_out),
        )
        .await
    }
}
