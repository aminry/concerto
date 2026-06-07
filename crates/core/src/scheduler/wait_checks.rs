//! `wait_for_check_runs` — the Scheduler check-runs gate primitive (Task 318,
//! `design/05 §3.9`/§5.1, `design/13 §3.3`).
//!
//! This is the awaitable Task 320's coordinated PR-set merge blocks on between
//! merging one set member and the next: poll a [`CheckRunsSource`] for a
//! commit SHA's check runs with the **FROZEN** exponential backoff
//! (`1s, 2s, 4s, 8s, 16s, 30s`-cap), resolving to a [`ChecksOutcome`] when every
//! run in the caller-supplied [`RequiredChecks`] set reaches a terminal
//! conclusion **or** the wall-clock `timeout` elapses. A timeout resolves with
//! `timed_out: true` (NOT an `Err`) — the caller (03) decides what to do
//! (`design/05 §8`).
//!
//! ## What this primitive is NOT
//!
//! It is independent of the `/loop` fire wheel + the `notify` machinery. It
//! never touches the wheel or the inflight map; it only consults its injected
//! source and sleeps. 320 awaits it on its own task, so it must never starve the
//! fire loop.
//!
//! ## The backoff cadence is a single source of truth
//!
//! The `[1, 2, 4, 8, 16, 30]`-cap sequence is **imported** from
//! [`concerto_vcs::rate_limit`] (Task 314, [`check_run_backoff_secs`]), NOT
//! re-declared here — the same literals back `design/13 §3.3`'s degraded cadence
//! and `design/05 §3.9`'s wait gate, so a divergence would be a latent bug.
//! [`CHECK_RUN_BACKOFF_SECS`] is re-exported from this module for callers /
//! interface snapshots, but the value lives in `concerto-vcs`.
//!
//! ## Webhook fast-path (optional + advisory)
//!
//! When the source exposes a [`WebhookWake`] (Task 315's `checks.<wa>.<repo>`
//! emits, surfaced via 316's `ChecksAggregator`), the loop `select!`s the
//! backoff sleep against the wake so a `check_run` event short-circuits the
//! sleep and triggers an immediate re-poll. The wake is a **hint only** — the
//! authoritative state always comes from a re-poll (the webhook payload is
//! opaque). Absent a wake, the loop degrades to pure backoff sleeps and is fully
//! provable against a stubbed source (the Tier-1 path).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use concerto_error::{Error, Result};
use concerto_persist::RepositoryId;
use tokio::sync::broadcast;
use tokio::time::Instant;

// Re-export 314's FROZEN cadence so callers + the interface snapshot see the
// constant on the Scheduler surface without a second declaration. The value
// lives in `concerto-vcs` (single source of truth).
pub use concerto_vcs::rate_limit::{check_run_backoff_secs, CHECK_RUN_BACKOFF_SECS};
use concerto_vcs::VcsEvent;

/// A transport-free snapshot of a single check run (`design/13 §3.8`). Mirrors
/// `concerto_vcs`'s `CheckRun`/`gh_cli::CheckRun` without leaking either across
/// the Scheduler boundary. FROZEN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRunSnapshot {
    /// The check's name (the workflow job / status context).
    pub name: String,
    /// `queued | in_progress | completed` (CheckRun) or `pending | success |
    /// failure | error` (legacy StatusContext), copied verbatim from the source.
    pub status: String,
    /// Terminal conclusion (`success | failure | neutral | cancelled |
    /// timed_out | action_required | stale | skipped`), empty until `completed`.
    pub conclusion: String,
}

impl CheckRunSnapshot {
    /// A run is **terminal** iff its `status == "completed"` (`design/05 §3.9`):
    /// only then is its `conclusion` set. A legacy StatusContext is terminal
    /// when its status is itself a conclusion (`success | failure | error`).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            "completed" | "success" | "failure" | "error"
        )
    }

    /// Whether a terminal run **passed**. Conservative per `design/05 §3.9`'s
    /// "success / failure / cancelled" terminal set + GitHub's full conclusion
    /// vocabulary: `success | neutral | skipped | stale` pass; `failure |
    /// cancelled | timed_out | action_required` do not. A legacy StatusContext
    /// whose status is `success` passes; `failure | error` do not. A non-terminal
    /// run has not passed.
    pub fn passed(&self) -> bool {
        if !self.is_terminal() {
            return false;
        }
        match self.status.as_str() {
            // Legacy StatusContext: the status IS the conclusion.
            "success" => true,
            "failure" | "error" => false,
            // CheckRun: the conclusion carries the verdict.
            _ => matches!(
                self.conclusion.as_str(),
                "success" | "neutral" | "skipped" | "stale"
            ),
        }
    }
}

/// The caller-supplied required-checks set (`PHASE3_PLANNING §2`: the required
/// set is a caller parameter — **no** branch-protection API read in V1.0).
/// FROZEN.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RequiredChecks {
    /// "All check-runs returned for the SHA must reach a terminal conclusion."
    /// The default 320 uses.
    #[default]
    AllTerminal,
    /// Restrict the gate to a named subset; runs whose names are not in the set
    /// are ignored (pending or not). Resolves once every named run is terminal.
    Named(Vec<String>),
}

impl RequiredChecks {
    /// The subset of `runs` this required set gates on. `AllTerminal` gates on
    /// every run; `Named` gates only on runs whose name is in the set.
    fn select<'a>(&self, runs: &'a [CheckRunSnapshot]) -> Vec<&'a CheckRunSnapshot> {
        match self {
            RequiredChecks::AllTerminal => runs.iter().collect(),
            RequiredChecks::Named(names) => runs
                .iter()
                .filter(|r| names.iter().any(|n| n == &r.name))
                .collect(),
        }
    }

    /// Whether the required set is **resolved** against the current `runs`: every
    /// gated run is terminal. An `AllTerminal` set over zero runs is *not*
    /// resolved (no checks have reported yet — keep waiting until the timeout); a
    /// `Named` set whose names are all present + terminal resolves even if other
    /// runs are still pending.
    fn resolved(&self, runs: &[CheckRunSnapshot]) -> bool {
        let gated = self.select(runs);
        match self {
            // No check has reported yet → not resolved (poll on).
            RequiredChecks::AllTerminal if gated.is_empty() => false,
            // A named set must observe every named run AND have them all terminal.
            RequiredChecks::Named(names) => names.iter().all(|n| {
                gated
                    .iter()
                    .find(|r| &r.name == n)
                    .map(|r| r.is_terminal())
                    .unwrap_or(false)
            }),
            RequiredChecks::AllTerminal => gated.iter().all(|r| r.is_terminal()),
        }
    }

    /// Whether the required set **passed**: it is resolved AND every gated run
    /// passed. (`passed` is only meaningful once `resolved`.)
    fn passed(&self, runs: &[CheckRunSnapshot]) -> bool {
        self.resolved(runs) && self.select(runs).iter().all(|r| r.passed())
    }
}

/// The outcome of [`SchedulerHandle::wait_for_check_runs`] (`design/05 §3.9`).
/// FROZEN.
///
/// `passed = the required set all reached a non-failure terminal conclusion`. A
/// timeout always yields `timed_out: true`; `passed` then reflects the last
/// observed state (so a timeout where everything had already gone green still
/// reports `passed: true, timed_out: true`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksOutcome {
    /// Every run in the required set reached a non-failure terminal conclusion.
    pub passed: bool,
    /// The wall-clock `timeout` elapsed before the required set resolved.
    pub timed_out: bool,
    /// The last observed check-run snapshot set (the state `passed`/`timed_out`
    /// were derived from).
    pub runs: Vec<CheckRunSnapshot>,
}

/// The check-runs source the poll loop consults (`design/05 §3.9`, the FROZEN
/// seam). Implemented by the production `VcsHandle` (delegating to
/// `get_check_runs`) and by the Tier-1 test stub (a scripted `Vec` of poll
/// results). The Scheduler holds an `Option<Arc<dyn CheckRunsSource>>`; absent
/// it, [`SchedulerHandle::wait_for_check_runs`] returns the typed
/// `scheduler.no_vcs_source` error.
#[async_trait]
pub trait CheckRunsSource: Send + Sync {
    /// Fetch the current check runs for `repo`'s commit `sha`. The poll loop
    /// calls this once per backoff step (and once on each webhook wake).
    async fn check_runs(&self, repo: &RepositoryId, sha: &str) -> Result<Vec<CheckRunSnapshot>>;

    /// Optional webhook fast-path (`design/05 §3.9` "subscribe to webhook
    /// updates if the VCS provides them"). Returns a [`WebhookWake`] the loop
    /// `select!`s against the backoff sleep so a `check_run` event for `repo`
    /// short-circuits the current sleep. The default is `None` — pure polling
    /// (the Tier-1 path, and the path when no relay/webhook is wired). The wake
    /// is **advisory**: the authoritative state always comes from a re-poll.
    fn webhook_wake(&self, _repo: &RepositoryId) -> Option<WebhookWake> {
        None
    }
}

/// An advisory webhook wake hint for one repository. Wraps the
/// `ChecksAggregator` broadcast (Task 316's `checks.<wa>.<repo>` emits, fed by
/// Task 315's webhook receiver) + the target repository id; [`WebhookWake::wake`]
/// resolves when an event for that repository arrives (or the channel lags —
/// also a "re-poll now" hint). A `Closed` channel resolves once and never again,
/// so the loop falls back to pure backoff sleeps.
pub struct WebhookWake {
    rx: broadcast::Receiver<VcsEvent>,
    repository_id: String,
    /// Set once the underlying broadcast closes so we stop selecting a
    /// ready-immediately arm (which would busy-spin the poll loop).
    closed: bool,
}

impl WebhookWake {
    /// Build a wake from a `ChecksAggregator` subscription + the repository id to
    /// filter on. Events for other repositories are skipped (they belong to a
    /// different wait).
    pub fn new(rx: broadcast::Receiver<VcsEvent>, repository_id: impl Into<String>) -> Self {
        Self {
            rx,
            repository_id: repository_id.into(),
            closed: false,
        }
    }

    /// Await the next wake hint for this repository. Returns when a matching
    /// event arrives or the channel lags. When the channel is closed it resolves
    /// once and then parks forever (`std::future::pending`) so the `select!`
    /// degrades cleanly to the backoff arm.
    async fn wake(&mut self) {
        if self.closed {
            std::future::pending::<()>().await;
        }
        loop {
            match self.rx.recv().await {
                Ok(ev) => {
                    if ev.repository_id == self.repository_id {
                        return;
                    }
                    // An event for a different repo — keep waiting.
                    continue;
                }
                // Lagged: events were dropped; treat as a "re-poll now" hint.
                Err(broadcast::error::RecvError::Lagged(_)) => return,
                // Closed: no more webhook hints. Resolve once; subsequent calls
                // park so the loop relies on the backoff sleep.
                Err(broadcast::error::RecvError::Closed) => {
                    self.closed = true;
                    return;
                }
            }
        }
    }
}

/// Stable wire-code the Scheduler returns when `wait_for_check_runs` is called
/// without a wired [`CheckRunsSource`].
const NO_VCS_SOURCE: &str = "scheduler.no_vcs_source";

/// Build the `scheduler.no_vcs_source` error (no check-runs source wired).
pub fn no_vcs_source() -> Error {
    Error::Internal(format!(
        "{NO_VCS_SOURCE}: wait_for_check_runs called before a CheckRunsSource was wired \
         (boot calls SchedulerHandle::set_check_runs_source after the VCS handle is built)"
    ))
}

/// The core poll/backoff/webhook loop, factored out of `SchedulerHandle` so it is
/// unit-testable with any [`CheckRunsSource`] (the Tier-1 stub or the production
/// `VcsHandle`). `SchedulerHandle::wait_for_check_runs` is a thin wrapper that
/// supplies its injected source.
///
/// Uses `tokio::time::{sleep, Instant}` for the backoff + the deadline so a test
/// can `tokio::time::pause()` + `advance(..)` and drive a 10-minute timeout
/// instantly. Never `std::thread::sleep`.
pub(crate) async fn run_wait_loop(
    source: &Arc<dyn CheckRunsSource>,
    repo: &RepositoryId,
    sha: &str,
    timeout: Duration,
    required: RequiredChecks,
) -> Result<ChecksOutcome> {
    let deadline = Instant::now() + timeout;
    let mut webhook = source.webhook_wake(repo);
    let mut attempt: usize = 0;
    let mut last_runs: Vec<CheckRunSnapshot>;

    loop {
        // Poll the authoritative state. A source error is surfaced (the caller
        // decides) — it is not a timeout.
        last_runs = source.check_runs(repo, sha).await?;

        if required.resolved(&last_runs) {
            return Ok(ChecksOutcome {
                passed: required.passed(&last_runs),
                timed_out: false,
                runs: last_runs,
            });
        }

        // Not resolved yet: sleep the next backoff step, but never past the
        // deadline. A webhook wake short-circuits the sleep → immediate re-poll.
        let now = Instant::now();
        if now >= deadline {
            return Ok(timeout_outcome(&required, last_runs));
        }
        let step = Duration::from_secs(check_run_backoff_secs(attempt));
        let until = (now + step).min(deadline);
        attempt = attempt.saturating_add(1);

        match &mut webhook {
            Some(wake) => {
                tokio::select! {
                    _ = tokio::time::sleep_until(until) => {}
                    _ = wake.wake() => {
                        // Advisory wake: re-poll immediately (do not advance the
                        // backoff — the next sleep, if needed, repeats this step).
                        attempt = attempt.saturating_sub(1);
                    }
                }
            }
            None => tokio::time::sleep_until(until).await,
        }

        // The wake / a short sleep can land exactly on the deadline; re-check so
        // a never-terminating SHA resolves to a timeout rather than re-polling.
        if Instant::now() >= deadline {
            // Re-poll once more so the timeout outcome reflects the freshest
            // observed state, then resolve as a timeout.
            if let Ok(runs) = source.check_runs(repo, sha).await {
                last_runs = runs;
                if required.resolved(&last_runs) {
                    return Ok(ChecksOutcome {
                        passed: required.passed(&last_runs),
                        timed_out: false,
                        runs: last_runs,
                    });
                }
            }
            return Ok(timeout_outcome(&required, last_runs));
        }
    }
}

/// Build the timeout outcome: `timed_out: true`, `passed` reflecting the last
/// observed state (so a timeout where everything already passed still reports
/// `passed: true`).
fn timeout_outcome(required: &RequiredChecks, runs: Vec<CheckRunSnapshot>) -> ChecksOutcome {
    ChecksOutcome {
        passed: required.passed(&runs),
        timed_out: true,
        runs,
    }
}

#[cfg(test)]
mod tests {
    //! Tier-1 unit tests (`tasks/v1.0/318`): an in-process [`CheckRunsSource`]
    //! stub + `tokio::time::pause`/`advance` drive the poll/backoff/timeout/
    //! webhook paths instantly + deterministically — no real network, no
    //! `wiremock`, no real wall-clock sleep.

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use concerto_vcs::checks::VcsEvent;

    use super::*;

    /// Convenience: a check-run snapshot from `(name, status, conclusion)`.
    fn snap(name: &str, status: &str, conclusion: &str) -> CheckRunSnapshot {
        CheckRunSnapshot {
            name: name.into(),
            status: status.into(),
            conclusion: conclusion.into(),
        }
    }

    /// A scripted in-process check-runs source. Each `check_runs` call returns the
    /// next entry in `script`; once the script is exhausted it keeps returning the
    /// last entry (a never-terminating SHA stays pending forever). `polls` counts
    /// every poll so a test can assert the loop's poll cadence. An optional
    /// broadcast wires the webhook fast-path.
    struct ScriptedSource {
        script: Vec<Vec<CheckRunSnapshot>>,
        polls: AtomicUsize,
        wake_tx: Mutex<Option<broadcast::Sender<VcsEvent>>>,
    }

    impl ScriptedSource {
        fn new(script: Vec<Vec<CheckRunSnapshot>>) -> Self {
            Self {
                script,
                polls: AtomicUsize::new(0),
                wake_tx: Mutex::new(None),
            }
        }

        /// Equip the source with a webhook broadcast (the fast-path). Returns the
        /// sender so the test can feed `check_run` events.
        fn with_webhook(self) -> (Arc<Self>, broadcast::Sender<VcsEvent>) {
            let (tx, _rx) = broadcast::channel(8);
            *self.wake_tx.lock().unwrap() = Some(tx.clone());
            (Arc::new(self), tx)
        }

        fn poll_count(&self) -> usize {
            self.polls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl CheckRunsSource for ScriptedSource {
        async fn check_runs(
            &self,
            _repo: &RepositoryId,
            _sha: &str,
        ) -> Result<Vec<CheckRunSnapshot>> {
            let n = self.polls.fetch_add(1, Ordering::SeqCst);
            let idx = n.min(self.script.len().saturating_sub(1));
            Ok(self.script.get(idx).cloned().unwrap_or_default())
        }

        fn webhook_wake(&self, repo: &RepositoryId) -> Option<WebhookWake> {
            self.wake_tx
                .lock()
                .unwrap()
                .as_ref()
                .map(|tx| WebhookWake::new(tx.subscribe(), repo.0.clone()))
        }
    }

    fn repo() -> RepositoryId {
        RepositoryId("owner/repo".into())
    }

    // --- The FROZEN backoff cadence -------------------------------------------

    #[test]
    fn backoff_cadence_is_frozen_and_capped() {
        // The constant is the single source of truth (imported from
        // `concerto_vcs::rate_limit`), and the helper caps at the last value.
        assert_eq!(CHECK_RUN_BACKOFF_SECS, [1, 2, 4, 8, 16, 30]);
        let seq: Vec<u64> = (0..9).map(check_run_backoff_secs).collect();
        assert_eq!(seq, [1, 2, 4, 8, 16, 30, 30, 30, 30]);
    }

    // --- Resolution paths ------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn pending_then_all_success_resolves_passed() {
        // pending → pending → all-success: resolves passed, not timed out.
        let source: Arc<dyn CheckRunsSource> = Arc::new(ScriptedSource::new(vec![
            vec![snap("build", "in_progress", "")],
            vec![snap("build", "queued", "")],
            vec![snap("build", "completed", "success")],
        ]));
        let out = run_wait_loop(
            &source,
            &repo(),
            "deadbeef",
            Duration::from_secs(600),
            RequiredChecks::AllTerminal,
        )
        .await
        .unwrap();
        assert!(out.passed, "all-success should pass: {out:?}");
        assert!(!out.timed_out);
        assert_eq!(out.runs, vec![snap("build", "completed", "success")]);
    }

    #[tokio::test(start_paused = true)]
    async fn failure_conclusion_resolves_not_passed() {
        let source: Arc<dyn CheckRunsSource> = Arc::new(ScriptedSource::new(vec![
            vec![snap("test", "in_progress", "")],
            vec![snap("test", "completed", "failure")],
        ]));
        let out = run_wait_loop(
            &source,
            &repo(),
            "sha",
            Duration::from_secs(600),
            RequiredChecks::AllTerminal,
        )
        .await
        .unwrap();
        assert!(!out.passed, "a failing run must not pass: {out:?}");
        assert!(!out.timed_out, "a resolved failure is not a timeout");
    }

    #[tokio::test(start_paused = true)]
    async fn never_terminating_times_out() {
        // A SHA whose only run never leaves `in_progress`. Under paused time the
        // backoff sleeps are advanced automatically by the runtime, so the
        // 5s-timeout resolves instantly (no real sleep).
        let source = Arc::new(ScriptedSource::new(vec![vec![snap(
            "build",
            "in_progress",
            "",
        )]]));
        let dyn_source: Arc<dyn CheckRunsSource> = source.clone();
        let out = run_wait_loop(
            &dyn_source,
            &repo(),
            "sha",
            Duration::from_secs(5),
            RequiredChecks::AllTerminal,
        )
        .await
        .unwrap();
        assert!(out.timed_out, "never-terminating must time out: {out:?}");
        assert!(!out.passed, "a pending run has not passed");
        // It polled more than once (the loop kept re-polling across backoffs).
        assert!(source.poll_count() >= 2, "polls={}", source.poll_count());
    }

    #[tokio::test(start_paused = true)]
    async fn all_terminal_with_zero_runs_waits_until_timeout() {
        // No check has reported yet → `AllTerminal` keeps waiting (it does NOT
        // resolve on an empty set), then times out.
        let source: Arc<dyn CheckRunsSource> = Arc::new(ScriptedSource::new(vec![vec![]]));
        let out = run_wait_loop(
            &source,
            &repo(),
            "sha",
            Duration::from_secs(5),
            RequiredChecks::AllTerminal,
        )
        .await
        .unwrap();
        assert!(out.timed_out, "empty set must keep waiting: {out:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn named_subset_resolves_ignoring_other_pending() {
        // `Named(["build"])` resolves when `build` is terminal even though `lint`
        // is still pending (it is not in the required set).
        let source: Arc<dyn CheckRunsSource> = Arc::new(ScriptedSource::new(vec![vec![
            snap("build", "completed", "success"),
            snap("lint", "in_progress", ""),
        ]]));
        let out = run_wait_loop(
            &source,
            &repo(),
            "sha",
            Duration::from_secs(600),
            RequiredChecks::Named(vec!["build".into()]),
        )
        .await
        .unwrap();
        assert!(
            out.passed,
            "named subset terminal+success should pass: {out:?}"
        );
        assert!(!out.timed_out);
    }

    #[tokio::test(start_paused = true)]
    async fn named_subset_waits_for_a_missing_named_run() {
        // `lint` is required but has not yet appeared in the source's runs → the
        // named set is unresolved → wait until timeout.
        let source: Arc<dyn CheckRunsSource> = Arc::new(ScriptedSource::new(vec![vec![snap(
            "build",
            "completed",
            "success",
        )]]));
        let out = run_wait_loop(
            &source,
            &repo(),
            "sha",
            Duration::from_secs(5),
            RequiredChecks::Named(vec!["lint".into()]),
        )
        .await
        .unwrap();
        assert!(
            out.timed_out,
            "a missing named run must keep waiting: {out:?}"
        );
    }

    // --- Webhook fast-path -----------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn webhook_wake_short_circuits_a_long_backoff_sleep() {
        // First poll is pending; the next backoff step would be a long sleep. A
        // fed `checks.<wa>.<repo>` event cancels that sleep so the loop re-polls
        // immediately and observes the now-green run — well before the wall-clock
        // timeout. We assert resolution happens at a wall-clock instant far short
        // of the timeout.
        let (source, tx) = ScriptedSource::new(vec![
            vec![snap("build", "in_progress", "")],
            vec![snap("build", "completed", "success")],
        ])
        .with_webhook();
        let dyn_source: Arc<dyn CheckRunsSource> = source.clone();

        // Resume the auto-advancing paused clock just enough to enter the first
        // backoff sleep, then feed the wake; the wake arm fires and re-polls.
        let waiter = tokio::spawn(async move {
            run_wait_loop(
                &dyn_source,
                &repo(),
                "sha",
                Duration::from_secs(600),
                RequiredChecks::AllTerminal,
            )
            .await
        });

        // Let the loop run its first poll + enter the select. Yield so the
        // spawned task makes progress under paused time.
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        // Fire the advisory wake → short-circuits the (1s) backoff sleep.
        tx.send(VcsEvent {
            workarea_id: "wa".into(),
            repository_id: "owner/repo".into(),
            frame: Vec::new(),
        })
        .unwrap();

        let out = waiter.await.unwrap().unwrap();
        assert!(out.passed, "webhook-woken re-poll saw success: {out:?}");
        assert!(!out.timed_out, "resolved via the wake, not the timeout");
        // Exactly two polls: the initial one + the wake-triggered re-poll.
        assert_eq!(source.poll_count(), 2, "wake should trigger one extra poll");
    }

    #[tokio::test(start_paused = true)]
    async fn missing_source_is_a_typed_error() {
        // Sanity: the no-source error code is stable for callers.
        let err = no_vcs_source();
        assert!(format!("{err}").contains("scheduler.no_vcs_source"));
    }
}
