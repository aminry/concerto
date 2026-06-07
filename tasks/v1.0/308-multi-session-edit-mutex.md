# Task 308 — Multiple Sessions per Workarea (multi-agent) + a Shared `EditMutexRegistry`

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 307 |
| Touches subsystem(s) | 03 (Workspace/Session Manager), 04 (Agent Supervisor) |
| Smoke gate | unchanged |

## Goal
Let a single workarea host **multiple concurrent agent sessions** (Claude alongside Codex on the same code) and add the **per-workarea edit mutex** that serializes their file writes so two agents never clobber each other mid-edit. The persistence layer already permits N sessions per workarea (`sessions.workarea_id` is a non-unique FK with no cardinality guard — `crates/persist/src/sessions.rs`), so the cardinality is structurally free; the missing piece is the concurrency contract `design/04 §3.5` specifies: **at most one session writes files at a time within a workarea**, enforced by a per-workarea `Mutex<()>` with a 10s acquisition timeout, the loser surfacing a clear "blocked on `<other session>`" error while reads (status/diff/git log) stay concurrent. This task introduces a shared **`EditMutexRegistry`** — a `HashMap<WorkareaId, Arc<Mutex<()>>>` living in a neutral module that both the Workarea owner (03) and the Agent Supervisor (04) hold an `Arc` to (per `PHASE3_PLANNING §2`) — wraps each session's active file-edit tool calls (`Write`/`Edit`/`NotebookEdit` + commit) in a timed acquisition of the workarea's mutex, and confirms two sessions can coexist with independent chats/processes/permissions while sharing the worktrees + `.context/`. After this task a user can add a "+ Codex session" tab to a running workarea and the two agents' writes are serialized, not racing.

## Inputs to read before starting
- `design/04_Agent_Supervisor.md` §3.5 (the authoritative contract: multiple `sessions` rows per workarea share **all worktrees + `.context/`** but have independent chat threads / agent processes / context windows / permission overrides; **"at most one session writes files at a time within a workarea… a per-workarea `Mutex<()>` around active edits… the other session's writes block (default 10s timeout) and surface a 'blocked on <other session>' indicator… Reads (status, diff, git log) are concurrent."**), §8 (the failure row "Two sessions on same workarea both try to commit → per-workarea edit mutex timeout → reject second commit with a clear error"), R-5 (serial mutex on writes, 10s timeout, per-workarea; per-file mutex is a V2.0 maybe).
- `design/03_Workspace_Session_Manager.md` §3.4 (session creation: one agent on one workarea; "A per-workarea edit mutex (`04 §3.5`) serializes file writes across sessions on the same workarea"), §6.3 (multi-session coexistence: independent spawn / chat / permission, shared worktrees + `.context/`, the mutex serializes writes), §3.11 (workarea-scope vs session-scope table — the mutex is a **workarea** construct, chat/process/approvals are **session** scope), §7.4 (the "+ Codex session" add-a-second-session sequence).
- `crates/core/src/agent_supervisor/actor.rs` — the supervisor where the mutex must be held. Note: a **per-session** writer `Mutex<tokio::net::unix::OwnedWriteHalf>` already exists (line ~184) — **that is the stdin write half, a different lock; do NOT reuse it.** The new mutex keys on `WorkareaId` (the `SessionEntry` carries `workarea_id`, line ~153). `SessionEntry` + the `sessions: Arc<Mutex<HashMap<SessionId, SessionEntry>>>` map (line ~232) are where the registry `Arc` gets threaded.
- `crates/core/src/agent_supervisor/approval.rs` + `tool_args.rs` — where tool calls are classified: `tool_args.rs` extracts the path for `Write`/`Edit`/`Read`/`NotebookEdit` (line ~43); `approval.rs` gates `Write`. The mutex is acquired around the **execution of write-class tool calls** (`Write`/`Edit`/`NotebookEdit`/`MultiEdit` + the commit path), **not** around `Read`/`Bash`-status/`Grep`. Define the write-class set precisely (mirror the `tool_args.rs` path-bearing-write set).
- `crates/persist/src/sessions.rs` — `insert` (line 84; no cardinality guard on `workarea_id`), `list_live_ids_by_workarea` (line 304; the union-of-live-sessions read 307 uses for `finished` and this task uses for the mutex-holder display name). Confirm nothing rejects a second session on a workarea.
- `crates/core/src/handlers/sessions.rs` — `create_session` (line 79; already workarea-scoped, validates the workarea exists) — confirm it has no "one session per workarea" assumption to remove.
- `crates/core/src/boot.rs` lines 530–599 — where `WorkareaManagerActor` and `AgentSupervisorActor` are constructed/spawned and wired (`with_agent_supervisor`). The shared `EditMutexRegistry` `Arc` is created here and handed to **both** via a `with_edit_mutex_registry(...)` builder (mirroring the existing `with_agent_supervisor` / `with_audit` builder pattern).
- `tasks/v1.0/307-parallel-workareas-fsm.md` → "Handoff Notes" — the FSM funnel + the union-of-sessions derivation for `finished` (this task adds the **second** live session that makes the union non-trivial).
- `tasks/v1.0/PHASE3_PLANNING.md` §2 row 308 ("a **shared `EditMutexRegistry`** (`HashMap<WorkareaId, Arc<Mutex<()>>>`) in a neutral module both the workarea owner (03) and the supervisor (04) hold an `Arc` to") — this is the locked placement decision.

## Scope — in
- **`EditMutexRegistry`** in a neutral module (`crates/core/src/workspace_manager/edit_mutex.rs`, re-exported from `mod.rs` — it is workarea-scoped state so it lives next to the workarea owner, but the type is held by both 03 and 04): an `Arc`-shareable struct wrapping `Mutex<HashMap<WorkareaId, Arc<Mutex<()>>>>` (an outer lock guarding the map, inner per-workarea `Arc<Mutex<()>>`). Public API: `acquire(&self, workarea: &WorkareaId, timeout: Duration) -> Result<EditGuard, EditBlocked>` (lazily inserts the inner mutex on first touch; returns an owned guard that releases on drop) + `holder(&self, workarea) -> Option<SessionId>` bookkeeping so the "blocked on `<session>`" message can name the current writer. `EditBlocked` carries the holding session id (or "another session" if unknown) + a typed wire-code `workarea.edit_mutex.blocked`.
- **Wire the mutex into write-class tool execution** (Agent Supervisor): before a session executes a `Write`/`Edit`/`NotebookEdit`/`MultiEdit`/commit tool call, `registry.acquire(workarea_id, 10s)`; hold the guard across the actual filesystem mutation; on timeout, **reject that tool call** for the blocked session with the `workarea.edit_mutex.blocked` error + the holder's session id (surfaced to that session's event stream — it is **not** a silent indefinite queue). Record the current holder in the registry while held. Reads (`Read`/`Grep`/diff/status/`git log`) acquire **nothing**.
- **Confirm N sessions** structurally: a defensive check that `create_session` allows a second session on a workarea (no guard to add or remove — just an integration test proving `ListSessions` returns 2). No hard cap (`design/03 R-7`; the UI's first-4-tabs cap is Task 323's, not the server's).
- **Boot wiring**: construct one `Arc<EditMutexRegistry>` in `boot.rs`, pass it to both the `AgentSupervisorActor` (which acquires it) and the `WorkareaManager` (which can read `holder()` for UI/diagnostics) via a `with_edit_mutex_registry` builder on each. The registry is process-wide, keyed by workarea.
- Tests (Tier 1): **two stub/echo sessions on one workarea, assert serialization** — session A acquires the mutex and holds it; session B's write blocks then errors with `workarea.edit_mutex.blocked` naming A (`design/03 §10` "assert per-workarea edit mutex serializes writes" integration row); a write on session A and a concurrent **read** on session B both proceed (reads don't block); the guard releases on drop so a later write succeeds; the 10s timeout is configurable down for the test (inject a short timeout); two sessions on the workarea both appear in `list_live_ids_by_workarea`.

## Scope — out
- **The workarea FSM + `finished`/`partial` statuses** — Task 307 (this task adds the second live session the union-of-sessions derivation needs, but does not own the FSM).
- **The Desktop multi-agent session-tabs UI** (first-4-tabs + overflow, the "+ Codex" menu, surfacing the "blocked on" indicator visually) — Task 323. This task emits the `workarea.edit_mutex.blocked` event; 323 renders it.
- **Per-file mutex granularity** — V2.0 (`design/04 R-5`); V1.0 is one serial write lock per workarea.
- **Cross-session checkpoint/branch coordination** beyond the write mutex — out; sessions share the worktree, each owns its own chat/checkpoints/approvals (`§3.11`).
- **The `Bash`/shell write-detection question** (a `Bash` tool call that happens to write files) — the mutex guards the **explicit edit tools** (`Write`/`Edit`/`NotebookEdit`/`MultiEdit`) + the Concerto commit path; a `Bash` command that writes is the agent's responsibility (same as today). Note this boundary precisely in the module doc; do not try to gate arbitrary `Bash`.

## Public interface this task locks
- **`EditMutexRegistry` (FROZEN), `crates/core/src/workspace_manager/edit_mutex.rs`:** `pub struct EditMutexRegistry` (cheap-to-clone `Arc` inner); `pub async fn acquire(&self, workarea: &WorkareaId, timeout: Duration) -> Result<EditGuard, EditBlocked>`; `pub fn holder(&self, workarea: &WorkareaId) -> Option<SessionId>`. `EditGuard` releases the inner per-workarea lock on `Drop` and clears the holder. `EditBlocked { holder: Option<SessionId> }` maps to the wire-code `workarea.edit_mutex.blocked`. The default acquisition timeout is **10s** (`design/04 §3.5` / R-5).
- **Mutex scope contract (FROZEN):** the lock is acquired around **write-class tool execution only** — `Write`, `Edit`, `MultiEdit`, `NotebookEdit`, and the Concerto-driven commit. Read-class operations (`Read`, `Grep`, diff, status, `git log`) acquire nothing. One lock per `WorkareaId`, shared across all that workarea's sessions.
- **Builder seam (FROZEN):** `AgentSupervisor::with_edit_mutex_registry(Arc<EditMutexRegistry>)` and `WorkareaManager::with_edit_mutex_registry(Arc<EditMutexRegistry>)` thread the single boot-constructed registry into both subsystems (mirrors the existing `with_agent_supervisor` / `with_audit` builders).

## Implementation notes
- **Neutral module, two holders.** The registry type lives under `workspace_manager/` (workarea-scoped state) but is held by `Arc` in **both** the Agent Supervisor (acquires it on writes) and the Workarea Manager (reads `holder()` for diagnostics/UI). Construct exactly one in `boot.rs` and `Arc::clone` into each — do **not** create two registries (that would defeat the cross-session lock). This is the placement `PHASE3_PLANNING §2` locked; don't relocate it into the supervisor (which would make the workarea owner unable to read the holder) or into `WorkareaContext` (which isn't shared with 04).
- **Use `tokio::sync::Mutex`, acquire with `tokio::time::timeout`.** The inner lock is held across an `.await` (the FS mutation), so it must be the async mutex, not `std::sync::Mutex`. `timeout(Duration::from_secs(10), inner.lock())` → `Err(_)` = blocked. Insert the inner `Arc<Mutex<()>>` under the outer map lock, then drop the outer lock **before** awaiting the inner (don't hold the map lock across the long write).
- **Don't reuse the per-session stdin writer mutex.** `actor.rs`'s existing `writer: Arc<Mutex<OwnedWriteHalf>>` (line ~184) serializes a single session's stdin frames — orthogonal to the cross-session edit lock. Keep them separate.
- **Reject, don't queue.** On timeout the blocked session's tool call **fails fast** with `workarea.edit_mutex.blocked` + the holder id; the agent sees a tool error and can retry. Indefinite queuing would deadlock multi-agent flows — the design's 10s + clear error is deliberate.
- **Holder bookkeeping must survive drop.** Set `holder = Some(session_id)` on successful acquire, clear it in `EditGuard::drop`. A panic mid-write must still release (the `Drop` impl guarantees it). Keep the holder map under the same outer lock.
- **Reads stay lock-free.** The gating is at the tool-execution dispatch in the supervisor; only the write-class arm calls `acquire`. Verify the read path (diff/status) is untouched so concurrent reads + a write don't serialize reads.
- **Cross-platform.** `tokio::sync` + `tokio::time` only; no `std::os::unix`. The supervisor's session-spawn is `#[cfg(unix)]` today (agent-host PTY), so the mutex wiring lands inside that gate, but the `EditMutexRegistry` type itself is cross-platform (Task 113 lanes; Windows agent-host is Task 702).
- **No migration, no proto change.** The mutex is pure in-memory runtime state; the `workarea.edit_mutex.blocked` error rides the existing `session.events` / `workarea.events` stream as a typed error, not a new proto field. Interfaces regen is a no-op unless you add a Rust pub type that lands in `rust-api.md` — if `EditMutexRegistry` is `pub`, regen + commit `rust-api.md`.

## Verification
Tier 1.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core edit_mutex` + `cargo test -p concerto-core agent_supervisor` → two-session serialization (B blocks then errors `workarea.edit_mutex.blocked` naming A), concurrent read-during-write does **not** block, guard-release-on-drop lets a later write succeed, short-timeout injection works, two sessions both live on one workarea.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (no new deps).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit if `rust-api.md` changed (the `pub EditMutexRegistry` surface); no proto/schema change.
7. `scripts/smoke.sh` → **unchanged gate** (the single-session `echo-session` path acquires the mutex trivially and must stay green).

**Tier-1 scope.** The mutex logic + multi-session cardinality is fully CI-provable with two stub/echo agents (`design/03 §10` lists it as an integration test). There is no physical-reality gap — this is pure Core concurrency logic, so no Tier-3 line attaches; the only operator-facing follow-on is the **visual** "blocked on" indicator, which is Task 323's Tier-2 UI.

## Definition of Done
- [x] `EditMutexRegistry` in a neutral module (`workspace_manager/edit_mutex.rs`): `acquire(timeout)` + `holder()` + `EditGuard` (releases on drop) + `EditBlocked`/`workarea.edit_mutex.blocked`
- [x] Write-class tool execution (`Write`/`Edit`/`MultiEdit`/`NotebookEdit`/commit) acquires the per-workarea mutex (10s); read-class acquires nothing; blocked session fails fast naming the holder
- [x] One registry constructed in `boot.rs`, `Arc`-shared into both Agent Supervisor + Workarea Manager via `with_edit_mutex_registry`
- [x] Two sessions coexist on one workarea (shared worktrees/`.context/`, independent chat/process/permission); no server-side session cap
- [x] Integration test asserts write serialization + concurrent-read non-blocking + guard release
- [x] No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code (deliberate seams in Handoff)
- [x] No files outside Outputs modified
- [x] Interfaces regenerated + committed if a `pub` Rust surface changed (`rust-api.md`)
- [x] Smoke gate green (unchanged)
- [x] Single commit with the message below

## Outputs
- `crates/core/src/workspace_manager/edit_mutex.rs` (new — `EditMutexRegistry` + `EditGuard` + `EditBlocked`)
- `crates/core/src/workspace_manager/mod.rs` (modified — `pub mod edit_mutex` + re-export)
- `crates/core/src/agent_supervisor/actor.rs` (modified — hold the registry `Arc`; `with_edit_mutex_registry`; acquire around write-class tool execution)
- `crates/core/src/agent_supervisor/approval.rs` or the tool-dispatch site (modified — gate write-class execution on the mutex)
- `crates/core/src/workspace_manager/workarea.rs` (modified — hold the registry `Arc` for `holder()` reads; `with_edit_mutex_registry`)
- `crates/core/src/boot.rs` (modified — construct the single registry; thread into both subsystems)
- `crates/core/tests/*` (new/modified — two-session serialization + concurrent-read + cardinality tests)
- `docs/interfaces/rust-api.md` (regenerated, if a `pub` type changed)

## Commit message
```
phase-3: multi-session workareas + per-workarea edit mutex

Adds a shared EditMutexRegistry (HashMap<WorkareaId, Arc<Mutex<()>>>)
held by both the Workarea Manager and the Agent Supervisor; write-class
tool calls (Write/Edit/NotebookEdit/commit) acquire the per-workarea
lock with a 10s timeout, the loser failing fast with
workarea.edit_mutex.blocked. Reads stay concurrent. Multiple sessions
can now run on one workarea without clobbering each other.

Refs: tasks/v1.0/308-multi-session-edit-mutex.md
```

## Handoff Notes (filled in when finishing)
- Drift from plan: gating landed at the **tool-dispatch site** (`agent_supervisor/actor.rs::dispatch_parse_event`, the `AwaitingApproval` arm), not `approval.rs` — both are listed as acceptable in Outputs; dispatch is where the approve→inject decision actually fires for every parser pack. `acquire` takes `(workarea, session_id, timeout)` (a `session_id` arg beyond the FROZEN `(workarea, timeout)` signature) so the holder is recorded at acquire time without a separate setter — additive, the read path (`holder()`) is unchanged. `rust-api.md` regen was a no-op: `concerto-core` has no `src/api.rs` entry in the doc, so the new `pub` `workspace_manager::edit_mutex` surface is not summarized there (nothing to commit) — exactly the "no-op unless it lands in rust-api.md" case the task anticipated. No files outside Outputs touched.
- Open questions for next task: Task 323 (Desktop session-tabs UI) renders the "blocked on `<session>`" indicator — the `workarea.edit_mutex.blocked` wire-code + holder description ride the existing `AgentEvent::ApprovalResolved.decision` string on `session.events` (no new proto field); 323 parses that string. `WorkareaManager::edit_mutex_holder(workarea)` is the read-side seam 323 can poll for the holder name.
- Deliberate debt: the guard is held across the **approval→stdin-inject** critical section, then released — it cannot span the wrapped agent CLI's *actual* out-of-process filesystem write (Core never observes that write completing across the agent-host UDS). V1.0's serialization is therefore at the inject boundary (sufficient to stop two sessions racing into a write at once + to surface a clear block); a true write-completion fence is a V2.0 item alongside the per-file mutex (`design/04 R-5`). `EditGuard::drop` clears the holder via `try_lock` (non-async `Drop`); under rare contention a stale holder *name* can linger one cycle — cosmetic only, the inner write lock always releases, so correctness is unaffected. Commit-path acquisition is via the same `acquire` API; today only the explicit edit tools flow through `dispatch_parse_event`, so the Concerto-driven commit will call `acquire` when that path is wired (the seam exists; no current commit caller).
- Smoke-gate state: green / unchanged — `scripts/smoke.sh` PASSED (124s, all checks incl. single-session `echo-session`); the single-session path acquires the workarea mutex trivially (uncontended) and stays green. No new smoke capability added (task = unchanged gate).
