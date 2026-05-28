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
- [ ] Verification commands pass.
- [ ] Panic-isolation test passes (one actor crashes; another keeps running).
- [ ] Restart-policy backoff verified via test.
- [ ] No `TODO` / `FIXME` / `todo!()` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

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
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** hard-abort uses tokio's cooperative abort; truly stuck blocking tasks may persist (V1.0 watchdog will address).
- **Smoke-gate state:** unchanged.
