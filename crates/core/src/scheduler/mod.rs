//! Scheduler subsystem (Task 38, design/05).
//!
//! Owns the `/loop` primitive — session-scoped recurring tasks tied to
//! a workarea. V0.1 ships interval-based firing only (30..=604800
//! seconds, 3-day expiry, inflight suppression). Cron-based persistent
//! scheduled tasks, cloud-task sync, promote, and budget guardrails are
//! V1.0 per `tasks/38-scheduler-loop.md §"Scope — out"`.
//!
//! ## Module layout
//!
//! - [`actor`] — [`SchedulerActor`] + [`SchedulerHandle`].
//! - [`fire_loop`] — the tokio task that owns the `BTreeMap<Instant,
//!   ScheduleId>` next-fire wheel and the lifecycle-watcher spawn glue.
//!
//! ## Public surface
//!
//! [`SchedulerHandle::create_schedule`],
//! [`SchedulerHandle::list_schedules`],
//! [`SchedulerHandle::pause_schedule`],
//! [`SchedulerHandle::delete_schedule`],
//! [`SchedulerHandle::get_history`], and
//! [`SchedulerHandle::fire_now`] are FROZEN per Task 38.

#![cfg(unix)]

pub mod actor;
pub mod fire_loop;

pub use actor::{
    CreateScheduleRequest, SchedulerActor, SchedulerConfig, SchedulerHandle, INTERVAL_MAX_SECONDS,
    INTERVAL_MIN_SECONDS, LOOP_EXPIRY_DEFAULT_MS,
};
