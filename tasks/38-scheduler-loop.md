# Task 38 — Scheduler `/loop` Primitive

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 22, 31 |
| Touches subsystem(s) | 05 (Scheduler), 04 (Agent Supervisor), 09 (Persistence) |
| Smoke gate | unchanged |

## Goal
Implement `/loop` — the session-scoped recurring task primitive from `design/05`. After this task, a user (or agent via slash command) can create a `/loop` with an interval; the Scheduler fires it on schedule, spawns a session in the workarea with the configured prompt, and tracks runs. Loops expire after 3 days. Persistent scheduled tasks (cron-based) are V1.0.

## Inputs to read before starting
- `design/05_Scheduler.md` §1 (scope: V0.1 is /loop only), §3.1 (unified schedules table), §3.4 (run dispatch via 04), §6.1 (fire loop), §6.2 (inflight suppression), §6.3 (crash recovery).
- `design/09_Persistence.md` §4.3 (`schedules` + `schedule_runs` schema).

## Scope — in
- Add migration `0003_schedules.sql` creating `schedules` and `schedule_runs` tables per `design/09 §4.3`.
- Implement `crates/core/src/scheduler/`:
  - `SchedulerActor` (impl `Actor`).
  - `create_schedule(req)` — V0.1 supports `kind=loop` only; reject others with `INVALID_ARGUMENT`. Validates: `workarea_id` required, `interval_seconds` in 30..=604800 (per `design/05 §12 R-3`), `expires_at` defaults to `now + 3 days`.
  - Fire loop: a single tokio task with a `BTreeMap<Instant, ScheduleId>`. Wakes on the next-fire time; on fire, spawns a session via `AgentSupervisorHandle::start_session` with the schedule's prompt and `agent_kind` (default `claude`). Re-schedules for `now + interval_seconds`.
  - Inflight suppression: if a previous run is still active when the next fires, emit `schedule.suppressed { reason: inflight }` and skip. (V0.1: no concurrent runs.)
  - Expiration sweep: every 5 min, deactivate loops whose `expires_at < now`.
  - Crash recovery: on Core start, load unpaused, unexpired schedules; rebuild the BTreeMap; recompute `next_fire = max(last_run + interval, now)`.
- gRPC: `Schedules.CreateSchedule`, `Schedules.ListSchedules`, `Schedules.PauseSchedule`, `Schedules.DeleteSchedule`, `Schedules.GetScheduleHistory`.
- Tests:
  - Create a loop with interval=2s on a workarea; let it fire at least twice; assert `schedule_runs` rows.
  - Pause a loop; verify no new firings.
  - Expire: set `expires_at` in the past via direct SQL; restart Core; verify the loop is not loaded.
  - Inflight suppression: fire with the previous run hung; verify the second fires-attempt is suppressed.

## Scope — out
- Cron-based persistent scheduled tasks (V1.0).
- Cloud-task sync (V1.0).
- Promote loop → scheduled (V1.0).
- Budget guardrails (V1.0).
- Jitter (V0.1 has no cron — only intervals).
- `wait_for_check_runs` (V1.0 — needed when PR sets arrive).
- Per-account daily caps (V1.0).

## Public interface this task locks
- Rust: `crates/core/src/scheduler/mod.rs` — `SchedulerHandle::create_schedule`, `.list_schedules`, `.pause_schedule`, `.delete_schedule`, `.fire_now`. Frozen.
- Proto: `Schedules` service per `design/10 §5.1` (V0.1 subset). Frozen field numbers.
- DB migration `0003_schedules.sql`. Frozen.
- Interval bounds: 30s ≤ interval ≤ 7 days. Frozen.

## Implementation notes
- Use `tokio::time::sleep_until(tokio::time::Instant::from_std(target))` for fire-time scheduling.
- The fire loop uses `select!` with two branches: sleep-until-next-fire, and a `Notify` that wakes on add/update/delete.
- Each fire spawns a session non-blockingly — the fire loop never awaits the agent's completion.
- For tracking when a run is "still in flight": subscribe to `session.events.<sid>` for the run's session; mark the `schedule_runs` row terminal when `TurnComplete` or `Exited` arrives.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core scheduler` → tests pass.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual: create a 30s loop; observe at least 2 firings in 1 minute; verify each spawns a session.
5. `./scripts/regen-interfaces.sh && git diff` → committed (schema + proto).
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] Loops fire and create schedule_runs rows.
- [ ] Pause / delete take effect immediately.
- [ ] Crash recovery rebuilds the BTreeMap correctly.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/persist/migrations/0003_schedules.sql` (new)
- `crates/persist/src/schedules.rs` (new)
- `crates/persist/src/schedule_runs.rs` (new)
- `crates/persist/src/lib.rs` (modified)
- `crates/core/src/scheduler/mod.rs` (new)
- `crates/core/src/scheduler/actor.rs` (new)
- `crates/core/src/scheduler/fire_loop.rs` (new)
- `crates/proto/proto/concerto/v1/schedules.proto` (new)
- `crates/core/src/handlers/schedules.rs` (new)
- `crates/core/src/main.rs` (modified — spawn SchedulerActor)
- `crates/core/tests/scheduler_loop.rs` (new)
- `docs/interfaces/proto.md`, `rust-api.md`, `schema.md` (regenerated)

## Commit message
```
phase-3: scheduler /loop primitive

Session-scoped recurring tasks tied to a workarea. Interval-based
firing 30s..7d, 3-day expiry, inflight suppression. Cron-based
persistent schedules are V1.0.

Refs: tasks/38-scheduler-loop.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** persistent scheduled tasks, cron, cloud sync, promote — all V1.0.
- **Smoke-gate state:** unchanged.
