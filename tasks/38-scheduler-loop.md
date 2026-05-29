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
- [x] Verification commands pass.
- [x] Loops fire and create schedule_runs rows.
- [x] Pause / delete take effect immediately.
- [x] Crash recovery rebuilds the BTreeMap correctly.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

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
- **Drift from plan:**
  - **Migration is `0004_schedules.sql`.** The task body said `0003_schedules.sql`; pre-decision #1 in the orchestrator prompt corrected this to 0004 because Task 30 (`workareas.settings_json`) shipped as 0002 and Task 36 (`sessions.last_acked_seq`) shipped as 0003. Confirmed against the migrations directory before writing.
  - **`schedules` schema is the V0.1 subset of design/09 §4.3**, not the full table. Columns shipped: `id, workarea_id, kind, interval_seconds, expires_at, last_run_at, paused, prompt, agent_kind, created_at`. The persistent-scheduled-task columns (`cron_expr`, `model`, `permission_mode`, `bypass_destructive_guard`, `worktree_mode`, `failure_policy_json`, `daily_budget_tokens`, `project_id`, `workspace_id`, `name`) from design/09 §4.3 are V1.0 and arrive in a later numbered migration alongside the cron + budget surface. `workarea_id` is `NOT NULL` per pre-decision #2 (loops are session-scoped); the `kind` CHECK is locked to `'loop'` for V0.1. `schedule_runs` similarly uses the pre-decision shape (`started_at, ended_at, terminal_state`) rather than design/09's `started_at, finished_at, status, tokens_in, tokens_out, error_message`; token bookkeeping rides with V1.0 budgets.
  - **`SchedulerHandle::agent_supervisor` is `Option<AgentSupervisorHandle>`.** Pre-decision #4 listed an unconditional supervisor field; in practice the integration test wants to exercise the persistence + suppression paths without a real `concerto-agent-host` running, so the field is optional. Production `main.rs` always wires `Some(supervisor)` (see Task 38 §"Outputs" `crates/core/src/main.rs`); the `None` path errors `scheduler.no_supervisor` when the fire loop actually tries to start a session.
  - **Inflight suppression checks the in-memory map AND the DB.** The pre-decision listed a `HashMap<ScheduleId, RunId>` check only; the implementation also queries `schedule_runs WHERE ended_at IS NULL` so a Core restart that lost the in-memory map still honours the suppression. Both checks emit the same `tracing::info!("schedule.suppressed reason=inflight")` log line so the wire-level audit shape is uniform.
  - **`fire_schedule` inserts the run row BEFORE calling `start_session`.** This keeps the suppression window honest even if `start_session` takes a long time; on `start_session` failure the run row is patched to `terminal_state = 'failed'` via the `mark_run_failed` helper. The pre-decision's "insert after returning sid" ordering would let a slow supervisor leak overlapping fires.
  - **Lifecycle watcher uses `subscribe_events_with_replay`** rather than plain `subscribe_events`. The supervisor's per-session broadcast may have already emitted `Exited` for a fast-finishing session by the time the watcher attaches; the replay buffer (locked surface from Task 23) closes that race. `TurnComplete` resolves to `terminal_state = 'completed'`; `Exited` with `signal.is_some()` or `exit_code != Some(0)` resolves to `'crashed'`; clean `Exited { exit_code: Some(0) | None, signal: None }` resolves to `'completed'`.
  - **`ApiServerActor::with_managers` gained a `scheduler: Option<SchedulerHandle>` arg under `#[cfg(unix)]` — now 8 args.** Pre-decision #18 called this out; documented in the api_server.rs source. `#[allow(clippy::too_many_arguments)]` was added on the constructor (the lint was on the older 7-arg signature in clippy's eyes; the new arg pushes us over the lint threshold).
  - **`docs/interfaces/proto.md` ordering: `schedules.proto` lands between `runtime.proto` and `sessions.proto` per the alphabetical sort in `scripts/regen-interfaces.sh`.** This is the same convention every other proto follows; no manual reorder.
- **Open questions for next task:**
  - **Task 39 (Skills Registry)** should consider whether the V1.0 cron surface (Task 38 §"Scope — out") reuses the `schedules.kind` discriminator or splits into a sibling table. The current `CHECK (kind = 'loop')` is intentionally narrow; widening it is a one-line migration. The proto's `Schedule.kind` is `string` (not enum) precisely so the V1.0 `'scheduled_task'` value lands without a wire break.
  - **Task 40 (Suggestion Rule Engine)** may want to consume `schedule_runs` history to surface "this loop has been failing for 6 hours" suggestions. The read surface already exists (`schedule_runs::list_by_schedule`); no new persistence work needed.
  - **V1.0 promote (loop → scheduled_task)** will need a new `schedules.kind` value plus a constructor on the handle. The current `SchedulerHandle::create_schedule` validates `kind == "loop"` early; the promote path can rebuild the row via the future write helper without touching the V0.1 surface.
- **Deliberate debt:** persistent scheduled tasks, cron parsing, cloud-task sync, promote loop→scheduled, budget guardrails, jittered firing, per-account daily caps, `wait_for_check_runs`, and the 6 PRD §12.4 starter templates — all V1.0. No `TODO` / `FIXME` / `todo!()` / `unimplemented!()` markers in new code.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still exits 0 with "Smoke gate v2: PASSED" — the new Scheduler actor is wired but the smoke gate doesn't currently exercise `Schedules.CreateSchedule`; Task 27's locked path (create project → repo → workspace → workarea → spawn echo session → assert output → archive workarea) is unaffected.
