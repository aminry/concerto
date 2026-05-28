//! fsmonitor lifecycle: bring-up, supervision, restart-cap bookkeeping.
//!
//! Task 28 (`design/02 §3.4`, §8) requires `git fsmonitor--daemon` to be
//! kept alive per repository: a dead daemon is restarted, but only up to
//! three times in any rolling 60-second window. The fourth death disables
//! the daemon for that repository until the next Core start — a noisy
//! restart loop almost always means the underlying filesystem refused
//! the daemon (NFS, certain tmpfs / FUSE backends), and hammering it
//! forever wastes CPU and pollutes the log.
//!
//! The supervisor loop is intentionally small: every 30s it walks the
//! `repositories` table, probes each recorded PID, and restarts dead
//! daemons subject to the per-repo restart-history budget.
//!
//! The restart-history bookkeeping is exposed as a free function
//! ([`record_restart`]) so the integration test can exercise the policy
//! against a deterministic mock alive-check without spinning the real
//! 30s loop.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use concerto_error::Result;
use concerto_gix_wrap as gixw;
use concerto_persist::{Persistence, Repository, RepositoryId};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Restart-history window per `design/02 §8` / task 28 §"Restart policy".
const RESTART_WINDOW: Duration = Duration::from_secs(60);

/// Maximum restarts the supervisor will perform within
/// [`RESTART_WINDOW`] before disabling fsmonitor for the repo.
const MAX_RESTARTS_IN_WINDOW: usize = 3;

/// How often the supervisor walks every repo. Sized for "cheap enough
/// to keep on hand" — the inner check is a stat-equivalent syscall per
/// row and a DB read.
pub const SUPERVISOR_INTERVAL: Duration = Duration::from_secs(30);

/// Per-repo ring of recent restart timestamps. The supervisor's shared
/// state is `Arc<Mutex<HashMap<RepositoryId, RestartHistory>>>`.
#[derive(Debug, Default, Clone)]
pub struct RestartHistory {
    /// Restart timestamps, oldest first. Bounded at
    /// `MAX_RESTARTS_IN_WINDOW` entries — we discard entries older than
    /// [`RESTART_WINDOW`] on every probe.
    pub recent: VecDeque<Instant>,
    /// `true` once the cap was breached. The supervisor leaves the
    /// daemon disabled until a Core restart clears in-memory state.
    pub disabled: bool,
}

impl RestartHistory {
    fn prune_older_than(&mut self, now: Instant, window: Duration) {
        while let Some(front) = self.recent.front().copied() {
            if now.duration_since(front) > window {
                self.recent.pop_front();
            } else {
                break;
            }
        }
    }
}

/// Outcome of a single supervisor probe for one repo. Carried back to
/// the caller so the supervisor (or a test) can persist the new PID
/// without re-implementing the policy.
#[derive(Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// Daemon is alive; no action needed.
    Alive,
    /// Daemon was missing or dead; supervisor restarted it and the
    /// caller should persist this new PID.
    Restarted { pid: u32 },
    /// Daemon was missing or dead; the supervisor exceeded the restart
    /// cap and disabled the repo. Caller should clear the recorded PID.
    Disabled,
    /// Daemon was missing or dead; the restart attempt itself failed.
    /// Caller should clear the recorded PID and rely on the next probe
    /// (and the cap) to either retry or disable.
    RestartFailed,
}

/// Record a fresh restart attempt at `now`, returning whether the cap
/// has been breached after this attempt. Pure bookkeeping; the caller
/// owns the side effects (spawning the daemon, persisting the PID).
///
/// The history is pruned of entries older than [`RESTART_WINDOW`]
/// before counting, so a quiet repo never reaches the cap.
pub fn record_restart(history: &mut RestartHistory, now: Instant) -> bool {
    history.prune_older_than(now, RESTART_WINDOW);
    history.recent.push_back(now);
    while history.recent.len() > MAX_RESTARTS_IN_WINDOW {
        history.recent.pop_front();
    }
    let breached = history.recent.len() >= MAX_RESTARTS_IN_WINDOW
        && history
            .recent
            .iter()
            .all(|t| now.duration_since(*t) <= RESTART_WINDOW);
    if breached {
        history.disabled = true;
    }
    breached
}

/// Walk every repository and probe its fsmonitor PID. Returns per-repo
/// outcomes so the caller can persist new / cleared PIDs.
///
/// `is_alive` is a closure so the integration test can pass a
/// deterministic mock; production calls [`gixw::is_fsmonitor_alive`].
pub async fn probe_all<F>(
    persistence: &Persistence,
    histories: &Mutex<HashMap<RepositoryId, RestartHistory>>,
    is_alive: F,
) -> Result<Vec<(RepositoryId, ProbeOutcome)>>
where
    F: Fn(u32) -> bool + Send + Sync,
{
    let repos = concerto_persist::repositories::list_all(persistence.readers()).await?;
    let mut outcomes = Vec::with_capacity(repos.len());
    for repo in repos {
        let outcome = probe_one(&repo, histories, &is_alive).await;
        outcomes.push((repo.id, outcome));
    }
    Ok(outcomes)
}

async fn probe_one<F>(
    repo: &Repository,
    histories: &Mutex<HashMap<RepositoryId, RestartHistory>>,
    is_alive: &F,
) -> ProbeOutcome
where
    F: Fn(u32) -> bool + Send + Sync,
{
    // `0` and `NULL` both mean "no daemon recorded". `Some(0)` is
    // benign — treat as missing.
    let recorded = repo.fs_monitor_pid.unwrap_or(0);
    if recorded > 0 && is_alive(recorded as u32) {
        return ProbeOutcome::Alive;
    }

    // Check the disabled flag before paying for a restart attempt.
    {
        let guard = histories.lock().await;
        if let Some(h) = guard.get(&repo.id) {
            if h.disabled {
                return ProbeOutcome::Disabled;
            }
        }
    }

    // Attempt restart. Record the attempt up-front so a failure still
    // counts toward the cap — a daemon that refuses to start is just as
    // expensive as one that crashes immediately.
    let now = Instant::now();
    let breached = {
        let mut guard = histories.lock().await;
        let history = guard.entry(repo.id.clone()).or_default();
        record_restart(history, now)
    };
    if breached {
        tracing::warn!(
            repo_id = %repo.id,
            "repo.fsmonitor_restarted_too_often: disabled after {MAX_RESTARTS_IN_WINDOW} restarts in 60s window"
        );
        // Best-effort stop so we don't leak a partly-launched daemon.
        let _ = gixw::stop_fsmonitor(Path::new(&repo.local_path)).await;
        return ProbeOutcome::Disabled;
    }

    match gixw::start_fsmonitor(Path::new(&repo.local_path)).await {
        Ok(pid) => {
            tracing::info!(
                repo_id = %repo.id,
                pid,
                "repo.fsmonitor_restarted"
            );
            ProbeOutcome::Restarted { pid }
        }
        Err(e) => {
            tracing::warn!(
                repo_id = %repo.id,
                error = %e,
                "fsmonitor restart failed; will retry next cycle"
            );
            ProbeOutcome::RestartFailed
        }
    }
}

/// Persist a probe outcome back to the `repositories.fs_monitor_pid`
/// column. Skipping `ProbeOutcome::Alive` (nothing changed) is a
/// deliberate optimisation — keeps the supervisor's per-cycle write
/// pressure proportional to actual lifecycle events.
pub async fn apply_outcome(
    persistence: &Persistence,
    id: &RepositoryId,
    outcome: &ProbeOutcome,
) -> Result<()> {
    match outcome {
        ProbeOutcome::Alive => Ok(()),
        ProbeOutcome::Restarted { pid } => {
            let mut writer = persistence.writer().await;
            concerto_persist::repositories::update_fs_monitor_pid(
                &mut writer,
                id,
                Some(*pid as i64),
            )
            .await
        }
        ProbeOutcome::Disabled | ProbeOutcome::RestartFailed => {
            let mut writer = persistence.writer().await;
            concerto_persist::repositories::update_fs_monitor_pid(&mut writer, id, None).await
        }
    }
}

/// Spawn the 30s supervisor loop. The returned `JoinHandle` is dropped
/// by the caller (the `RepoManagerActor::run` task); shutdown is signalled
/// via the supplied `CancellationToken`.
pub fn spawn_supervisor(
    persistence: Arc<Persistence>,
    histories: Arc<Mutex<HashMap<RepositoryId, RestartHistory>>>,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SUPERVISOR_INTERVAL);
        // First tick fires immediately — the second `tick().await` is
        // the one that actually paces the loop.
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let _ = ticker.tick().await;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::debug!("fsmonitor supervisor shutting down");
                    return;
                }
                _ = ticker.tick() => {
                    let outcomes = match probe_all(
                        &persistence,
                        &histories,
                        gixw::is_fsmonitor_alive,
                    ).await {
                        Ok(o) => o,
                        Err(e) => {
                            tracing::warn!(error = %e, "fsmonitor supervisor: probe_all failed");
                            continue;
                        }
                    };
                    for (id, outcome) in outcomes {
                        if let Err(e) = apply_outcome(&persistence, &id, &outcome).await {
                            tracing::warn!(error = %e, repo_id = %id, "fsmonitor supervisor: failed to persist outcome");
                        }
                    }
                }
            }
        }
    })
}

/// Best-effort post-clone bring-up of fsmonitor + maintenance + perf
/// config. Returns the daemon PID on success.
///
/// fsmonitor failure (filesystem unsupported) is downgraded to `None` +
/// an info log per `design/02 §8`: "treat as not supported on this
/// filesystem and disable gracefully".
pub async fn bring_up_after_clone(repo_dir: &Path) -> Option<u32> {
    if let Err(e) = gixw::apply_perf_config(repo_dir).await {
        tracing::warn!(
            error = %e,
            repo_dir = %repo_dir.display(),
            "apply_perf_config failed; continuing"
        );
    }
    // register_maintenance swallows its own errors (the helper is
    // documented as best-effort).
    let _ = gixw::register_maintenance(repo_dir).await;
    match gixw::start_fsmonitor(repo_dir).await {
        Ok(pid) => Some(pid),
        Err(e) => {
            tracing::info!(
                error = %e,
                repo_dir = %repo_dir.display(),
                "fsmonitor not supported on this filesystem; disabling for this repo"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_restart_under_cap_does_not_disable() {
        let mut h = RestartHistory::default();
        let t0 = Instant::now();
        assert!(!record_restart(&mut h, t0));
        assert!(!record_restart(&mut h, t0 + Duration::from_millis(10)));
        assert!(!h.disabled);
        assert_eq!(h.recent.len(), 2);
    }

    #[test]
    fn record_restart_breaches_at_three_within_window() {
        let mut h = RestartHistory::default();
        let t0 = Instant::now();
        record_restart(&mut h, t0);
        record_restart(&mut h, t0 + Duration::from_millis(10));
        // Third restart inside the window flips the cap.
        let breached = record_restart(&mut h, t0 + Duration::from_millis(20));
        assert!(breached);
        assert!(h.disabled);
    }

    #[test]
    fn record_restart_prunes_old_entries() {
        let mut h = RestartHistory::default();
        let t0 = Instant::now();
        record_restart(&mut h, t0);
        record_restart(&mut h, t0 + Duration::from_millis(10));
        // A restart well outside the window discards the two stale
        // entries — count is back to one, no breach.
        let later = t0 + RESTART_WINDOW + Duration::from_secs(5);
        let breached = record_restart(&mut h, later);
        assert!(!breached);
        assert!(!h.disabled);
        assert_eq!(h.recent.len(), 1);
    }
}
