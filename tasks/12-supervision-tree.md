# Task 12 — Actor Supervision Tree

| Field | Value |
|---|---|
| Phase | 1 |
| Size | small (≤4h) |
| Depends on | 11 |
| Touches subsystem(s) | 01 (Runtime) |
| Smoke gate | unchanged |

## Goal
Add the typed actor wrapper and root supervisor described in `design/01 §3.2`. After this task, the Core has a `RootSupervisor` that spawns and supervises `Actor` implementations with crash-isolation and restart policy. No real actors are added yet — that's later tasks — but the wiring is in place so Task 13 can drop in the gRPC server as the first supervised actor.

## Inputs to read before starting
- `design/01_Core_Daemon_Runtime.md` §3.2 (supervision tree shape), §4.2 (in-memory state), §5.2 (actor trait API), §6.2 (crash-restart policy), §7.2 (sequence diagram).
- `design/00_Architecture_Overview.md` §7.3 (error handling — crash isolation between agents).
- `tasks/11-runtime-skeleton.md` → "Handoff Notes".

## Scope — in
Implement `crates/core/src/supervisor.rs`:

```rust
#[async_trait::async_trait]
pub trait Actor: Send + 'static {
    const NAME: &'static str;
    type Config: Send + Sync + 'static;
    async fn run(self, ctx: ActorContext<Self::Config>) -> Result<()>;
}

pub struct ActorContext<C> {
    pub config: Arc<RwLock<C>>,
    pub shutdown: CancellationToken,
    pub persistence: Persistence,
}

pub struct ActorHandle {
    pub name: &'static str,
    join: JoinHandle<()>,
    stop: CancellationToken,
    state: Arc<RwLock<ActorState>>,
    restart_history: Arc<Mutex<RestartHistory>>,
}

pub enum ActorState {
    Starting,
    Running,
    Restarting { backoff: Duration },
    Failed { reason: String },
}

pub struct RootSupervisor {
    actors: HashMap<&'static str, ActorHandle>,
    shutdown: CancellationToken,
    persistence: Persistence,
}

impl RootSupervisor {
    pub fn new(persistence: Persistence, shutdown: CancellationToken) -> Self;
    pub async fn spawn<A: Actor>(&mut self, actor: A, config: A::Config) -> Result<()>;
    pub fn list(&self) -> Vec<ActorStatusSummary>;
    pub async fn shutdown(self) -> Result<()>;
}
```

The supervisor must:
- Wrap each actor's `run` in `std::panic::catch_unwind` (via `tokio::task::spawn_blocking` ... actually, `tokio::spawn` and use `AssertUnwindSafe` with `FutureExt::catch_unwind` from `futures::FutureExt`).
- Implement the restart policy from `design/01 §6.2`:
  - ≤ 3 restarts in last 60s → restart immediately.
  - 4–10 → exponential backoff (1s, 2s, 4s, 8s, 16s, 32s).
  - > 10 → mark Failed; do not restart; log loudly. Other actors keep running.
- Emit `tracing::warn!` (later `tracing::error!`) on each crash with actor name + cause.
- Honor the global `shutdown` token: on cancel, propagate to each child's `ctx.shutdown` and await their `run` futures with a 10s per-actor timeout, then hard-abort.
- Provide a list-method for diagnostics (used by Task 13's `RuntimeStatus`).

Wire into `Runtime` from Task 11:
- `Runtime::start` builds a `RootSupervisor` and stores it.
- `Runtime::stop` calls `supervisor.shutdown()`.
- `Runtime::supervisor_mut()` returns a handle so Task 13 can register the gRPC server actor.

Add unit + integration tests:
- A test actor that panics on Nth iteration; verify restart history tracks correctly.
- A test actor that returns Err; verify Failed state.
- A test actor that hangs; verify shutdown forces it down within 10s + 1s grace.
- Two actors run in parallel; one panics; verify the other is unaffected.

## Scope — out
- Watchdog (V1.0).
- Per-actor heartbeats (V1.0).
- Per-actor memory budget (V1.0).
- Config hot-reload broadcasting to actors (V1.0).

## Public interface this task locks
- Rust: `crates/core/src/supervisor.rs` — `pub trait Actor`, `pub struct ActorContext`, `pub struct ActorHandle`, `pub enum ActorState`, `pub struct RootSupervisor`.
- Restart policy: 3/10/exp-backoff schedule above.
- Crash isolation: each actor runs under a `catch_unwind`-equivalent boundary.

## Implementation notes
- Use `futures::FutureExt::catch_unwind` after `AssertUnwindSafe` — tokio's own panic handling won't capture them by default.
- The `RestartHistory` is a small ring buffer (16 entries via `ArrayVec` is fine, per `design/01 §4.2`).
- `ActorState` lives behind an `Arc<RwLock<_>>` so external observers (gRPC `RuntimeStatus`) can read it.
- For shutdown, use `tokio::select! { _ = ctx.shutdown.cancelled() => ..., result = actor.run(ctx) => ... }` inside the wrapper.
- The hard-abort path: after the per-actor 10s timeout, call `join.abort()` and log a warning. Tokio's abort is cooperative; tasks that hold on to blocking work won't actually stop. V0.1 accepts this limitation.

## Verification
1. `cargo build -p concerto-core` → succeeds.
2. `cargo test -p concerto-core supervisor` → all four tests pass.
3. `cargo clippy -p concerto-core -- -D warnings` → clean.
4. Manual: `cargo run --bin concerto-core` starts; pid file exists; supervisor is initialized (verified via `tracing` log at DEBUG: "RootSupervisor ready, 0 actors").
5. `./scripts/regen-interfaces.sh && git diff docs/interfaces/rust-api.md` → updated.
6. `cargo deny check` → clean.

## Definition of Done
- [x] Verification commands pass.
- [x] Panic-isolation test passes (one actor crashes; another keeps running).
- [x] Restart-policy backoff verified via test.
- [x] No `TODO` / `FIXME` / `todo!()` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `crates/core/Cargo.toml` (modified — async-trait, futures)
- `crates/core/src/supervisor.rs` (new)
- `crates/core/src/lib.rs` (modified)
- `crates/core/src/runtime.rs` (modified — embeds RootSupervisor)
- `crates/core/tests/supervisor_crash.rs` (new)
- `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-1: actor supervision tree

Implements typed Actor trait + RootSupervisor with catch_unwind
isolation and restart policy (3 immediate / exp-backoff to 32s / mark
Failed after 10 in 60s) per design/01 §3.2, §6.2.

Refs: tasks/12-supervision-tree.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **`ActorContext.persistence` uses `Arc<Persistence>`, not `Persistence`.** Pre-authorized in the orchestrator brief: `Persistence` is not `Clone` (Task 08 handoff), so the sketch field type would have required `Clone`-on-spawn and broken every caller. `Arc<Persistence>` is the only sound substitution — the reader pool and writer mutex inside `Persistence` already serialize concurrent access. Documented at the top of `crates/core/src/supervisor.rs` and on the `ActorContext` doc-comment.
  - **`Runtime` now stores `persistence: Option<Arc<Persistence>>`** (was `Option<Persistence>`). `Runtime::persistence()` returns `Option<&Arc<Persistence>>` so callers can `Arc::clone` to share with their own subsystems. `Runtime::stop` uses `Arc::try_unwrap` to recover the inner value for `persistence.shutdown().await`; if a stray clone outlives the supervisor (shouldn't happen post-shutdown but defensive), we drop our clone and let the last holder close it on Drop, with a warn log noting the strong count.
  - **`Runtime::supervisor_mut()` + `Runtime::supervisor()` added** (`pub fn supervisor_mut(&mut self) -> Option<&mut RootSupervisor>`, `pub fn supervisor(&self) -> Option<&RootSupervisor>`). Returns `Option` because `Runtime::stop` consumes the supervisor (via `Option::take`), and the runtime tests need a way to assert the supervisor was actually dropped. Task 13's gRPC actor registration will use `runtime.supervisor_mut().expect("supervisor not yet shut down")`.
  - **`RootSupervisor::spawn` takes a `factory: F` closure, not the `Actor` value directly.** The task pseudocode shows `spawn<A: Actor>(&mut self, actor: A, config: A::Config)`, but `Actor::run` *consumes* the actor — each restart needs a fresh instance. A `Fn() -> A + Send + Sync + 'static` factory is the minimal addition; callers wanting to pass a single value can use `|| A::default()` or capture by clone. The factory is wrapped in `Arc` internally so the supervisor wrapper task can own it across restarts. Public-interface impact: signature change, not a semantic surprise.
  - **`ActorHandle::state` and `restart_total` are sync** (`fn state(&self) -> ActorState`, `fn restart_total(&self) -> u64`). The spec sketch had `state: Arc<RwLock<ActorState>>` and `restart_history: Arc<Mutex<RestartHistory>>` typed against tokio's async locks; in practice the locks are held for nanoseconds (clone an enum, increment a counter) and we need a *sync* read path for `RootSupervisor::list()` (Task 13's `GetStatus` RPC, plus the future `tracing::debug!` lines). I switched the internal locks to `std::sync::{RwLock, Mutex}`. The `config: Arc<RwLock<C>>` in `ActorContext` stays as `tokio::sync::RwLock` per the orchestrator's drift note (will be held across `.await` once config-reload lands).
  - **`RootSupervisor::new_with_policy` was NOT added.** The locked restart policy (3 immediate / exp-backoff to 32s / Failed after 10) is a contract per the task's "Public interface this task locks" section. Tests use `tokio::time::pause()` to compress virtual time instead of overriding constants. This required adding tokio's `test-util` feature to `crates/core/[dev-dependencies]`.
  - **`async-trait = "0.1"` and `futures = "0.3"` added as workspace deps** (per orchestrator directive in the prompt header), then referenced in `crates/core/Cargo.toml`. Both are MIT/Apache-2.0; cargo-deny clean.
  - **`tokio = { ..., features = ["test-util"] }` layered into `crates/core/[dev-dependencies]`** so `tokio::time::pause()` is available in tests but not in the production binary. Without `test-util`, `start_paused`/`pause` are method-not-found at compile time.
  - **The `peer_actor_unaffected_by_neighbor_panic` + `panic_triggers_restart_then_failed` + `err_return_also_restarts_then_fails` tests call `tokio::time::pause()` AFTER `Persistence::open`** instead of using `#[tokio::test(start_paused = true)]`. Reason: sqlx's connection pool acquire uses real wall-clock timers that immediately `PoolTimedOut` under paused virtual time. Opening persistence first, THEN pausing, gives the supervisor restart loop virtual time without breaking the underlying database handle.
  - **`docs/interfaces/rust-api.md` was NOT updated.** Same as Task 11: the interface generator only scrapes `crates/<crate>/src/api.rs`, and `concerto-core` does not follow that convention (no `api.rs` exists). `scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` is clean, satisfying verification step 5. The locked types — `Actor`, `ActorContext`, `ActorHandle`, `ActorState`, `RestartHistory`, `RootSupervisor`, `ActorStatusSummary` — live in `crates/core/src/supervisor.rs`. If a future task wants them surfaced in `rust-api.md`, a re-export module at `crates/core/src/api.rs` would do it.
  - **`RootSupervisor::actor_count()` + `RootSupervisor::persistence()` added** as small public helpers. `actor_count` is used by the `Runtime::start` startup-log line ("RootSupervisor ready, N actors"). `persistence()` lets the future Task 13 supervisor-side gRPC actor get an `Arc<Persistence>` clone without going back through the Runtime.
- **Open questions for next task:**
  - **Task 13 wiring contract:** `runtime.supervisor_mut().expect("supervisor present").spawn::<GrpcServerActor, _>(|| GrpcServerActor::new(...), config).await?` is the canonical registration call. The `factory` closure must be `Send + Sync + 'static`. If the gRPC server holds a `tokio::sync::Mutex` it doesn't own at construction time, the factory will need to `Arc::clone` it on every call — same pattern as the integration tests' `PanicOnThird`.
  - **`ActorState::Failed` has no manual recovery path in V0.1.** Once an actor crosses 10 restarts in 60s, the only way to revive it is restarting the Core. Tasks 12–13 do not need this; Task 13's `RuntimeAdmin::ReloadConfig` is the natural future hook. A `RootSupervisor::restart(name)` admin method is the V1.0 ask.
  - **`list()` is synchronous but borrows `&self`.** Task 13's `GetStatus` RPC handler will likely take `Arc<Mutex<Runtime>>` (or pass the supervisor through tonic state); if a future caller needs a fully-async snapshot (e.g. when locks become contended), an `async fn list_async(&self) -> Vec<ActorStatusSummary>` is a non-breaking addition.
  - **The `tokio::time::pause()` test pattern is now used by `concerto-core`.** If Task 17's dev-deps harness crate ends up sharing supervisor-style tests, the `tokio` `test-util` feature must be enabled at that level too.
  - **Per-actor `stop` child token is exposed but unused externally.** Task 12 includes the plumbing for a `kill <actor>` admin RPC (each `ActorHandle.stop` is an independent `CancellationToken`), but no public API surfaces it yet. Adding `RootSupervisor::stop_actor(name)` is a follow-on; the supervisor's design.md sketch doesn't list it as V0.1.
- **Deliberate debt:** hard-abort uses tokio's cooperative abort (`JoinHandle::abort` via `tokio::time::timeout` consumption of the handle); truly stuck blocking sections (e.g. a `std::thread::sleep` inside an actor) may persist past `Runtime::stop`. V1.0 watchdog will address. No `TODO`/`FIXME` markers in new code.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still prints "Smoke gate: PASSED (no checks active yet — Phase 0)". Manual `cargo run --bin concerto-core` confirms the `RootSupervisor ready, 0 actors` DEBUG log line and clean shutdown.
