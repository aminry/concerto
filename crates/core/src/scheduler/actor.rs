//! `SchedulerActor` + cloneable `SchedulerHandle` (Task 38).
//!
//! Follows the same actor pattern as the other Core managers
//! (`RepoManagerActor`, `WorkspaceManagerActor`, `WorkareaManagerActor`,
//! `AgentSupervisorActor`): the actor's `run` parks on shutdown; all
//! meaningful work flows through the cheap-to-clone handle.
//!
//! ## V0.1 surface
//!
//! - [`SchedulerHandle::create_schedule`] inserts a `kind='loop'` row,
//!   updates the in-memory next-fire wheel, and wakes the fire loop.
//! - [`SchedulerHandle::list_schedules`] reads the persisted rows for a
//!   workarea.
//! - [`SchedulerHandle::pause_schedule`] marks the row paused and evicts
//!   the next-fire entry.
//! - [`SchedulerHandle::delete_schedule`] deletes the row (runs cascade)
//!   and evicts the next-fire entry.
//! - [`SchedulerHandle::get_history`] reads the `schedule_runs` rows.
//! - [`SchedulerHandle::fire_now`] is a test-facing hook that drives the
//!   fire path synchronously without waiting on `sleep_until`.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use concerto_error::{Error, Result};
use concerto_persist::{
    NewSchedule, NewScheduleRun, Persistence, RepositoryId, Schedule, ScheduleId, ScheduleRun,
    ScheduleRunId, SessionId, WorkareaId,
};
use sqlx::Connection;
use tokio::sync::{Mutex, Notify};

use crate::agent_supervisor::{AgentEvent, AgentKind, AgentSupervisorHandle, StartSessionRequest};
use crate::scheduler::wait_checks::{self, CheckRunsSource, ChecksOutcome, RequiredChecks};
use crate::supervisor::{Actor, ActorContext};

/// Minimum interval seconds. Frozen per Task 38 §"Public interface this
/// task locks". Matches `design/05 §12 R-3`.
pub const INTERVAL_MIN_SECONDS: i64 = 30;

/// Maximum interval seconds (7 days). Frozen per Task 38.
pub const INTERVAL_MAX_SECONDS: i64 = 7 * 24 * 3600;

/// Default loop expiry — 3 days from creation per `design/05 §1`.
pub const LOOP_EXPIRY_DEFAULT_MS: i64 = 3 * 24 * 3600 * 1000;

/// Sweep interval for the expiration task. Per Task 38 §"Implementation
/// notes": "every 5 min".
pub const EXPIRATION_SWEEP_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Caller-supplied parameters for [`SchedulerHandle::create_schedule`].
#[derive(Clone, Debug)]
pub struct CreateScheduleRequest {
    pub workarea_id: WorkareaId,
    /// V0.1: always `"loop"`. Other values are rejected with
    /// `INVALID_ARGUMENT`.
    pub kind: String,
    pub interval_seconds: i64,
    pub prompt: String,
    /// One of `claude|codex|gemini|maestro`. Empty string defaults to
    /// `"claude"`. Validated against the schema CHECK set.
    pub agent_kind: String,
    /// Unix epoch milliseconds; `None` (or `Some(0)`) means
    /// "use the design default of `now + 3 days`".
    pub expires_at_unix_ms: Option<i64>,
}

/// Config for the actor's `run` loop. V0.1 has no knobs — the actor
/// parks on shutdown.
#[derive(Clone, Debug, Default)]
pub struct SchedulerConfig;

/// Supervised actor that owns the schedule fire loop.
pub struct SchedulerActor {
    handle: SchedulerHandle,
}

/// Cheap-cloneable, shareable handle to the Scheduler's state. All
/// meaningful work flows through this struct; the actor's `run` parks on
/// shutdown.
#[derive(Clone)]
pub struct SchedulerHandle {
    persistence: Arc<Persistence>,
    /// Set when the Scheduler is wired against a live supervisor.
    ///
    /// Tests can construct a handle without one and exercise the
    /// persistence + suppression paths; the fire loop refuses to spawn
    /// sessions without a supervisor and emits a
    /// `scheduler.no_supervisor` log instead.
    agent_supervisor: Option<AgentSupervisorHandle>,
    /// Next-fire wheel. Keyed by absolute `tokio::time::Instant` so the
    /// fire loop's `sleep_until` can target the next entry without an
    /// extra system-time round-trip.
    wheel: Arc<Mutex<BTreeMap<tokio::time::Instant, ScheduleId>>>,
    /// Per-schedule inflight watermark. The fire path checks this map
    /// before consulting the DB so the common case (no inflight) is a
    /// lock-only check.
    inflight: Arc<Mutex<HashMap<ScheduleId, ScheduleRunId>>>,
    /// Wakes the fire loop on add / update / delete. Capacity 1 — the
    /// loop just re-reads the wheel on every wake.
    notify: Arc<Notify>,
    /// The check-runs source [`SchedulerHandle::wait_for_check_runs`] polls
    /// (Task 318). Set post-construction by [`set_check_runs_source`] because
    /// the VCS handle is built after the Scheduler in `boot.rs`; a
    /// `std::sync::OnceLock` mirrors the `OnceCell` interior-mutability pattern
    /// the rest of Core uses, keeping `SchedulerHandle::new`'s V0.1 signature
    /// unchanged. `None` (unset) ⇒ `wait_for_check_runs` returns the typed
    /// `scheduler.no_vcs_source` error. Independent of the fire wheel.
    ///
    /// [`set_check_runs_source`]: SchedulerHandle::set_check_runs_source
    check_runs_source: Arc<std::sync::OnceLock<Arc<dyn CheckRunsSource>>>,
}

impl SchedulerHandle {
    /// Build a fresh handle. Production callers go through
    /// [`SchedulerActor::new`] which also wires the supervisor; tests
    /// can construct a handle without an agent supervisor to exercise
    /// the persistence + suppression paths.
    pub fn new(
        persistence: Arc<Persistence>,
        agent_supervisor: Option<AgentSupervisorHandle>,
    ) -> Self {
        Self {
            persistence,
            agent_supervisor,
            wheel: Arc::new(Mutex::new(BTreeMap::new())),
            inflight: Arc::new(Mutex::new(HashMap::new())),
            notify: Arc::new(Notify::new()),
            check_runs_source: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// Wire the check-runs source for [`wait_for_check_runs`] (Task 318).
    /// Called once, post-construction, from `boot.rs` after the VCS handle is
    /// built (the handle does not exist when the Scheduler is constructed).
    /// Idempotent-on-first-set: a second call is ignored (logged) so a
    /// supervisor restart that re-wires cannot panic. Returns whether the source
    /// was installed by this call.
    ///
    /// [`wait_for_check_runs`]: SchedulerHandle::wait_for_check_runs
    pub fn set_check_runs_source(&self, source: Arc<dyn CheckRunsSource>) -> bool {
        match self.check_runs_source.set(source) {
            Ok(()) => true,
            Err(_) => {
                tracing::debug!(
                    "scheduler.set_check_runs_source: source already set; ignoring re-wire"
                );
                false
            }
        }
    }

    /// Wait for the check runs on `repo`'s commit `sha` to resolve (Task 318,
    /// `design/05 §3.9`/§5.1 — the FROZEN gate Task 320's coordinated PR-set
    /// merge blocks on between members).
    ///
    /// Polls the wired [`CheckRunsSource`] with the FROZEN
    /// `[1, 2, 4, 8, 16, 30]`-cap backoff (`design/13 §3.3` ==
    /// `design/05 §3.9`, imported from `concerto_vcs::rate_limit`), resolving to
    /// a [`ChecksOutcome`] when every run in the caller-supplied `required` set
    /// reaches a terminal conclusion, **or** when the wall-clock `timeout`
    /// elapses (a timeout resolves with `timed_out: true` — NOT an `Err`; the
    /// caller decides what to do, `design/05 §8`). When the source exposes a
    /// webhook wake (Task 315's `checks.<wa>.<repo>` emits via Task 316's
    /// `ChecksAggregator`), a `check_run` event short-circuits the current
    /// backoff sleep and triggers an immediate re-poll; absent it, the loop
    /// degrades to pure polling.
    ///
    /// `required` defaults to [`RequiredChecks::AllTerminal`] (the set 320 uses
    /// — "all check-runs for the SHA reach a terminal conclusion"); no
    /// branch-protection API is read (`PHASE3_PLANNING §2`).
    ///
    /// Errors only when no source is wired (`scheduler.no_vcs_source`) or the
    /// source itself errors on a poll. Independent of the `/loop` fire wheel —
    /// safe to await on 320's own task.
    pub async fn wait_for_check_runs(
        &self,
        repo: RepositoryId,
        sha: &str,
        timeout: Duration,
        required: RequiredChecks,
    ) -> Result<ChecksOutcome> {
        let source = self
            .check_runs_source
            .get()
            .ok_or_else(wait_checks::no_vcs_source)?;
        wait_checks::run_wait_loop(source, &repo, sha, timeout, required).await
    }

    /// Borrow the shared persistence handle. Used by the gRPC
    /// `Schedules` handler so it doesn't need a separate
    /// `Arc<Persistence>` plumbed through `api_server`.
    pub fn persistence(&self) -> Arc<Persistence> {
        Arc::clone(&self.persistence)
    }

    /// Borrow the in-memory next-fire wheel. Used by the fire loop and
    /// the test-facing `fire_now`.
    pub(crate) fn wheel(&self) -> Arc<Mutex<BTreeMap<tokio::time::Instant, ScheduleId>>> {
        Arc::clone(&self.wheel)
    }

    /// Borrow the inflight map. The fire loop reads it to short-circuit
    /// the suppression check; the lifecycle watcher clears it when the
    /// run terminates.
    pub(crate) fn inflight(&self) -> Arc<Mutex<HashMap<ScheduleId, ScheduleRunId>>> {
        Arc::clone(&self.inflight)
    }

    /// Borrow the wake-up notifier so the fire loop can park on it.
    pub(crate) fn notify(&self) -> Arc<Notify> {
        Arc::clone(&self.notify)
    }

    /// Borrow the optional supervisor handle. Used by the fire loop
    /// when it actually wants to spawn a session.
    pub(crate) fn agent_supervisor(&self) -> Option<AgentSupervisorHandle> {
        self.agent_supervisor.clone()
    }

    /// Create a `/loop` schedule. Validates the workarea exists,
    /// interval bounds, and agent_kind; defaults `expires_at` to
    /// `now + 3 days` when missing.
    pub async fn create_schedule(&self, req: CreateScheduleRequest) -> Result<Schedule> {
        // V0.1: only kind=loop is supported. Reject any other value at
        // the API layer — the SQL CHECK constraint is the safety net.
        if req.kind != "loop" {
            return Err(Error::Validation(format!(
                "schedule.kind_unsupported: V0.1 supports kind=\"loop\" only (got {:?})",
                req.kind
            )));
        }
        if req.interval_seconds < INTERVAL_MIN_SECONDS
            || req.interval_seconds > INTERVAL_MAX_SECONDS
        {
            return Err(Error::Validation(format!(
                "schedule.interval_out_of_bounds: must be {}..={}, got {}",
                INTERVAL_MIN_SECONDS, INTERVAL_MAX_SECONDS, req.interval_seconds
            )));
        }
        if req.prompt.trim().is_empty() {
            return Err(Error::Validation(
                "schedule.prompt_required: prompt must not be empty".into(),
            ));
        }
        let agent_kind = if req.agent_kind.trim().is_empty() {
            "claude".to_string()
        } else {
            match req.agent_kind.as_str() {
                "claude" | "codex" | "gemini" | "maestro" => req.agent_kind.clone(),
                other => {
                    return Err(Error::Validation(format!(
                        "schedule.agent_kind_unsupported: must be one of claude|codex|gemini|maestro, got {:?}",
                        other
                    )));
                }
            }
        };

        let workarea =
            concerto_persist::workareas::get(self.persistence.readers(), &req.workarea_id)
                .await?
                .ok_or_else(|| {
                    Error::NotFound(format!("workarea {} not found", req.workarea_id))
                })?;
        if workarea.archived_at.is_some() {
            return Err(Error::Validation(format!(
                "workarea.archived: workarea {} is archived",
                req.workarea_id
            )));
        }

        let now_ms = now_unix_ms();
        let expires_at = match req.expires_at_unix_ms {
            Some(v) if v > 0 => v,
            _ => now_ms + LOOP_EXPIRY_DEFAULT_MS,
        };
        if expires_at <= now_ms {
            return Err(Error::Validation(format!(
                "schedule.expired: expires_at ({expires_at}) must be in the future (now={now_ms})"
            )));
        }

        let id = ScheduleId(uuid::Uuid::now_v7().to_string());
        let row = NewSchedule {
            id: id.clone(),
            workarea_id: req.workarea_id.clone(),
            kind: req.kind.clone(),
            interval_seconds: req.interval_seconds,
            expires_at,
            last_run_at: None,
            paused: false,
            prompt: req.prompt.clone(),
            agent_kind,
            created_at: now_ms,
        };
        {
            let mut writer = self.persistence.writer().await;
            concerto_persist::schedules::insert(&mut writer, row).await?;
        }

        // Read back so the wire shape mirrors persistence exactly.
        let inserted = concerto_persist::schedules::get(self.persistence.readers(), &id)
            .await?
            .ok_or_else(|| Error::Internal("schedule row missing after insert".into()))?;

        // Stamp the wheel for `now + interval` and wake the fire loop.
        let next_fire =
            tokio::time::Instant::now() + Duration::from_secs(inserted.interval_seconds as u64);
        {
            let mut wheel = self.wheel.lock().await;
            wheel.insert(next_fire, id.clone());
        }
        self.notify.notify_one();

        Ok(inserted)
    }

    /// List schedules for a workarea, oldest first.
    pub async fn list_schedules(&self, workarea_id: &WorkareaId) -> Result<Vec<Schedule>> {
        concerto_persist::schedules::list_by_workarea(self.persistence.readers(), workarea_id).await
    }

    /// Pause a schedule. Idempotent; evicts the wheel entry.
    pub async fn pause_schedule(&self, id: &ScheduleId) -> Result<Schedule> {
        let _existing = concerto_persist::schedules::get(self.persistence.readers(), id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("schedule {id} not found")))?;
        {
            let mut writer = self.persistence.writer().await;
            concerto_persist::schedules::pause(&mut writer, id).await?;
        }
        self.evict_wheel(id).await;
        self.notify.notify_one();
        let updated = concerto_persist::schedules::get(self.persistence.readers(), id)
            .await?
            .ok_or_else(|| Error::Internal("schedule row missing after pause".into()))?;
        Ok(updated)
    }

    /// Delete a schedule. Cascades to `schedule_runs` via the FK.
    pub async fn delete_schedule(&self, id: &ScheduleId) -> Result<()> {
        let _existing = concerto_persist::schedules::get(self.persistence.readers(), id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("schedule {id} not found")))?;
        {
            let mut writer = self.persistence.writer().await;
            concerto_persist::schedules::delete(&mut writer, id).await?;
        }
        self.evict_wheel(id).await;
        {
            let mut inflight = self.inflight.lock().await;
            inflight.remove(id);
        }
        self.notify.notify_one();
        Ok(())
    }

    /// Return the run history (newest first) for a schedule. Pure read.
    pub async fn get_history(&self, id: &ScheduleId) -> Result<Vec<ScheduleRun>> {
        // 404 first so the empty-list case is unambiguous.
        concerto_persist::schedules::get(self.persistence.readers(), id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("schedule {id} not found")))?;
        concerto_persist::schedule_runs::list_by_schedule(self.persistence.readers(), id).await
    }

    /// Force-fire a schedule synchronously. Test-facing — the production
    /// fire loop fires via `sleep_until` + `notify`. Returns `Ok(None)`
    /// when the fire was suppressed because a prior run is still in
    /// flight; returns `Ok(Some(run_id))` when a session was started.
    ///
    /// Does NOT update the wheel — the caller is expected to either be
    /// testing suppression (in which case the wheel is irrelevant) or
    /// expecting the fire loop to schedule the next entry as usual.
    pub async fn fire_now(&self, id: &ScheduleId) -> Result<Option<ScheduleRunId>> {
        fire_schedule(self, id).await
    }

    /// Rebuild the next-fire wheel from persistence. Called on Core boot
    /// (and on demand by the fire loop the first time it parks).
    pub async fn rebuild_wheel(&self) -> Result<usize> {
        let now_ms = now_unix_ms();
        let active =
            concerto_persist::schedules::list_active(self.persistence.readers(), now_ms).await?;
        let now_instant = tokio::time::Instant::now();
        let mut wheel = self.wheel.lock().await;
        wheel.clear();
        let mut count = 0;
        for s in &active {
            // next_fire = max(last_run + interval, now). Computed in
            // wall-clock ms first so the boot-time recovery honors the
            // persisted last_run; then mapped to the monotonic Instant
            // axis the fire loop runs on.
            let interval_ms = s.interval_seconds * 1000;
            let next_ms = match s.last_run_at {
                Some(last) => (last + interval_ms).max(now_ms),
                None => now_ms + interval_ms,
            };
            let delay_ms = next_ms.saturating_sub(now_ms);
            let when = now_instant + Duration::from_millis(delay_ms as u64);
            // Use a unique instant per schedule by adding nanoseconds —
            // a BTreeMap keyed on Instant collapses duplicates so two
            // schedules that compute the same next_ms would lose one.
            let key = when + Duration::from_nanos(count as u64);
            wheel.insert(key, s.id.clone());
            count += 1;
        }
        Ok(count)
    }

    /// Run the expiration sweep once. The fire loop wakes a separate
    /// task on a 5-minute ticker that calls this; tests can call it
    /// directly to avoid waiting on the ticker.
    pub async fn run_expiration_sweep(&self) -> Result<u64> {
        let now_ms = now_unix_ms();
        let paused = {
            let mut writer = self.persistence.writer().await;
            concerto_persist::schedules::pause_expired(&mut writer, now_ms).await?
        };
        if paused > 0 {
            // Conservative: rebuild the wheel so any newly-paused
            // entries are dropped. Cheap for V0.1 (a single SELECT
            // bounded by the number of active schedules).
            let _ = self.rebuild_wheel().await?;
            self.notify.notify_one();
        }
        Ok(paused)
    }

    /// Evict every wheel entry that matches `id`. Cheap O(N) walk over
    /// the BTreeMap — V0.1 schedule counts are small.
    async fn evict_wheel(&self, id: &ScheduleId) {
        let mut wheel = self.wheel.lock().await;
        let keys: Vec<tokio::time::Instant> = wheel
            .iter()
            .filter_map(|(k, v)| if v == id { Some(*k) } else { None })
            .collect();
        for k in keys {
            wheel.remove(&k);
        }
    }
}

impl SchedulerActor {
    /// Build a new actor with a wired supervisor handle.
    pub fn new(persistence: Arc<Persistence>, supervisor: AgentSupervisorHandle) -> Self {
        Self {
            handle: SchedulerHandle::new(persistence, Some(supervisor)),
        }
    }

    /// Cheap clone of the shared handle.
    pub fn handle(&self) -> SchedulerHandle {
        self.handle.clone()
    }
}

#[async_trait]
impl Actor for SchedulerActor {
    const NAME: &'static str = "scheduler";
    type Config = SchedulerConfig;

    async fn run(self, ctx: ActorContext<Self::Config>) -> Result<()> {
        // Rebuild the wheel from persistence on first start. Cold-start
        // path: the wheel will be empty; idempotent on restart.
        match self.handle.rebuild_wheel().await {
            Ok(0) => tracing::debug!("scheduler.rebuild_wheel: no active schedules"),
            Ok(n) => tracing::info!(active = n, "scheduler.rebuild_wheel complete"),
            Err(e) => tracing::warn!(error = %e, "scheduler.rebuild_wheel failed"),
        }

        // Spawn the fire loop + the expiration sweep as background tasks
        // owned by this actor; they exit when `ctx.shutdown` fires
        // (propagated via clones).
        let fire_handle = self.handle.clone();
        let fire_shutdown = ctx.shutdown.clone();
        let fire_task = tokio::spawn(async move {
            super::fire_loop::run_fire_loop(fire_handle, fire_shutdown).await;
        });
        let sweep_handle = self.handle.clone();
        let sweep_shutdown = ctx.shutdown.clone();
        let sweep_task = tokio::spawn(async move {
            super::fire_loop::run_expiration_sweep(sweep_handle, sweep_shutdown).await;
        });

        tracing::info!("Scheduler ready");
        ctx.shutdown.cancelled().await;
        tracing::debug!("Scheduler actor shutting down");
        // Best-effort join; tasks are also tied to the cancellation
        // token so they'll exit promptly.
        let _ = fire_task.await;
        let _ = sweep_task.await;
        Ok(())
    }
}

/// Wall-clock unix-epoch milliseconds.
pub(crate) fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Execute one fire of a schedule.
///
/// 1. Inflight suppression check — both the in-memory map and the DB
///    (`schedule_runs WHERE ended_at IS NULL`).
/// 2. Insert a `schedule_runs` row with `ended_at = NULL`.
/// 3. Call `AgentSupervisorHandle::start_session` with the schedule's
///    prompt + workarea + agent_kind.
/// 4. Patch the `schedule_runs.session_id` once the supervisor returns.
/// 5. Patch `schedules.last_run_at`.
/// 6. Spawn a background watcher that listens on `subscribe_events`
///    and resolves the run to `completed|crashed` on terminal events.
///
/// Returns `Ok(None)` when the fire was suppressed; `Ok(Some(run_id))`
/// when a session was successfully started.
pub(crate) async fn fire_schedule(
    handle: &SchedulerHandle,
    schedule_id: &ScheduleId,
) -> Result<Option<ScheduleRunId>> {
    // Re-read the row so we always have the latest prompt + agent_kind
    // + paused state. A schedule that was paused after the wheel popped
    // its key should still skip cleanly.
    let row = match concerto_persist::schedules::get(handle.persistence().readers(), schedule_id)
        .await?
    {
        Some(r) => r,
        None => {
            tracing::warn!(schedule = %schedule_id, "scheduler.fire: schedule row missing");
            return Ok(None);
        }
    };
    if row.paused {
        tracing::debug!(schedule = %schedule_id, "scheduler.fire: schedule is paused");
        return Ok(None);
    }
    let now_ms = now_unix_ms();
    if row.expires_at <= now_ms {
        tracing::debug!(schedule = %schedule_id, "scheduler.fire: schedule expired");
        return Ok(None);
    }

    // Inflight suppression: in-memory map first, DB second so a Core
    // restart that lost the map still honours the suppression.
    let persistence = handle.persistence();
    let inflight_map = handle.inflight();
    {
        let inflight = inflight_map.lock().await;
        if inflight.contains_key(schedule_id) {
            tracing::info!(
                schedule = %schedule_id,
                reason = "inflight",
                "schedule.suppressed"
            );
            return Ok(None);
        }
    }
    if let Some(_run) =
        concerto_persist::schedule_runs::current_inflight(persistence.readers(), schedule_id)
            .await?
    {
        tracing::info!(
            schedule = %schedule_id,
            reason = "inflight",
            "schedule.suppressed"
        );
        return Ok(None);
    }

    // Insert the run row first so the suppression window is honored
    // even if start_session takes time.
    let run_id = ScheduleRunId(uuid::Uuid::now_v7().to_string());
    {
        let mut writer = persistence.writer().await;
        concerto_persist::schedule_runs::insert(
            &mut writer,
            NewScheduleRun {
                id: run_id.clone(),
                schedule_id: schedule_id.clone(),
                session_id: None,
                started_at: now_ms,
            },
        )
        .await?;
    }
    {
        let mut inflight = inflight_map.lock().await;
        inflight.insert(schedule_id.clone(), run_id.clone());
    }

    // Resolve the supervisor + workarea cwd. Without a supervisor we
    // mark the run failed and unwind.
    let supervisor = match handle.agent_supervisor() {
        Some(s) => s,
        None => {
            tracing::warn!(
                schedule = %schedule_id,
                "scheduler.no_supervisor: cannot start session"
            );
            mark_run_failed(handle, &run_id).await;
            return Err(Error::Internal(
                "scheduler.no_supervisor: SchedulerHandle has no AgentSupervisorHandle".into(),
            ));
        }
    };

    let workarea = concerto_persist::workareas::get(persistence.readers(), &row.workarea_id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("workarea {} not found", row.workarea_id)))?;
    let cwd = PathBuf::from(&workarea.worktree_root);
    let agent_kind = match row.agent_kind.as_str() {
        "claude" => AgentKind::Claude,
        "codex" => AgentKind::Codex,
        "gemini" => AgentKind::Gemini,
        other => {
            mark_run_failed(handle, &run_id).await;
            return Err(Error::Internal(format!(
                "scheduler: unsupported agent_kind {other:?} on schedule {schedule_id}"
            )));
        }
    };

    let start_req = StartSessionRequest {
        workarea_id: row.workarea_id.clone(),
        agent_kind,
        echo_text: None,
        cwd,
        permission_mode: None,
        resume_session_id: None,
    };
    let session_id = match supervisor.start_session(start_req).await {
        Ok(sid) => sid,
        Err(e) => {
            tracing::warn!(
                schedule = %schedule_id,
                error = %e,
                "scheduler.fire: start_session failed"
            );
            mark_run_failed(handle, &run_id).await;
            return Err(e);
        }
    };

    // Persist the session_id on the run, then bump last_run_at on the
    // schedule. Single transaction so a crash between the two leaves
    // the schedule untouched (the run row already has session_id NULL,
    // which is the inert state — the watcher will resolve it on its
    // own).
    {
        let mut writer = persistence.writer().await;
        let mut tx = writer.begin().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
        concerto_persist::schedule_runs::update_session(&mut tx, &run_id, &session_id).await?;
        concerto_persist::schedules::update_last_run(&mut tx, schedule_id, now_ms).await?;
        tx.commit().await.map_err(|e| Error::Sqlx(Box::new(e)))?;
    }

    // Spawn a background watcher: when the session emits TurnComplete
    // or Exited, mark the run terminal and clear the inflight slot.
    spawn_run_watcher(
        handle.clone(),
        supervisor,
        run_id.clone(),
        session_id,
        schedule_id.clone(),
    );

    Ok(Some(run_id))
}

/// Mark a run as `failed` and clear the inflight slot. Best-effort
/// cleanup used when an early-stage error trips before the session is
/// alive.
async fn mark_run_failed(handle: &SchedulerHandle, run_id: &ScheduleRunId) {
    let now = now_unix_ms();
    let persistence = handle.persistence();
    {
        let mut writer = persistence.writer().await;
        if let Err(e) =
            concerto_persist::schedule_runs::update_terminal(&mut writer, run_id, now, "failed")
                .await
        {
            tracing::warn!(
                error = %e,
                run = %run_id,
                "scheduler.mark_failed: update_terminal failed"
            );
        }
    }
    let inflight_map = handle.inflight();
    let mut inflight = inflight_map.lock().await;
    inflight.retain(|_, v| v != run_id);
}

/// Spawn a tokio task that subscribes to the session's event stream and
/// resolves the run row on the first terminal event. The fire loop must
/// not await this — long sessions are expected.
fn spawn_run_watcher(
    handle: SchedulerHandle,
    supervisor: AgentSupervisorHandle,
    run_id: ScheduleRunId,
    session_id: SessionId,
    schedule_id: ScheduleId,
) {
    tokio::spawn(async move {
        // Subscribe (with replay so a fast-finishing session doesn't
        // race past us). `subscribe_events_with_replay` returns None
        // only if the session entry has already been evicted — treat
        // that as an immediate completion.
        let (replay, mut rx) = match supervisor.subscribe_events_with_replay(&session_id).await {
            Some(pair) => pair,
            None => {
                resolve_run(&handle, &run_id, &schedule_id, "completed").await;
                return;
            }
        };
        for ev in replay {
            if let Some(state) = terminal_state_of(&ev) {
                resolve_run(&handle, &run_id, &schedule_id, state).await;
                return;
            }
        }
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if let Some(state) = terminal_state_of(&ev) {
                        resolve_run(&handle, &run_id, &schedule_id, state).await;
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    // Channel closed without a terminal event — treat
                    // as completed; the session entry was evicted.
                    resolve_run(&handle, &run_id, &schedule_id, "completed").await;
                    return;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Keep going; we may still observe a terminal
                    // event.
                    continue;
                }
            }
        }
    });
}

/// Map an `AgentEvent` to one of the `schedule_runs.terminal_state`
/// values, or `None` if the event is not terminal.
fn terminal_state_of(ev: &AgentEvent) -> Option<&'static str> {
    match ev {
        AgentEvent::TurnComplete { .. } => Some("completed"),
        AgentEvent::Exited {
            exit_code, signal, ..
        } => {
            if signal.is_some() || matches!(exit_code, Some(c) if *c != 0) {
                Some("crashed")
            } else {
                Some("completed")
            }
        }
        _ => None,
    }
}

/// Resolve a run row + clear the inflight slot.
async fn resolve_run(
    handle: &SchedulerHandle,
    run_id: &ScheduleRunId,
    schedule_id: &ScheduleId,
    state: &str,
) {
    let now = now_unix_ms();
    let persistence = handle.persistence();
    {
        let mut writer = persistence.writer().await;
        if let Err(e) =
            concerto_persist::schedule_runs::update_terminal(&mut writer, run_id, now, state).await
        {
            tracing::warn!(
                error = %e,
                run = %run_id,
                "scheduler.resolve_run: update_terminal failed"
            );
        }
    }
    let inflight_map = handle.inflight();
    let mut inflight = inflight_map.lock().await;
    if let Some(v) = inflight.get(schedule_id) {
        if v == run_id {
            inflight.remove(schedule_id);
        }
    }
}
