//! Scheduler fire loop (Task 38).
//!
//! Owns the `BTreeMap<Instant, ScheduleId>` next-fire wheel. Parks in a
//! `select!` on `sleep_until(next_fire)` and `notify.notified()`; the
//! notifier wakes the loop on schedule add / update / delete so the
//! sleep target is always current.
//!
//! Per-fire side effects (`schedule_runs` insert, supervisor
//! `start_session`, lifecycle watcher spawn) live in
//! [`super::actor::fire_schedule`]; this module is the wheel + the
//! park loop only.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::actor::{fire_schedule, SchedulerHandle, EXPIRATION_SWEEP_INTERVAL};

/// Idle delay when the wheel is empty. Bounded so the loop wakes
/// periodically and notices schedules added without a `notify_one`
/// (defensive; the create / pause / delete paths all call
/// `notify_one`).
const EMPTY_WHEEL_PARK: Duration = Duration::from_secs(60);

/// Run the fire loop until `shutdown` is cancelled.
///
/// Each iteration:
/// 1. Lock the wheel to read the first entry's instant + id.
/// 2. Park in `select!` on either `sleep_until(instant)`,
///    `notify.notified()`, or `shutdown.cancelled()`.
/// 3. On `sleep_until` resolving: pop the entry, fire the schedule,
///    and re-stamp the schedule for `now + interval`.
/// 4. On notify or shutdown: loop / exit.
pub async fn run_fire_loop(handle: SchedulerHandle, shutdown: CancellationToken) {
    let wheel = handle.wheel();
    let notify = handle.notify();

    loop {
        if shutdown.is_cancelled() {
            return;
        }
        // Snapshot the next entry without holding the wheel lock across
        // `await` points.
        let next = {
            let map = wheel.lock().await;
            map.iter().next().map(|(k, v)| (*k, v.clone()))
        };
        let park = match next {
            Some((when, _)) => when,
            None => tokio::time::Instant::now() + EMPTY_WHEEL_PARK,
        };
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = notify.notified() => {
                // Wheel mutated; loop again to read the new head.
                continue;
            }
            _ = tokio::time::sleep_until(park) => {
                let (when, sched_id) = match next {
                    Some(pair) => pair,
                    None => continue,
                };
                // Pop the entry. If something else evicted it between
                // the snapshot and now, skip.
                {
                    let mut map = wheel.lock().await;
                    if map.get(&when).map(|v| v == &sched_id).unwrap_or(false) {
                        map.remove(&when);
                    } else {
                        continue;
                    }
                }
                // Re-read the schedule to get the canonical interval
                // (and to honour any updates the caller has made).
                let row_opt = match concerto_persist::schedules::get(
                    handle.persistence().readers(),
                    &sched_id,
                )
                .await
                {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            schedule = %sched_id,
                            "scheduler.fire_loop: schedule get failed"
                        );
                        continue;
                    }
                };
                let row = match row_opt {
                    Some(r) => r,
                    None => continue, // schedule was deleted between pop + read
                };
                // Re-arm BEFORE firing so a slow `start_session` doesn't
                // push the next fire arbitrarily far out.
                if !row.paused {
                    let next_when = tokio::time::Instant::now()
                        + Duration::from_secs(row.interval_seconds as u64);
                    let mut map = wheel.lock().await;
                    // Unique key — nanoseconds bump if the slot is
                    // taken; cheap O(retries) loop, retries are
                    // vanishingly rare.
                    let mut key = next_when;
                    let mut nano: u64 = 0;
                    while map.contains_key(&key) {
                        nano += 1;
                        key = next_when + Duration::from_nanos(nano);
                    }
                    map.insert(key, sched_id.clone());
                }
                // Fire (the function checks inflight + expired itself).
                if let Err(e) = fire_schedule(&handle, &sched_id).await {
                    tracing::warn!(
                        error = %e,
                        schedule = %sched_id,
                        "scheduler.fire_loop: fire failed"
                    );
                }
            }
        }
    }
}

/// Run the expiration sweep on a 5-minute ticker until shutdown.
pub async fn run_expiration_sweep(handle: SchedulerHandle, shutdown: CancellationToken) {
    let mut ticker = tokio::time::interval(EXPIRATION_SWEEP_INTERVAL);
    // Skip the immediate first tick; the cold-boot `rebuild_wheel`
    // already filters expired rows out.
    ticker.tick().await;
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = ticker.tick() => {
                match handle.run_expiration_sweep().await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(
                        paused = n,
                        "scheduler.expiration_sweep: paused expired schedules"
                    ),
                    Err(e) => tracing::warn!(
                        error = %e,
                        "scheduler.expiration_sweep failed"
                    ),
                }
            }
        }
    }
}
