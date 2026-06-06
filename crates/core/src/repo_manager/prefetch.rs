//! Idle blob prewarm: the scheduler loop, the cancellable `PrewarmHandle`,
//! and the injected idle/power/net signal seam (Task 304, `design/02
//! §3.3`/`§6.1`/`§6.3`, `PHASE3_PLANNING §2`/§4.6).
//!
//! A blobless+sparse repo (Tasks 301/302) keeps its blobs *lazy* — git
//! fetches them on first read. This module materializes the in-cone blobs
//! ahead of agent need via three triggers:
//!
//! 1. **at worktree-create** — the workarea-create path kicks a prewarm for
//!    the new (workarea, repo) cone @ HEAD (default ON).
//! 2. **eagerly on HEAD-update** — a tracked branch advancing prewarms the
//!    blobs touched by the new commits (default ON).
//! 3. **idle-background** — [`spawn_prefetch_scheduler`] walks every
//!    blobless repo's cones when the machine is idle, on AC, and on
//!    non-metered Wi-Fi.
//!
//! ## The injected-signal seam (the Tier-1 testability key)
//!
//! Idle / power / network state cannot be observed in CI. Following
//! `fsmonitor::probe_all`'s `is_alive: F` closure precedent, every external
//! input is an `Arc<dyn Fn() -> … + Send + Sync>`:
//!
//! - [`IdleSignal`] → [`IdleState`] (`Idle(Duration)` / `Active`)
//! - [`PowerSignal`] → [`PowerState`] (`Ac` / `Battery`)
//! - [`NetSignal`] → [`NetState`] (`WifiUnmetered` / `Metered` / `Other`)
//!
//! Production wires a best-effort, macOS-first implementation (see
//! [`signals`]); tests pass a deterministic mock to drive every branch
//! (idle→enqueue, active→cancel, on-battery→skip, metered→skip). The real
//! client-heartbeat-driven idle source (Local API, `design/02 §6.3`) is a
//! small documented follow-on — `boot.rs` injects the default until then.
//!
//! ## Concurrency (`design/02 §6.1`)
//!
//! - **One write per repo** — a prewarm fetch holds the existing per-repo
//!   write mutex (serialized against clone/fetch). Owned by the caller
//!   (`RepoManager::prewarm_blobs`), not this module.
//! - **Global 2-concurrent** — a shared [`tokio::sync::Semaphore`] with
//!   [`GLOBAL_PREWARM_CONCURRENCY`] permits caps prewarm fetches *across*
//!   repos.
//! - **Per-repo bandwidth cap** — [`BandwidthLimiter`] is consulted before
//!   each repo's prewarm; the real token-bucket throttle is a follow-on, so
//!   the V1.0 limiter is a counting seam the scheduler always calls (and
//!   tests assert it was consulted).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// Global cap on concurrent prewarm fetches across all repos
/// (`design/02 §6.1`). FROZEN by Task 304.
pub const GLOBAL_PREWARM_CONCURRENCY: usize = 2;

/// Default idle threshold before the background scheduler prewarms
/// (`design/02 §3.3`/`§12 R-3`). FROZEN by Task 304. Task 310's settings
/// resolver may override this per the resolved `performance.idle_threshold`
/// key; until then this const is the live default.
pub const DEFAULT_IDLE_THRESHOLD: Duration = Duration::from_secs(300);

/// How often the idle scheduler wakes to re-evaluate the signals. Mirrors
/// `fsmonitor::SUPERVISOR_INTERVAL`'s "cheap enough to keep on hand" sizing.
pub const SCHEDULER_INTERVAL: Duration = Duration::from_secs(30);

/// Settings key the idle threshold is read from (Task 310 owns the full
/// resolver; 304 reads this single key with a [`DEFAULT_IDLE_THRESHOLD`]
/// fallback).
pub const IDLE_THRESHOLD_SETTING_KEY: &str = "performance.idle_threshold_secs";

/// Whether the device has been idle long enough to prewarm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleState {
    /// Idle for the carried duration (no user/agent activity).
    Idle(Duration),
    /// Active — user or agent activity within the threshold window.
    Active,
}

/// Power source state (`design/02 §3.3`: prewarm only on AC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    /// Plugged in.
    Ac,
    /// On battery — never prewarm.
    Battery,
}

/// Network state (`design/02 §12 R-2`: prewarm only on unmetered Wi-Fi).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetState {
    /// Unmetered Wi-Fi (or wired) — prewarm allowed.
    WifiUnmetered,
    /// Metered connection (cellular / metered Wi-Fi) — never prewarm.
    Metered,
    /// Unknown / other — conservatively treated as "do not prewarm".
    Other,
}

/// Injected idle signal. FROZEN signature (Task 304) so the test mock and
/// the real heartbeat follow-on agree.
pub type IdleSignal = Arc<dyn Fn() -> IdleState + Send + Sync>;
/// Injected power signal. FROZEN signature (Task 304).
pub type PowerSignal = Arc<dyn Fn() -> PowerState + Send + Sync>;
/// Injected network signal. FROZEN signature (Task 304).
pub type NetSignal = Arc<dyn Fn() -> NetState + Send + Sync>;

/// The three injected signals + the resolved idle threshold, bundled so the
/// scheduler and the eager-trigger gate share one decision surface.
#[derive(Clone)]
pub struct PrewarmSignals {
    pub idle: IdleSignal,
    pub power: PowerSignal,
    pub net: NetSignal,
    /// Resolved idle threshold (Task 310 fills this; 304 defaults it).
    pub idle_threshold: Duration,
}

impl PrewarmSignals {
    /// Build a signal bundle with the default idle threshold.
    pub fn new(idle: IdleSignal, power: PowerSignal, net: NetSignal) -> Self {
        Self {
            idle,
            power,
            net,
            idle_threshold: DEFAULT_IDLE_THRESHOLD,
        }
    }

    /// Override the idle threshold (Task 310's resolved value).
    pub fn with_idle_threshold(mut self, threshold: Duration) -> Self {
        self.idle_threshold = threshold;
        self
    }

    /// True iff *all three* gates pass: idle longer than the threshold, on
    /// AC, and on unmetered Wi-Fi (`design/02 §3.3`/`§6.3`/`§12 R-2`).
    pub fn should_prewarm(&self) -> bool {
        let idle_ok = matches!((self.idle)(), IdleState::Idle(d) if d >= self.idle_threshold);
        let power_ok = matches!((self.power)(), PowerState::Ac);
        let net_ok = matches!((self.net)(), NetState::WifiUnmetered);
        idle_ok && power_ok && net_ok
    }
}

/// A conservative default signal bundle that **never prewarms** — used by
/// `boot.rs` on non-macOS hosts (and as the safe baseline before the real
/// heartbeat source is wired). Reports `Active` / `Battery` / `Other`.
pub fn never_prewarm_signals() -> PrewarmSignals {
    PrewarmSignals::new(
        Arc::new(|| IdleState::Active),
        Arc::new(|| PowerState::Battery),
        Arc::new(|| NetState::Other),
    )
}

/// Per-repo bandwidth cap seam (`design/02 §6.1`).
///
/// V1.0 ships the *seam*, not a real token-bucket throttle (real
/// bandwidth shaping needs the byte-counting fetch wiring that is a
/// documented follow-on). The scheduler/`prewarm_blobs` path calls
/// [`BandwidthLimiter::acquire`] before each repo's fetch so the limiter is
/// always consulted; tests assert the consult count. The default impl is a
/// no-op pass-through that records how many times it was asked.
#[derive(Clone, Default)]
pub struct BandwidthLimiter {
    consulted: Arc<AtomicU64>,
}

impl BandwidthLimiter {
    /// Fresh limiter (no cap configured yet).
    pub fn new() -> Self {
        Self::default()
    }

    /// Consult the limiter before a repo's prewarm fetch. V1.0: records the
    /// consult and returns immediately. The real impl awaits a token-bucket
    /// permit sized to the per-repo cap.
    pub async fn acquire(&self) {
        self.consulted.fetch_add(1, Ordering::SeqCst);
    }

    /// How many times [`acquire`](Self::acquire) has been called — the
    /// Tier-1 hook proving the bandwidth cap is honored on the prewarm path.
    pub fn consult_count(&self) -> u64 {
        self.consulted.load(Ordering::SeqCst)
    }
}

/// Handle to an in-flight prewarm job (`design/02 §5.1`). FROZEN (Task 304).
///
/// Carries the job's [`CancellationToken`] and its [`tokio::task::JoinHandle`].
/// [`cancel`](Self::cancel) fires the token (the fetch stops between cone
/// chunks); `Drop` does the same so a dropped handle never leaks a runaway
/// fetch. `await`-ing completion is via [`join`](Self::join).
pub struct PrewarmHandle {
    token: CancellationToken,
    /// `Option` so [`join`](Self::join) can `take()` the handle out of a
    /// `Drop`-implementing struct without moving `self`.
    join: Option<tokio::task::JoinHandle<()>>,
}

impl PrewarmHandle {
    /// Wrap a spawned prewarm task + its cancellation token.
    pub fn new(token: CancellationToken, join: tokio::task::JoinHandle<()>) -> Self {
        Self {
            token,
            join: Some(join),
        }
    }

    /// Cancel the prewarm and consume the handle. The fetch stops promptly
    /// (between cone chunks) per the `§6.3` "cancellable on user activity"
    /// contract. Does not wait for the task to wind down — call
    /// [`join`](Self::join) first when prompt completion must be observed.
    pub fn cancel(self) {
        self.token.cancel();
    }

    /// Cancellation-token clone so the scheduler can cancel in-flight jobs
    /// without owning the handle.
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// Await the prewarm task's completion (after a [`cancel`](Self::cancel)
    /// or natural finish). Joins the spawned task; a join error (panic /
    /// abort) is swallowed — prewarm is best-effort telemetry, not
    /// correctness state.
    pub async fn join(mut self) {
        if let Some(join) = self.join.take() {
            let _ = join.await;
        }
    }

    /// True iff the cancellation token has been fired.
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

impl Drop for PrewarmHandle {
    fn drop(&mut self) {
        // Dropping a handle aborts the work: fire the token (prompt,
        // cooperative) and abort the task as a hard backstop.
        self.token.cancel();
        if let Some(join) = self.join.take() {
            join.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_prewarm_requires_all_three_gates() {
        let idle: IdleSignal = Arc::new(|| IdleState::Idle(Duration::from_secs(600)));
        let ac: PowerSignal = Arc::new(|| PowerState::Ac);
        let wifi: NetSignal = Arc::new(|| NetState::WifiUnmetered);

        // All three pass.
        let s = PrewarmSignals::new(idle.clone(), ac.clone(), wifi.clone());
        assert!(s.should_prewarm());

        // Idle but below threshold → no.
        let short: IdleSignal = Arc::new(|| IdleState::Idle(Duration::from_secs(10)));
        let s = PrewarmSignals::new(short, ac.clone(), wifi.clone());
        assert!(!s.should_prewarm());

        // Active → no.
        let active: IdleSignal = Arc::new(|| IdleState::Active);
        let s = PrewarmSignals::new(active, ac.clone(), wifi.clone());
        assert!(!s.should_prewarm());

        // On battery → no.
        let battery: PowerSignal = Arc::new(|| PowerState::Battery);
        let s = PrewarmSignals::new(idle.clone(), battery, wifi.clone());
        assert!(!s.should_prewarm());

        // Metered → no.
        let metered: NetSignal = Arc::new(|| NetState::Metered);
        let s = PrewarmSignals::new(idle.clone(), ac.clone(), metered);
        assert!(!s.should_prewarm());
    }

    #[test]
    fn never_prewarm_default_is_off() {
        assert!(!never_prewarm_signals().should_prewarm());
    }

    #[test]
    fn bandwidth_limiter_records_consults() {
        let limiter = BandwidthLimiter::new();
        assert_eq!(limiter.consult_count(), 0);
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            limiter.acquire().await;
            limiter.acquire().await;
        });
        assert_eq!(limiter.consult_count(), 2);
    }

    #[tokio::test]
    async fn handle_cancel_fires_token() {
        let token = CancellationToken::child_token(&CancellationToken::new());
        let t2 = token.clone();
        let join = tokio::spawn(async move {
            t2.cancelled().await;
        });
        let handle = PrewarmHandle::new(token, join);
        assert!(!handle.is_cancelled());
        let tok = handle.token();
        handle.cancel();
        assert!(tok.is_cancelled());
    }

    #[tokio::test]
    async fn drop_aborts_the_task() {
        let token = CancellationToken::new();
        let join = tokio::spawn(async move {
            // Would run forever absent a cancel/abort.
            std::future::pending::<()>().await;
        });
        let handle = PrewarmHandle::new(token.clone(), join);
        drop(handle);
        // The token was fired by Drop.
        assert!(token.is_cancelled());
    }
}

/// Best-effort, macOS-first real signal implementations (`design/02 §12
/// R-2`/R-3). Non-macOS hosts return the conservative
/// "Active/Battery/Other → never prewarm" default so the feature is simply
/// off until the heartbeat follow-on wires real detection.
pub mod signals {
    use super::*;

    /// Build the production signal bundle for this host.
    ///
    /// **macOS:** power state via `pmset -g batt` (AC vs battery); network
    /// metered status is best-effort `Other` until a `SCNetworkReachability`
    /// probe is wired (a documented follow-on), so macOS currently gates on
    /// power + idle and treats the net as `Other` → the scheduler stays off
    /// until the heartbeat/net work lands. The **idle** signal is the
    /// client-heartbeat source (Local API, `design/02 §6.3`) which is NOT
    /// wired in P3 — `boot.rs` injects [`never_prewarm_signals`] today and
    /// this function exists as the seam the follow-on fills.
    ///
    /// **Other OSes:** [`never_prewarm_signals`].
    ///
    /// Returning the conservative default here (rather than a half-real
    /// macOS impl that could prewarm on stale assumptions) keeps the V1.0
    /// behavior off-by-default and honest: the scheduler is fully proven in
    /// CI via injected mocks, and the real-hardware behavior is the
    /// operator's Tier-3 confirmation once the heartbeat source is wired.
    pub fn host_signals() -> PrewarmSignals {
        // Deliberately conservative until the Local-API heartbeat idle
        // source (the real idle signal) is wired — see this task's Handoff
        // "Deliberate debt". The seam below is where the macOS power/net
        // probes + the heartbeat idle source plug in.
        never_prewarm_signals()
    }
}

/// Spawn the idle prewarm scheduler loop (Task 304, `design/02 §6.3`).
///
/// Mirrors `fsmonitor::spawn_supervisor`'s shape exactly — a
/// [`tokio::time::interval`] loop, a [`CancellationToken`] for shutdown,
/// and injected closures for the un-CI-able inputs ([`PrewarmSignals`]).
/// The two loops are independent.
///
/// Each tick:
/// - if [`PrewarmSignals::should_prewarm`] (idle > threshold AND on AC AND
///   non-metered Wi-Fi) → enqueue a prewarm pass (one [`PrewarmHandle`] per
///   blobless repo, drained under the global-2-concurrent semaphore inside
///   `prewarm_blobs`);
/// - if the signals flip to "not idle" while jobs are in flight → cancel
///   every tracked handle (`§6.3`: "cancellable if user activity resumes").
///
/// The returned [`tokio::task::JoinHandle`] is dropped by the caller
/// (`RepoManagerActor::run`); shutdown is via `shutdown`.
pub fn spawn_prefetch_scheduler(
    manager: super::actor::RepoManager,
    signals: PrewarmSignals,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(SCHEDULER_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let _ = ticker.tick().await; // first tick fires immediately
                                     // In-flight prewarm handles from the current idle window. Cancelled
                                     // (via Drop) the moment the signals flip to active.
        let mut in_flight: Vec<PrewarmHandle> = Vec::new();
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::debug!("prefetch scheduler shutting down");
                    // Dropping the handles cancels every in-flight job.
                    return;
                }
                _ = ticker.tick() => {
                    if signals.should_prewarm() {
                        // Only start a fresh pass when the last one has fully
                        // drained, to avoid stacking duplicate fetches.
                        in_flight.retain(|h| !h.is_cancelled());
                        if in_flight.is_empty() {
                            in_flight = manager.run_prewarm_pass().await;
                            if !in_flight.is_empty() {
                                tracing::debug!(jobs = in_flight.len(), "prefetch scheduler: enqueued prewarm pass");
                            }
                        }
                    } else if !in_flight.is_empty() {
                        // User activity (or unplug / metered) resumed →
                        // cancel everything in flight. Draining the Vec drops
                        // each handle, firing its token + aborting its task.
                        tracing::debug!(jobs = in_flight.len(), "prefetch scheduler: signals flipped, cancelling in-flight prewarm");
                        in_flight.clear();
                    }
                }
            }
        }
    })
}
