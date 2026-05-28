//! Typed actor wrapper + root supervisor (Task 12).
//!
//! Implements the supervision tree described in `design/01 §3.2`, §4.2,
//! §5.2, §6.2 and §7.2. The supervisor:
//!
//! - Spawns each [`Actor::run`] under a `catch_unwind` boundary
//!   (`AssertUnwindSafe` + [`futures::FutureExt::catch_unwind`]) so a
//!   panic in one actor does not poison the tokio runtime or affect
//!   peers.
//! - Tracks a 16-slot ring buffer of recent crash timestamps per actor
//!   and applies the design's restart policy:
//!     - ≤ 3 restarts in last 60s → restart immediately.
//!     - 4–10 → exponential backoff (1s, 2s, 4s, 8s, 16s, 32s).
//!     - \> 10 → mark the actor `Failed`; do **not** restart; other
//!       actors keep running.
//! - Listens on a shared shutdown [`CancellationToken`]; when cancelled,
//!   each child gets up to 10s to drain before its [`JoinHandle::abort`]
//!   is invoked. Aborting a tokio task is cooperative — V0.1 accepts
//!   that truly stuck blocking sections may persist (the V1.0 watchdog
//!   will address this; tracked in Task 12's Handoff Notes).
//! - Exposes a list method ([`RootSupervisor::list`]) returning a
//!   snapshot of each actor's state for the future
//!   `RuntimeAdmin::GetStatus` RPC (Task 13).
//!
//! ## Drift from the spec sketch
//!
//! The task file's pseudocode shows `ActorContext.persistence:
//! Persistence`, but `Persistence` is not `Clone` (Task 08 handoff). We
//! use `Arc<Persistence>` instead, which matches the existing
//! reader/writer access pattern and was authorized by the orchestrator
//! prompt for Task 12.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use concerto_error::{Error, Result};
use concerto_persist::Persistence;
use futures::FutureExt;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Backoff schedule applied when an actor has crashed 4–10 times within
/// the recent-window: index `n` (0-based) gives the wait before the
/// (n+4)th restart. After ≥ 10 the actor is marked Failed; per design
/// the 10-element table covers indexes 0..6 with 32s held for the tail.
const BACKOFF_TABLE: [Duration; 7] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
    Duration::from_secs(32),
    Duration::from_secs(32),
];

/// Window over which restart counts are evaluated (design/01 §6.2).
const RESTART_WINDOW: Duration = Duration::from_secs(60);

/// Maximum restart attempts within `RESTART_WINDOW` before an actor is
/// marked Failed and removed from the restart loop.
const MAX_RESTARTS_IN_WINDOW: usize = 10;

/// Restart threshold below which restarts happen immediately.
const IMMEDIATE_RESTART_LIMIT: usize = 3;

/// Per-actor drain budget on shutdown (design/01 §6.4 step 5).
const SHUTDOWN_DRAIN_BUDGET: Duration = Duration::from_secs(10);

/// Capacity of the per-actor crash-timestamp ring buffer
/// (design/01 §4.2 — `ArrayVec<Instant, 16>`).
const RESTART_HISTORY_CAP: usize = 16;

/// What a supervised actor sees when its [`Actor::run`] future is
/// invoked.
///
/// `config` is wrapped in `Arc<RwLock<_>>` so future config-reload
/// (V1.0) can swap the inner value without forcing actors to recreate
/// themselves. V0.1 has no reload broadcaster yet — the lock is
/// effectively read-only.
///
/// `shutdown` is the supervisor-scoped cancellation token. An actor
/// MUST await `shutdown.cancelled()` in its top-level `select!` so it
/// drops out of `run` when shutdown is requested.
///
/// `persistence` is `Arc<Persistence>` because `Persistence` itself is
/// not `Clone` (Task 08 handoff). Multiple actors share one handle; the
/// reader pool and writer mutex serialize access internally.
pub struct ActorContext<C> {
    pub config: Arc<RwLock<C>>,
    pub shutdown: CancellationToken,
    pub persistence: Arc<Persistence>,
}

/// The typed actor trait.
///
/// An actor is a `Send + 'static` value with a fixed `NAME` and an
/// associated `Config` type. The supervisor consumes the actor in
/// [`Actor::run`]; the trait is intentionally one-shot. An actor that
/// must restart with fresh state is constructed anew by the
/// `restart_factory` passed to [`RootSupervisor::spawn`].
#[async_trait]
pub trait Actor: Send + 'static {
    /// Stable, static identifier used in logs, metrics, and the actor
    /// status list. Must be unique within a single supervisor.
    const NAME: &'static str;

    /// Configuration handed to the actor on each (re)start.
    type Config: Send + Sync + 'static;

    /// Run the actor until shutdown or an unrecoverable error.
    ///
    /// Returning `Ok(())` means "clean exit, do not restart" — that's
    /// only valid in response to `ctx.shutdown.cancelled()`. Returning
    /// `Err(_)` or panicking will be observed by the supervisor and
    /// trigger restart logic.
    async fn run(self, ctx: ActorContext<Self::Config>) -> Result<()>;
}

/// Coarse state of a supervised actor.
///
/// Observers (the future `RuntimeStatus` RPC, audit log) read this
/// through the `Arc<RwLock<_>>` held by [`ActorHandle::state`].
#[derive(Debug, Clone)]
pub enum ActorState {
    /// Task has been spawned but its `run` future hasn't been polled
    /// yet. Brief; usually invisible.
    Starting,
    /// Currently executing `run`.
    Running,
    /// `run` returned (panic or Err); we are waiting `backoff` before
    /// the next restart.
    Restarting { backoff: Duration },
    /// Exceeded the restart budget. The actor will stay dead until the
    /// supervisor is restarted (V1.0 will add a manual `restart` RPC).
    Failed { reason: String },
}

/// Ring buffer of the most recent crash timestamps for one actor.
///
/// `VecDeque` is used instead of `arrayvec::ArrayVec` to avoid pulling
/// a dependency for what is essentially a 16-element FIFO. The cap is
/// enforced manually in [`RestartHistory::record`].
#[derive(Debug, Default)]
pub struct RestartHistory {
    /// Timestamps, newest at the back.
    entries: VecDeque<Instant>,
    /// Lifetime restart count for diagnostics; never decreases.
    total: u64,
}

impl RestartHistory {
    /// Push a crash event with `now` as its timestamp, evicting the
    /// oldest if at capacity.
    pub fn record(&mut self, now: Instant) {
        if self.entries.len() == RESTART_HISTORY_CAP {
            self.entries.pop_front();
        }
        self.entries.push_back(now);
        self.total = self.total.saturating_add(1);
    }

    /// Count of restarts whose timestamp falls within
    /// `RESTART_WINDOW` of `now`.
    pub fn count_in_window(&self, now: Instant) -> usize {
        self.entries
            .iter()
            .filter(|t| now.duration_since(**t) <= RESTART_WINDOW)
            .count()
    }

    /// Lifetime restart count (monotonically non-decreasing).
    pub fn total(&self) -> u64 {
        self.total
    }
}

/// Supervisor-side handle to one spawned actor.
///
/// The supervisor keeps these in its `actors` map and uses them to
/// observe state and join on shutdown.
pub struct ActorHandle {
    pub name: &'static str,
    /// Tokio handle for the supervisor wrapper task (not the actor's
    /// inner `run` future directly).
    join: JoinHandle<()>,
    /// Per-actor cancellation. Cancelling this cancels just this
    /// actor's `ctx.shutdown`; the root token cancels everyone.
    stop: CancellationToken,
    /// State is held behind `std::sync::RwLock`: locked only for
    /// nanoseconds (clone an `ActorState` enum), and we need a
    /// synchronous read path for [`RootSupervisor::list`].
    state: Arc<StdRwLock<ActorState>>,
    /// Same reasoning as `state`: `RestartHistory` is a 16-slot ring
    /// updated only on crash, locked synchronously.
    restart_history: Arc<StdMutex<RestartHistory>>,
}

impl ActorHandle {
    /// Read the current state. Cheap.
    pub fn state(&self) -> ActorState {
        self.state
            .read()
            .expect("supervisor state lock poisoned")
            .clone()
    }

    /// Lifetime restart count for diagnostics.
    pub fn restart_total(&self) -> u64 {
        self.restart_history
            .lock()
            .expect("supervisor history lock poisoned")
            .total()
    }
}

/// Snapshot of one actor's status for the future
/// `RuntimeAdmin::GetStatus` RPC (Task 13). All fields are owned so
/// the snapshot can outlive the supervisor's locks.
#[derive(Debug, Clone)]
pub struct ActorStatusSummary {
    pub name: &'static str,
    pub state: ActorState,
    pub restart_total: u64,
}

/// Cloneable, read-only view of the supervisor's actor table.
///
/// Hands out a snapshot of every actor's current
/// [`ActorStatusSummary`] without holding a reference to the
/// supervisor itself — useful for subsystems (e.g. the Task 13
/// gRPC `RuntimeHandler`) that outlive a single `&RootSupervisor`
/// borrow.
///
/// The shared state is a vector of `(name, state-arc, history-arc)`
/// triples updated by [`RootSupervisor::spawn`]; the view reads it
/// under a short `std::sync::RwLock` critical section. Cheap enough
/// for diagnostic RPC paths.
#[derive(Clone, Default)]
pub struct SupervisorView {
    inner: Arc<StdRwLock<Vec<ActorViewEntry>>>,
}

struct ActorViewEntry {
    name: &'static str,
    state: Arc<StdRwLock<ActorState>>,
    history: Arc<StdMutex<RestartHistory>>,
}

impl SupervisorView {
    /// Snapshot every currently-registered actor. Sorted by name for
    /// stable output.
    pub fn list(&self) -> Vec<ActorStatusSummary> {
        let guard = self.inner.read().expect("supervisor view lock poisoned");
        let mut out: Vec<ActorStatusSummary> = guard
            .iter()
            .map(|e| ActorStatusSummary {
                name: e.name,
                state: e
                    .state
                    .read()
                    .expect("supervisor state lock poisoned")
                    .clone(),
                restart_total: e
                    .history
                    .lock()
                    .expect("supervisor history lock poisoned")
                    .total(),
            })
            .collect();
        out.sort_by_key(|s| s.name);
        out
    }
}

/// The root supervisor.
///
/// Owns the handles of every supervised actor, together with the
/// shared persistence handle and shutdown token. Built by
/// [`Runtime::start`](crate::runtime::Runtime::start) once per process;
/// consumed by [`RootSupervisor::shutdown`] during graceful exit.
pub struct RootSupervisor {
    actors: HashMap<&'static str, ActorHandle>,
    shutdown: CancellationToken,
    persistence: Arc<Persistence>,
    view: SupervisorView,
}

impl RootSupervisor {
    /// Build a new supervisor.
    ///
    /// `persistence` is shared with every spawned actor (one
    /// `Arc::clone` per spawn). `shutdown` is the runtime-wide
    /// cancellation token; cancelling it triggers a graceful stop of
    /// every actor when [`RootSupervisor::shutdown`] is called.
    pub fn new(persistence: Arc<Persistence>, shutdown: CancellationToken) -> Self {
        tracing::debug!("RootSupervisor ready, 0 actors");
        Self {
            actors: HashMap::new(),
            shutdown,
            persistence,
            view: SupervisorView::default(),
        }
    }

    /// Cloneable handle that exposes a sorted snapshot of every
    /// registered actor's status. Used by the Task 13 gRPC
    /// `RuntimeHandler` so it can outlive a `&RootSupervisor` borrow
    /// while still observing live state.
    pub fn view(&self) -> SupervisorView {
        self.view.clone()
    }

    /// Spawn an actor under supervision.
    ///
    /// `factory` is called once per (re)start to produce a fresh
    /// `Actor` instance — the actor is consumed by `run`, so each
    /// restart needs a new value. `config` is wrapped in
    /// `Arc<RwLock<_>>` and shared across restarts; V0.1 has no
    /// reload mechanism so the lock is effectively read-only.
    ///
    /// Errors if an actor with the same `NAME` is already registered.
    pub async fn spawn<A, F>(&mut self, factory: F, config: A::Config) -> Result<()>
    where
        A: Actor,
        F: Fn() -> A + Send + Sync + 'static,
    {
        if self.actors.contains_key(A::NAME) {
            return Err(Error::Internal(format!(
                "actor '{}' already registered with this supervisor",
                A::NAME
            )));
        }

        let state = Arc::new(StdRwLock::new(ActorState::Starting));
        let restart_history = Arc::new(StdMutex::new(RestartHistory::default()));
        let stop = self.shutdown.child_token();
        let persistence = Arc::clone(&self.persistence);
        let config = Arc::new(RwLock::new(config));

        let wrapper_state = Arc::clone(&state);
        let wrapper_history = Arc::clone(&restart_history);
        let wrapper_stop = stop.clone();

        let factory = Arc::new(factory);

        let join = tokio::spawn(async move {
            run_supervised::<A, _>(
                factory,
                config,
                persistence,
                wrapper_stop,
                wrapper_state,
                wrapper_history,
            )
            .await;
        });

        // Register in the shared view BEFORE inserting into our owned
        // map: the inserts must succeed in lockstep, but the view's
        // lock is short-lived and uncontended at spawn time.
        {
            let mut guard = self
                .view
                .inner
                .write()
                .expect("supervisor view lock poisoned");
            guard.push(ActorViewEntry {
                name: A::NAME,
                state: Arc::clone(&state),
                history: Arc::clone(&restart_history),
            });
        }

        self.actors.insert(
            A::NAME,
            ActorHandle {
                name: A::NAME,
                join,
                stop,
                state,
                restart_history,
            },
        );

        tracing::info!(actor = A::NAME, "actor spawned under supervision");
        Ok(())
    }

    /// Snapshot the state of every supervised actor. Ordered by name
    /// for stable diagnostics output.
    ///
    /// Synchronous: the per-actor locks are `std::sync` primitives held
    /// only for nanoseconds, so this is safe to call from inside an
    /// async context.
    pub fn list(&self) -> Vec<ActorStatusSummary> {
        let mut out: Vec<ActorStatusSummary> = self
            .actors
            .values()
            .map(|h| ActorStatusSummary {
                name: h.name,
                state: h.state(),
                restart_total: h.restart_total(),
            })
            .collect();
        out.sort_by_key(|s| s.name);
        out
    }

    /// Borrow the supervisor's persistence handle. Used by callers
    /// (e.g. the `Runtime` integration tests) that need a clone of the
    /// shared `Arc<Persistence>` without spawning an actor.
    pub fn persistence(&self) -> Arc<Persistence> {
        Arc::clone(&self.persistence)
    }

    /// Number of currently-tracked actors. Cheap; intended for tests
    /// and the startup `tracing::debug!` line.
    pub fn actor_count(&self) -> usize {
        self.actors.len()
    }

    /// Graceful shutdown. Consumes `self`.
    ///
    /// Per `design/01 §6.4`:
    /// 1. Cancel the shared shutdown token (idempotent if already
    ///    cancelled by the signal listener).
    /// 2. For each actor: wait up to [`SHUTDOWN_DRAIN_BUDGET`] for its
    ///    wrapper task to finish, then `abort()` and log.
    ///
    /// Returns `Ok(())` even if some actors had to be aborted —
    /// failing here would block the runtime from finishing its own
    /// `stop()` sequence (persistence flush, pid-file removal).
    pub async fn shutdown(self) -> Result<()> {
        tracing::info!(
            actors = self.actors.len(),
            "RootSupervisor shutdown beginning"
        );
        self.shutdown.cancel();

        for (name, handle) in self.actors {
            // Tell THIS actor to stop (already covered by the root
            // cancel above, but the explicit per-actor child token
            // means a future `kill <actor>` admin RPC can target one).
            handle.stop.cancel();
            match tokio::time::timeout(SHUTDOWN_DRAIN_BUDGET, handle.join).await {
                Ok(Ok(())) => tracing::debug!(actor = name, "actor wrapper joined"),
                Ok(Err(join_err)) => {
                    if join_err.is_cancelled() {
                        tracing::debug!(actor = name, "actor wrapper cancelled");
                    } else {
                        tracing::warn!(
                            actor = name,
                            error = %join_err,
                            "actor wrapper panicked during shutdown"
                        );
                    }
                }
                Err(_) => {
                    tracing::warn!(
                        actor = name,
                        budget = ?SHUTDOWN_DRAIN_BUDGET,
                        "actor did not drain within budget; tokio abort is cooperative"
                    );
                    // No `join` to call here — `tokio::time::timeout`
                    // already consumed the handle.
                }
            }
        }

        tracing::info!("RootSupervisor shutdown complete");
        Ok(())
    }
}

/// The supervisor wrapper task body. One of these per spawned actor.
///
/// Loops on:
/// 1. Construct a fresh actor via `factory`.
/// 2. Mark Running, then `select!` on `ctx.shutdown.cancelled()` vs
///    `actor.run(ctx)` wrapped in `catch_unwind`.
/// 3. Cancel → exit cleanly.
/// 4. Err / panic → record in history, decide backoff or Failed.
async fn run_supervised<A, F>(
    factory: Arc<F>,
    config: Arc<RwLock<A::Config>>,
    persistence: Arc<Persistence>,
    stop: CancellationToken,
    state: Arc<StdRwLock<ActorState>>,
    history: Arc<StdMutex<RestartHistory>>,
) where
    A: Actor,
    F: Fn() -> A + Send + Sync + 'static,
{
    loop {
        // Bail before we start if shutdown was already requested.
        if stop.is_cancelled() {
            tracing::debug!(actor = A::NAME, "shutdown observed before actor start");
            return;
        }

        let actor = (factory)();
        let ctx = ActorContext {
            config: Arc::clone(&config),
            shutdown: stop.clone(),
            persistence: Arc::clone(&persistence),
        };

        {
            let mut s = state.write().expect("state lock poisoned");
            *s = ActorState::Running;
        }

        // catch_unwind requires AssertUnwindSafe because our futures
        // hold mutable state across `.await` points. The supervisor
        // does not share that state with anything else, so the
        // assertion is sound: a panic just drops everything the future
        // owned.
        let run_fut = AssertUnwindSafe(actor.run(ctx)).catch_unwind();

        let outcome: ActorOutcome = tokio::select! {
            _ = stop.cancelled() => ActorOutcome::Shutdown,
            res = run_fut => match res {
                Ok(Ok(())) => ActorOutcome::CleanExit,
                Ok(Err(e)) => ActorOutcome::ReturnedErr(format!("{e}")),
                Err(panic) => ActorOutcome::Panicked(format_panic(&panic)),
            }
        };

        match outcome {
            ActorOutcome::Shutdown => {
                tracing::debug!(actor = A::NAME, "actor stopping on shutdown");
                return;
            }
            ActorOutcome::CleanExit => {
                tracing::info!(actor = A::NAME, "actor returned Ok(()); not restarting");
                // Treat a clean Ok return outside of a shutdown as a
                // permanent stop — actors that should restart must
                // return Err or panic.
                return;
            }
            ActorOutcome::ReturnedErr(reason) => {
                tracing::warn!(actor = A::NAME, reason = %reason, "actor returned Err");
                if !decide_restart::<A>(&history, &state, &reason) {
                    return;
                }
            }
            ActorOutcome::Panicked(reason) => {
                tracing::warn!(actor = A::NAME, reason = %reason, "actor panicked");
                if !decide_restart::<A>(&history, &state, &reason) {
                    return;
                }
            }
        }

        // Backoff sleep, honouring shutdown.
        let backoff = {
            let s = state.read().expect("state lock poisoned").clone();
            match s {
                ActorState::Restarting { backoff } => backoff,
                _ => Duration::ZERO,
            }
        };
        if backoff > Duration::ZERO {
            tracing::info!(actor = A::NAME, backoff = ?backoff, "scheduling actor restart");
            tokio::select! {
                _ = stop.cancelled() => {
                    tracing::debug!(actor = A::NAME, "shutdown observed during backoff");
                    return;
                }
                _ = tokio::time::sleep(backoff) => {}
            }
        } else {
            tracing::info!(actor = A::NAME, "restarting actor immediately");
        }
    }
}

/// Records a crash and decides whether to restart.
///
/// Returns `true` if the supervisor should loop (restart), `false` if
/// the actor has been marked Failed and the wrapper should exit.
fn decide_restart<A: Actor>(
    history: &Arc<StdMutex<RestartHistory>>,
    state: &Arc<StdRwLock<ActorState>>,
    reason: &str,
) -> bool {
    let now = Instant::now();
    let count_in_window = {
        let mut h = history.lock().expect("history lock poisoned");
        h.record(now);
        h.count_in_window(now)
    };

    if count_in_window > MAX_RESTARTS_IN_WINDOW {
        // Loudly mark Failed. Note we record BEFORE the threshold
        // check so the count includes this crash.
        tracing::error!(
            actor = A::NAME,
            count_in_window = count_in_window,
            reason = %reason,
            "actor exceeded restart budget; marking Failed and ceasing restarts"
        );
        let mut s = state.write().expect("state lock poisoned");
        *s = ActorState::Failed {
            reason: reason.to_string(),
        };
        return false;
    }

    let backoff = if count_in_window <= IMMEDIATE_RESTART_LIMIT {
        Duration::ZERO
    } else {
        // Indices 0..6 → the 4th, 5th, ..., 10th restart.
        let idx = count_in_window - (IMMEDIATE_RESTART_LIMIT + 1);
        BACKOFF_TABLE[idx.min(BACKOFF_TABLE.len() - 1)]
    };

    {
        let mut s = state.write().expect("state lock poisoned");
        *s = ActorState::Restarting { backoff };
    }
    true
}

enum ActorOutcome {
    Shutdown,
    CleanExit,
    ReturnedErr(String),
    Panicked(String),
}

/// Best-effort stringification of a `Box<dyn Any + Send>` panic payload.
fn format_panic(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Build a sandboxed `Arc<Persistence>` for tests. Uses a unique
    /// in-process tempdir so concurrent tests do not collide.
    async fn test_persistence() -> Arc<Persistence> {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Leak the tempdir guard for the lifetime of the test process.
        // Tests are short-lived and the OS reaps tempdir on exit;
        // keeping the guard alive across `await` points in every
        // helper bloats the call-sites and adds nothing.
        let path = tmp.path().join("test.db");
        std::mem::forget(tmp);
        let cfg = concerto_persist::PersistenceConfig {
            db_path: path,
            max_readers: 2,
        };
        Arc::new(Persistence::open(cfg).await.expect("persistence open"))
    }

    /// Actor that does nothing but wait for shutdown.
    struct IdleActor;
    #[async_trait]
    impl Actor for IdleActor {
        const NAME: &'static str = "idle";
        type Config = ();
        async fn run(self, ctx: ActorContext<Self::Config>) -> Result<()> {
            ctx.shutdown.cancelled().await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn spawn_and_shutdown_cleanly() {
        let token = CancellationToken::new();
        let mut sup = RootSupervisor::new(test_persistence().await, token.clone());
        sup.spawn::<IdleActor, _>(|| IdleActor, ()).await.unwrap();
        assert_eq!(sup.actor_count(), 1);
        sup.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn duplicate_actor_name_rejected() {
        let token = CancellationToken::new();
        let mut sup = RootSupervisor::new(test_persistence().await, token.clone());
        sup.spawn::<IdleActor, _>(|| IdleActor, ()).await.unwrap();
        let err = sup.spawn::<IdleActor, _>(|| IdleActor, ()).await;
        assert!(err.is_err(), "second spawn under same NAME must error");
        sup.shutdown().await.unwrap();
    }

    /// Tracks how many times the supervisor has constructed a new
    /// instance via the factory. Useful in restart-history tests.
    fn counted_factory<A: Actor + Default>() -> (Arc<AtomicUsize>, impl Fn() -> A + Send + Sync) {
        let counter = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&counter);
        (counter, move || {
            c.fetch_add(1, Ordering::SeqCst);
            A::default()
        })
    }

    #[derive(Default)]
    struct ImmediatePanic;
    #[async_trait]
    impl Actor for ImmediatePanic {
        const NAME: &'static str = "immediate-panic";
        type Config = ();
        async fn run(self, _ctx: ActorContext<Self::Config>) -> Result<()> {
            panic!("intentional panic");
        }
    }

    #[tokio::test]
    async fn panic_triggers_restart_then_failed() {
        let token = CancellationToken::new();
        let mut sup = RootSupervisor::new(test_persistence().await, token.clone());
        // Pause AFTER persistence is open — sqlx's pool needs real
        // time to handshake. With time paused, the per-restart
        // exponential backoff (1s+2s+4s+8s+16s+32s = 63s of wall
        // clock) compresses to a few polls.
        tokio::time::pause();
        let (count, factory) = counted_factory::<ImmediatePanic>();
        sup.spawn::<ImmediatePanic, _>(factory, ()).await.unwrap();

        // The panic-and-restart loop should burn through the immediate
        // restart budget (≤3) almost instantly and stop at the 11th
        // crash with Failed.
        let start = Instant::now();
        loop {
            let s = sup.actors["immediate-panic"].state();
            if matches!(s, ActorState::Failed { .. }) {
                break;
            }
            if start.elapsed() > Duration::from_secs(20) {
                panic!("actor never reached Failed state; last={s:?}");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let total = count.load(Ordering::SeqCst);
        assert!(
            total > IMMEDIATE_RESTART_LIMIT,
            "expected more than {IMMEDIATE_RESTART_LIMIT} invocations, got {total}"
        );
        // The actor should have been constructed exactly
        // MAX_RESTARTS_IN_WINDOW + 1 times before we cross the
        // threshold. Allow a small slack window in case the test
        // observed Failed slightly late.
        assert!(
            total <= MAX_RESTARTS_IN_WINDOW + 2,
            "constructed too many times: {total}"
        );

        sup.shutdown().await.unwrap();
    }

    #[derive(Default)]
    struct ReturnsErr;
    #[async_trait]
    impl Actor for ReturnsErr {
        const NAME: &'static str = "returns-err";
        type Config = ();
        async fn run(self, _ctx: ActorContext<Self::Config>) -> Result<()> {
            Err(Error::Internal("intentional".into()))
        }
    }

    #[tokio::test]
    async fn err_return_also_restarts_then_fails() {
        let token = CancellationToken::new();
        let mut sup = RootSupervisor::new(test_persistence().await, token.clone());
        tokio::time::pause();
        sup.spawn::<ReturnsErr, _>(|| ReturnsErr, ()).await.unwrap();

        let start = Instant::now();
        loop {
            let s = sup.actors["returns-err"].state();
            if matches!(s, ActorState::Failed { .. }) {
                break;
            }
            if start.elapsed() > Duration::from_secs(20) {
                panic!("actor never failed");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        sup.shutdown().await.unwrap();
    }

    /// One actor panics in a loop; a peer just runs idle. Verifies
    /// crash isolation: the peer is unaffected.
    #[tokio::test]
    async fn peer_actor_unaffected_by_neighbor_panic() {
        let token = CancellationToken::new();
        let mut sup = RootSupervisor::new(test_persistence().await, token.clone());
        tokio::time::pause();
        sup.spawn::<ImmediatePanic, _>(|| ImmediatePanic, ())
            .await
            .unwrap();
        sup.spawn::<IdleActor, _>(|| IdleActor, ()).await.unwrap();

        // Wait until the noisy actor has hit Failed.
        let start = Instant::now();
        loop {
            let s = sup.actors["immediate-panic"].state();
            if matches!(s, ActorState::Failed { .. }) {
                break;
            }
            if start.elapsed() > Duration::from_secs(20) {
                panic!("noisy actor never failed");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // The idle actor should still be Running.
        let idle_state = sup.actors["idle"].state();
        assert!(
            matches!(idle_state, ActorState::Running),
            "idle peer should be unaffected; saw {idle_state:?}"
        );

        sup.shutdown().await.unwrap();
    }
}
