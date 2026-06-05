# Task 318 — Scheduler `wait_for_check_runs` Primitive (Poll + Exponential Backoff + Optional Webhook Fast-Path)

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 1 |
| Size | small (≤4h) |
| Depends on | 315 |
| Touches subsystem(s) | 05 (Scheduler), 13 (VCS Provider Integration — consumer of check-runs) |
| Smoke gate | unchanged |

## Goal
Add the `SchedulerHandle::wait_for_check_runs(repo, sha, timeout)` primitive (`design/05 §3.9`, §5.1) — the gate that task 320's coordinated PR-set merge blocks on between merging one member and merging the next. Today the Scheduler has no check-runs awareness; `crates/core/src/scheduler/actor.rs` runs a `/loop` fire wheel and nothing else. This task gives `SchedulerHandle` a poll loop that calls the VCS check-runs source for a commit SHA with a **FROZEN exponential backoff (1s, 2s, 4s, 8s, 16s, 30s-cap)** and resolves to a `ChecksOutcome` when **every check in the required set reaches a terminal conclusion** (`success`/`failure`/`cancelled`) or the wall-clock `timeout` elapses. The **required set is a caller parameter** (320 supplies it), **defaulting to "all check-runs for the SHA reach a terminal conclusion"** (PHASE3_PLANNING §2 — no GitHub branch-protection API read in V1.0). When 315's webhook receiver is wired and provides updates, the loop **prefers a webhook wake over the next poll**; when no webhook is available (the Tier-1 CI path), it degrades to pure polling and is fully provable against a stubbed check-runs source. This lands in Phase 3 — not Phase 6 with the rest of the Scheduler thickening — **because 320 needs it** (README §6 note).

## Inputs to read before starting
- `design/05_Scheduler.md` §3.9 — the authoritative spec: (1) poll loop against VCS (13) for the SHA's check runs; (2) **exponential backoff (1s, 2s, 4s, 8s, 16s, 30s — capped)**; (3) subscribe to webhook updates if the VCS provides them (preferred over polling); (4) resolve when all **required** checks are conclusive (`success`/`failure`/`cancelled`) **or** timeout. §5.1 — the FROZEN signature `pub async fn wait_for_check_runs(&self, repo: RepositoryId, sha: &str, timeout: Duration) -> Result<ChecksOutcome>`. §7.4 — the consumed-by-03 sequence diagram (the `par poll / and webhook` interleave). §8 — `wait_for_check_runs timeout` row: **resolve with a `Timeout` outcome; the caller (03) decides what to do** (do NOT error on timeout — return the outcome).
- `design/13_VCS_Provider_Integration.md` §3.3 — the polling cadence: "Check runs — poll with exponential backoff while waiting (1s, 2s, 4s, 8s, 16s, 30s cap) — **same as 05 §3.9**." The two backoff sequences MUST be byte-identical; this task FREEZES the constant shared with 13. §6.2 (webhook flow — the source of the optional fast-path wake).
- `tasks/v1.0/PHASE3_PLANNING.md` §2 (task 318 row): **"The required set is a caller parameter (`320` supplies it), defaulting to 'all check-runs for the SHA reach a terminal conclusion.' No branch-protection API read in V1.0."** §6 (refined deps — 318's webhook fast-path consumes 315; the poll path does not).
- `crates/core/src/scheduler/actor.rs` — the `SchedulerHandle` to extend: it already holds `Arc<Persistence>`, the next-fire `wheel`, the `inflight` map, and the `notify`. The new method does **not** touch the wheel or inflight state — it is a standalone awaitable. Add an `Option<VcsHandle>` (or an `Option<Box<dyn CheckRunsSource>>` test seam — see Implementation notes) so the poll loop can call the VCS check-runs source; wire it in `boot.rs`. Read `rebuild_wheel`/`fire_schedule` only to match the actor's locking/clone conventions — do **not** entangle the new method with the fire loop.
- `crates/core/src/vcs/actor.rs` `VcsHandle::get_check_runs(repository_id, sha) -> Result<Vec<gh_cli::CheckRun>>` + `crates/core/src/vcs/gh_cli.rs` `CheckRun { name, status, conclusion, details_url }` (`status ∈ queued|in_progress|completed`; `conclusion` set only when `completed`). This is the existing source the poll loop calls. **NOTE:** 313 relocates `VcsHandle` into `crates/vcs`; consume whatever the boot path hands the Scheduler (read 313's Handoff when it exists for the final type path).
- `crates/core/src/boot.rs` — lines ~607 (the Scheduler is constructed via `SchedulerActor::new(persistence, supervisor)`) and ~726 (`VcsProviderActor::new` → `vcs_handle`). **Critical ordering fact:** the Scheduler is built at 607, the `vcs_handle` only at 726 — the vcs handle does not exist yet when the Scheduler is constructed. Wire the dependency with a **post-construction setter** (`SchedulerHandle::set_check_runs_source(...)` / `with_check_runs_source(...)` called after line 726) rather than reordering the whole boot sequence; note the chosen approach in Handoff.
- `tasks/v1.0/315-webhook-receiver.md` → "Handoff Notes" (when it exists) — the `checks.<workarea>.<repo>` event subject + the subscription handle the optional webhook fast-path subscribes to. **315 is the declared dependency, but the webhook subscription MUST be optional** so 318 works poll-only when no webhook is wired (the Tier-1 path). If 315's surface differs, follow its Handoff.

## Scope — in
- `SchedulerHandle::wait_for_check_runs(&self, repo: RepositoryId, sha: &str, timeout: Duration) -> Result<ChecksOutcome>` per the FROZEN `design/05 §5.1` signature, **plus** a `required: RequiredChecks` parameter (the caller-supplied required set per PHASE3_PLANNING §2) — see Public interface for how the signature carries it.
- The poll loop: call the check-runs source for `sha`; classify each run terminal/pending; resolve when every run in the required set is terminal; sleep the next backoff step `[1s, 2s, 4s, 8s, 16s, 30s, 30s, …]` between polls; honor the wall-clock `timeout` as a `tokio::time::timeout`-style deadline that resolves to `ChecksOutcome { passed: <all-success>, timed_out: true, runs }` (NOT an `Err`).
- The **required-set default**: `RequiredChecks::AllTerminal` (the default 320 uses) = "all check-runs returned for the SHA must reach a terminal conclusion." `RequiredChecks::Named(Vec<String>)` lets 320 (or a future caller) restrict to a named subset. No branch-protection API read.
- The optional webhook fast-path: if a `checks.<workarea>.<repo>` subscription is available, `select!` between the backoff sleep and a webhook wake so a `check_run.completed` event short-circuits the sleep and triggers an immediate re-poll. Absent a subscription, fall back to pure backoff sleeps. The webhook side is a **wake hint only** — the authoritative state always comes from a re-poll (the webhook payload is opaque per the streams contract).
- The `Option<...check-runs source...>` dependency on `SchedulerHandle` + its `boot.rs` wiring (post-construction setter), keeping the existing `SchedulerHandle::new` constructor's V0.1 signature back-compat (add the source via a setter or a defaulted field, not a breaking constructor change).
- Tests (Tier 1, against a stub check-runs source — no real network): a SHA whose runs go pending→pending→all-success resolves `passed: true`; a run that ends `failure` resolves `passed: false, timed_out: false`; a SHA that never terminates hits `timeout` and resolves `timed_out: true` (drive synthetic time with `tokio::time::pause`/`advance` so the test is instant); the backoff sequence is asserted to be exactly `[1,2,4,8,16,30,30,…]` (capped); `RequiredChecks::Named` ignores non-required pending runs and resolves on the named subset; a webhook wake (a fed event on the in-process subscription) short-circuits a long backoff sleep and triggers an early resolve.

## Scope — out
- The **coordinated merge loop** that calls this (`merge_workarea_pr_set` → `wait_for_check_runs` → continue/pause) — **task 320** (rust, Tier-2). 318 is the primitive only.
- The rest of the **Scheduler thickening** (cron parse + jitter + run history, budget guardrails, promote-loop, cloud sync, templates) — **tasks 609–613 (Phase 6)**. ONLY `wait_for_check_runs` lands in P3 (README §6).
- The **webhook receiver itself** (the relay route + HMAC verify + delivery-id idempotency + the `checks.<wa>.<repo>` emit) — **task 315**. 318 only *subscribes* to the event subject 315 emits, and only optionally.
- Branch-protection / required-checks discovery via the GitHub API — explicitly **not V1.0** (PHASE3_PLANNING §2); the required set is a caller parameter.
- Any new proto/RPC — `wait_for_check_runs` is an **internal Rust handle method** consumed in-process by 320 (on the `Workareas` service), not a standalone RPC. (`design/05 §5.2` says it "mirrors in the Schedules service," but its only V1.0 caller is 320 in-process; no `schedules.proto` change.)

## Public interface this task locks
- **Rust signature (FROZEN, `design/05 §5.1`):** `impl SchedulerHandle { pub async fn wait_for_check_runs(&self, repo: RepositoryId, sha: &str, timeout: Duration, required: RequiredChecks) -> Result<ChecksOutcome> }`. The design's literal signature has no `required` arg; this task ADDS it (PHASE3_PLANNING §2 made the required set a caller parameter) and FREEZES the four-arg form. 320 calls it with `RequiredChecks::AllTerminal`.
- **`RequiredChecks` (FROZEN):** `pub enum RequiredChecks { AllTerminal, Named(Vec<String>) }`. `AllTerminal` is the default 320 uses.
- **`ChecksOutcome` (FROZEN):** `pub struct ChecksOutcome { pub passed: bool, pub timed_out: bool, pub runs: Vec<CheckRunSnapshot> }` where `passed = required set all reached a non-failure terminal conclusion` (a timeout always yields `timed_out: true`, and `passed` reflects the last observed state). `CheckRunSnapshot { name: String, status: String, conclusion: String }` is a transport-free snapshot of the source `CheckRun` (do not leak the gh_cli type across the Scheduler boundary).
- **Backoff constant (FROZEN, shared with `design/13 §3.3`):** `pub const CHECK_RUN_BACKOFF_SECS: [u64; 6] = [1, 2, 4, 8, 16, 30];` with the last value (`30`) repeated as the cap for all subsequent polls. This is the SAME sequence task 314 reuses for its degraded cadence and 13 cites — keep them identical; if 314 has already pinned it, reuse its constant rather than duplicating (note which in Handoff).
- **Check-runs source seam (FROZEN):** `pub trait CheckRunsSource: Send + Sync { async fn check_runs(&self, repo: &RepositoryId, sha: &str) -> Result<Vec<CheckRunSnapshot>>; }`, implemented by `VcsHandle` (the production source) and by a test stub (the Tier-1 double). The Scheduler holds an `Option<Arc<dyn CheckRunsSource>>`; absent it, `wait_for_check_runs` returns a typed `scheduler.no_vcs_source` error.

## Implementation notes
- **Synthetic time is the whole game for Tier-1.** Use `tokio::time::sleep` for the backoff so the test can `tokio::time::pause()` + `advance(Duration)` and drive a 10-minute timeout instantly. Never `std::thread::sleep`. The poll loop must be a plain `async fn` awaitable — do NOT route it through the fire wheel or `notify`; it is independent of the `/loop` machinery.
- **Inject the source as a trait, not the concrete `VcsHandle`.** The `CheckRunsSource` trait keeps the Tier-1 test a pure in-process stub (a `Vec<Vec<CheckRunSnapshot>>` script that returns successive poll results) with zero network and no `wiremock`. `VcsHandle` implements `CheckRunsSource` by delegating to `get_check_runs` + mapping `gh_cli::CheckRun` → `CheckRunSnapshot`. This also sidesteps the 313 crate-relocation churn — the Scheduler depends on the trait, not on where `VcsHandle` lives.
- **Terminal classification.** A run is terminal iff `status == "completed"` (its `conclusion` is then one of `success|failure|neutral|cancelled|timed_out|action_required|stale|skipped`). `passed` for the required set = every required run is `completed` AND none has a failing conclusion (`failure|cancelled|timed_out|action_required` count as not-passed; `success|neutral|skipped|stale` count as passed — match `design/05 §3.9`'s "success / failure / cancelled" terminal set and treat the GitHub conclusion vocabulary conservatively). Document the exact pass/fail mapping in the function doc-comment and FREEZE it.
- **The webhook fast-path is optional + advisory.** `select!` over `(backoff_sleep, webhook_recv)`. A webhook wake never carries authoritative state — it just cancels the current sleep so the loop re-polls immediately. If the subscription is `None`, the `select!` collapses to the sleep arm. This keeps the Tier-1 path (no 315) fully functional and provable.
- **Boot wiring without reordering.** Because the `vcs_handle` is built after the Scheduler in `boot.rs`, add `SchedulerHandle::set_check_runs_source(Arc<dyn CheckRunsSource>)` (interior-mutable via an `Arc<OnceCell<...>>` field, mirroring the existing `gh_path` `OnceCell` pattern in `VcsHandle`) and call it after line ~726. Keep `SchedulerHandle::new`'s V0.1 signature unchanged so no existing call site breaks.
- **Do not block the fire loop.** `wait_for_check_runs` is awaited by 320's merge loop on its own task; it must never hold the wheel/inflight locks or starve the fire loop. It only touches its injected source + sleeps.

## Verification
**Tier 1.** Fully CI-self-verifiable against an in-process `CheckRunsSource` stub + synthetic time; no real network, no `wiremock`.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core wait_for_check_runs` → all-success resolves `passed:true`; a `failure` run resolves `passed:false`; never-terminating resolves `timed_out:true` (under `tokio::time::pause`); the backoff sequence equals `[1,2,4,8,16,30,30,…]`; `RequiredChecks::Named` resolves on the named subset; a fed webhook wake short-circuits a long sleep.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (no new dependency — `tokio` time + the existing types only).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → the new public Rust types (`ChecksOutcome`, `RequiredChecks`, `CheckRunsSource`, `CheckRunSnapshot`, `CHECK_RUN_BACKOFF_SECS`) appear in `docs/interfaces/rust-api.md`; commit the regen. **No proto change** (this is an internal Rust primitive — no `schedules.proto` edit).
7. `scripts/smoke.sh` → **unchanged** (no smoke capability; the real coordinated merge that exercises this is 320 + the Tier-3 checklist).

## Definition of Done
- [ ] `SchedulerHandle::wait_for_check_runs(repo, sha, timeout, required)` implemented per the FROZEN `design/05 §5.1` signature (+ the `required` caller-param)
- [ ] FROZEN exponential backoff `[1,2,4,8,16,30]`-cap shared identically with `design/13 §3.3` (reused from 314's constant if present)
- [ ] `RequiredChecks::{AllTerminal, Named}` (default `AllTerminal`); resolves on the required set terminal or wall-clock timeout (timeout → `timed_out:true`, never `Err`)
- [ ] `CheckRunsSource` trait injected (impl'd by `VcsHandle`); Tier-1 tests use an in-process stub + `tokio::time::pause`
- [ ] Optional `checks.<wa>.<repo>` webhook wake short-circuits a backoff sleep; absent it, pure-polling still works (Tier-1)
- [ ] `boot.rs` wires the source via a post-construction setter (no boot reordering); `SchedulerHandle::new` V0.1 signature unchanged
- [ ] All `rust` §5.3 commands pass; interfaces regenerated (rust-api.md); no proto change
- [ ] No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code (deliberate seams in Handoff)
- [ ] Single commit with the message below

## Outputs
- `crates/core/src/scheduler/wait_checks.rs` (new — `wait_for_check_runs`, `ChecksOutcome`, `RequiredChecks`, `CheckRunsSource`, `CheckRunSnapshot`, `CHECK_RUN_BACKOFF_SECS`)
- `crates/core/src/scheduler/actor.rs` (modified — `Option<Arc<dyn CheckRunsSource>>` field + `set_check_runs_source`; expose `wait_for_check_runs` on `SchedulerHandle`)
- `crates/core/src/scheduler/mod.rs` (modified — `pub mod wait_checks` + re-exports)
- `crates/core/src/vcs/actor.rs` (modified — `impl CheckRunsSource for VcsHandle` delegating to `get_check_runs`)
- `crates/core/src/boot.rs` (modified — call `scheduler_handle.set_check_runs_source(...)` after the vcs handle is built)
- `crates/core/tests/wait_for_check_runs.rs` (new — Tier-1 stub-source + synthetic-time tests)
- `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-3: scheduler wait_for_check_runs primitive (poll + backoff + webhook)

SchedulerHandle::wait_for_check_runs(repo, sha, timeout, required) polls a
CheckRunsSource with the frozen [1,2,4,8,16,30]-cap backoff, resolving when
the caller-supplied required set (default AllTerminal) is terminal or on
wall-clock timeout. Optional checks.<wa>.<repo> webhook wake short-circuits
a sleep. Consumed by task 320's coordinated PR-set merge.

Refs: tasks/v1.0/318-wait-for-check-runs.md
```

## Handoff Notes (filled in when finishing)
- Drift from plan: —
- Open questions for next task: —
- Deliberate debt: —
- Smoke-gate state: —
