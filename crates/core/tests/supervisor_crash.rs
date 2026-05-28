//! Integration tests for the Task 12 supervision tree.
//!
//! These complement the in-crate unit tests in `crates/core/src/supervisor.rs`.
//! The four mandatory tests from the task spec are covered as follows:
//!
//! 1. **Actor that panics on Nth iteration; restart history tracks
//!    correctly** — `panic_on_third_iteration_tracks_history`.
//! 2. **Actor that returns Err; verify Failed state** — covered by
//!    `crate::supervisor::tests::err_return_also_restarts_then_fails`
//!    in the supervisor module. Replicated here as
//!    `err_actor_reaches_failed_in_integration` for cross-crate
//!    visibility.
//! 3. **Actor that hangs; shutdown forces it down within 10s + 1s
//!    grace** — `hanging_actor_is_aborted_on_shutdown`.
//! 4. **Two actors run in parallel; one panics; the other is
//!    unaffected** — `parallel_actors_isolated_on_panic`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use concerto_core::supervisor::{Actor, ActorContext, ActorState, RootSupervisor};
use concerto_error::{Error, Result};
use concerto_persist::{Persistence, PersistenceConfig};
use tokio_util::sync::CancellationToken;

async fn fresh_persistence() -> Arc<Persistence> {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("supervisor-test.db");
    std::mem::forget(tmp);
    let cfg = PersistenceConfig {
        db_path: path,
        max_readers: 2,
    };
    Arc::new(Persistence::open(cfg).await.expect("persistence open"))
}

/// Actor whose `run` body checks an atomic counter and panics only on
/// the third invocation; otherwise it sleeps a tick and returns
/// `Err(_)` so the supervisor will restart it.
struct PanicOnThird {
    counter: Arc<AtomicUsize>,
}

#[async_trait]
impl Actor for PanicOnThird {
    const NAME: &'static str = "panic-on-third";
    type Config = ();
    async fn run(self, _ctx: ActorContext<Self::Config>) -> Result<()> {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        if n == 2 {
            panic!("panic on the third iteration");
        }
        // Yield then return Err so the supervisor schedules a restart.
        tokio::time::sleep(Duration::from_millis(5)).await;
        Err(Error::Internal(format!("planned failure at iteration {n}")))
    }
}

#[tokio::test]
async fn panic_on_third_iteration_tracks_history() {
    let token = CancellationToken::new();
    let mut sup = RootSupervisor::new(fresh_persistence().await, token.clone());
    // Pause AFTER persistence is open — sqlx's pool needs real time
    // to handshake. With time paused, the per-restart exponential
    // backoff (up to 32s per restart) compresses to virtual time so
    // the test runs in <1s of wall clock.
    tokio::time::pause();

    let counter = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&counter);
    sup.spawn::<PanicOnThird, _>(
        move || PanicOnThird {
            counter: Arc::clone(&c),
        },
        (),
    )
    .await
    .unwrap();

    // Wait until at least 5 iterations have observably happened
    // (covers the panic and at least one restart after it).
    let start = Instant::now();
    while counter.load(Ordering::SeqCst) < 5 {
        if start.elapsed() > Duration::from_secs(15) {
            panic!(
                "counter only reached {} in 15s; supervisor didn't restart",
                counter.load(Ordering::SeqCst)
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Check the supervisor's view: per-actor restart_total should
    // match the factory's invocation count modulo race.
    let snap = sup.list();
    let summary = snap.iter().find(|s| s.name == "panic-on-third").unwrap();
    assert!(
        summary.restart_total >= 3,
        "restart history should record each crash; saw {}",
        summary.restart_total
    );

    sup.shutdown().await.unwrap();
}

/// Returns `Err` immediately. The supervisor will restart it until the
/// crash budget is exhausted, then mark it Failed.
struct ErrActor;
#[async_trait]
impl Actor for ErrActor {
    const NAME: &'static str = "err-actor";
    type Config = ();
    async fn run(self, _ctx: ActorContext<Self::Config>) -> Result<()> {
        Err(Error::Internal("nope".into()))
    }
}

#[tokio::test]
async fn err_actor_reaches_failed_in_integration() {
    let token = CancellationToken::new();
    let mut sup = RootSupervisor::new(fresh_persistence().await, token.clone());
    tokio::time::pause();
    sup.spawn::<ErrActor, _>(|| ErrActor, ()).await.unwrap();

    let start = Instant::now();
    loop {
        let snap = sup.list();
        let err_summary = snap.iter().find(|s| s.name == "err-actor").expect("found");
        if matches!(err_summary.state, ActorState::Failed { .. }) {
            break;
        }
        if start.elapsed() > Duration::from_secs(20) {
            panic!(
                "err-actor never reached Failed; saw {:?}",
                err_summary.state
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    sup.shutdown().await.unwrap();
}

/// Actor that ignores shutdown for an absurd time. Used to verify the
/// supervisor's hard-abort path.
struct HangingActor;
#[async_trait]
impl Actor for HangingActor {
    const NAME: &'static str = "hanging-actor";
    type Config = ();
    async fn run(self, _ctx: ActorContext<Self::Config>) -> Result<()> {
        // Note: we deliberately do NOT `select!` on ctx.shutdown. The
        // supervisor's per-actor 10s drain budget should expire and
        // the join handle should be aborted.
        tokio::time::sleep(Duration::from_secs(3600)).await;
        Ok(())
    }
}

#[tokio::test]
async fn hanging_actor_is_aborted_on_shutdown() {
    let token = CancellationToken::new();
    let mut sup = RootSupervisor::new(fresh_persistence().await, token.clone());
    sup.spawn::<HangingActor, _>(|| HangingActor, ())
        .await
        .unwrap();

    // Give the actor a moment to enter Running.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // shutdown() should return within 10s + 1s grace = 11s.
    let start = Instant::now();
    tokio::time::timeout(Duration::from_secs(12), sup.shutdown())
        .await
        .expect("shutdown must return within 10s drain budget + slack")
        .expect("shutdown returns Ok");
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(12),
        "shutdown took {elapsed:?}; expected < 12s"
    );
}

/// Twin of the in-crate `peer_actor_unaffected_by_neighbor_panic` test,
/// repeated at the integration boundary so failures here surface as
/// "the public API is broken" rather than "private internals drifted".
#[derive(Default)]
struct AlwaysPanic;
#[async_trait]
impl Actor for AlwaysPanic {
    const NAME: &'static str = "always-panic";
    type Config = ();
    async fn run(self, _ctx: ActorContext<Self::Config>) -> Result<()> {
        panic!("kaboom");
    }
}

struct PeerIdle;
#[async_trait]
impl Actor for PeerIdle {
    const NAME: &'static str = "peer-idle";
    type Config = ();
    async fn run(self, ctx: ActorContext<Self::Config>) -> Result<()> {
        ctx.shutdown.cancelled().await;
        Ok(())
    }
}

#[tokio::test]
async fn parallel_actors_isolated_on_panic() {
    let token = CancellationToken::new();
    let mut sup = RootSupervisor::new(fresh_persistence().await, token.clone());
    tokio::time::pause();
    sup.spawn::<AlwaysPanic, _>(|| AlwaysPanic, ())
        .await
        .unwrap();
    sup.spawn::<PeerIdle, _>(|| PeerIdle, ()).await.unwrap();

    // Wait for the noisy actor to be marked Failed.
    let start = Instant::now();
    loop {
        let snap = sup.list();
        let noisy = snap
            .iter()
            .find(|s| s.name == "always-panic")
            .expect("found");
        if matches!(noisy.state, ActorState::Failed { .. }) {
            break;
        }
        if start.elapsed() > Duration::from_secs(20) {
            panic!("noisy actor never failed");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Peer must still be Running.
    let snap = sup.list();
    let peer = snap.iter().find(|s| s.name == "peer-idle").expect("found");
    assert!(
        matches!(peer.state, ActorState::Running),
        "peer should remain Running; saw {:?}",
        peer.state
    );

    sup.shutdown().await.unwrap();
}
