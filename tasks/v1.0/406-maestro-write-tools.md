# Task 406 — Maestro write tool set (5 `MustAsk`-gated mutation tools behind 401's frozen schemas)

| Field | Value |
|---|---|
| Phase | 4 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 401, 402 |
| Touches subsystem(s) | 08 (Maestro), 03 (Workspace/Workarea/Session Mgr), 04 (Agent Supervisor) |
| Smoke gate | unchanged |

## Goal
Make the Maestro's five **write** tools actually mutate Concerto. Today the Maestro module exists only as 401's skeleton: `crates/core/src/maestro/tools/mod.rs` registers all 16 tool schemas with their FROZEN input/output shapes, and each write tool returns a **typed `unimplemented` MCP error** (the 305 / `UpsertProjectMcp` seam discipline — never `todo!()`, never empty-success). There is no code that turns a Maestro tool call into a real `send_input`/`create_workspace`/`create_workarea`/`transition_workarea`, and there is no path that gates those mutations behind a user confirmation. This task implements the **5 write tools** behind 401's frozen schemas (consumes **§4.1**) in a new `crates/core/src/maestro/tools/write.rs`: `route_prompt_to_session(session_id, prompt)` → `AgentSupervisorHandle::send_input(&SessionId, Vec<u8>)` (`actor.rs:930`); `fanout_to_sessions(session_ids[], prompt)` → the same `send_input` fanned across N sessions; `create_workspace(spec)` → `WorkspaceManager::create_workspace(name, repos, permission_mode, description, icon)` (`actor.rs:185`); `create_workarea(workspace_id, spec)` → `WorkareaManager::create_workarea(workspace_id, permission_mode)` (`workarea.rs:695`); `set_workarea_paused(workarea_id, paused)` → `transition_workarea(id, WorkareaEvent::Pause | WorkareaEvent::Resume)` (`workarea.rs:2610` / `fsm.rs:133/135`). **Each tool classifies as a non-`ReadOnly` `ToolClass`** so that under the Maestro's always-`strict` permission mode the built `PermissionResolver` returns `Decision::MustAsk` (`permission.rs:451`, `(Strict, _) ⇒ MustAsk`) **before** the mutation runs — surfaced as the existing **`AwaitingApproval` / `ResolveApproval` confirmation-chip** flow (carries `urgent` + `destructive_label`, `events.rs:62`), consumes **§4.8**. **No bypass** (`design/08 R-2` / §3.10: every user-visible mutation gets a confirmation chip). After this task, 405 (read tools) + 406 (write tools) + 407 (side-channels) jointly light up the full 16-tool surface behind 401's registry, and **Task 411 (`create_workspace_from_description`) EXTENDS this file's create flow** — it wraps `create_workspace`/`create_workarea` after issue-parse + cone-suggest, so the `create_*` entry points here are authored as the reusable inner functions 411 calls. What stays out: real user-tap-to-confirm UX (the Desktop chip render + tap → 415 / Tier-3), and the LLM that decides *which* tool to call (402's agent loop).

## Inputs to read before starting
- `tasks/v1.0/PHASE4_PLANNING.md` §4.1, §4.8, §2 (rows "405/406/407 tool-file split", "406 write-tool confirmation"), §6 (the 406 dep edge), §8.1 (write-set: `tools/write.rs` + the `tools/mod.rs` registration line) — **AUTHORITATIVE** for D3 (MCP transport), **D4** (`ToolClass::ReadOnly` + strict-matrix → write tools force `MustAsk`), and the lead-owned `tools/mod.rs` seam this obeys.
- `design/08_Maestro_Agent.md` §5.1 — the FROZEN write-tool argument shapes (`route_prompt_to_session(session_id, prompt)`, `fanout_to_sessions([session_ids], prompt)`, `create_workspace(spec) → workspace_id`, `create_workarea(workspace_id, spec) → workarea_id`, `set_workarea_paused(workarea_id, paused: bool)`); §5.1's note "**Write — all require user confirmation chip in the UI before executing**"; §3.10 (R-2: no bypass; inert-on-exhaust does NOT disable routing/tool calls — those stay deterministic).
- `design/04_Agent_Supervisor.md` §3.2 / §3.10 — the permission matrix `400` amended: `strict` + `ReadOnly` ⇒ auto-approve, `strict` + everything-else ⇒ `MustAsk`; the `AwaitingApproval` → `ResolveApproval` chip lifecycle the write tools ride.
- `tasks/v1.0/401-…md` → "Public interface this task locks" + "Handoff Notes" — the FROZEN 16-tool schema registry, the `crates/core/src/maestro/{mod,mcp}.rs` + `tools/mod.rs` skeleton, the typed-`unimplemented`-MCP-error convention each tool returns until its impl lands, and the `rmcp` tool-dispatch shape (how a registered tool's handler is invoked). **Consume the write-tool input/output schemas as frozen by 401 (PHASE4_PLANNING §4.1) — do NOT re-shape them.**
- `tasks/v1.0/402-…md` → "Public interface this task locks" + "Handoff Notes" — the FROZEN `AgentKind::Maestro`, the new **`ToolClass::ReadOnly`** bucket + the amended `(Strict, ReadOnly) ⇒ AutoApprove` matrix arm, and the Maestro-session `strict` convention. **Consume `ToolClass::ReadOnly` + the strict matrix as frozen by 402 (PHASE4_PLANNING §4.8) — this task only assigns the 5 write tools to a non-`ReadOnly` class so they hit `MustAsk`.**
- `crates/core/src/agent_supervisor/actor.rs:930` (`send_input(&self, session_id: &SessionId, data: Vec<u8>) -> Result<()>` — the only send-prompt path), `:368` (`start_session` / `StartSessionRequest` — context for how a `SessionId` is shaped) — the route/fanout target.
- `crates/core/src/workspace_manager/actor.rs:185` (`create_workspace(name: &str, repos: &[WorkspaceRepoSpec], permission_mode: Option<String>, description: Option<String>, icon: Option<String>) -> Result<Workspace>`) + `crates/core/src/workspace_manager/workarea.rs:695` (`create_workarea(workspace_id: &str, permission_mode: Option<String>) -> Result<Workarea>`) + `:2610` (`transition_workarea(id: &WorkareaId, event: WorkareaEvent) -> Result<Workarea>`) + `crates/core/src/workspace_manager/fsm.rs:133/135` (`WorkareaEvent::{Pause, Resume}`; `WorkareaState::Paused`) — the four mutation targets; `transition_workarea` already rejects an illegal `Pause`/`Resume` as a typed `Error::Policy` (FSM precondition).
- `crates/core/src/agent_supervisor/events.rs:62` (`AgentEvent::AwaitingApproval { session_id, approval_id, tool, summary, payload_json, urgent, destructive_label }`) + `crates/core/src/agent_supervisor/approval.rs` (`PendingApprovals`, `user_decision_string`, the `oneshot::Sender<Decision>` park/resolve bookkeeping) + `crates/core/src/security/permission.rs:448` (`PermissionResolver::decide(tool) -> Decision`; `(Strict, _) ⇒ MustAsk`) — the confirmation-chip mechanism the write tools must route through, NOT around.
- `tasks/v1.0/313-vcs-provider-github.md` + `tasks/v1.0/305-cone-stats-suggest-seam.md` — the dense citation-heavy register + the "seam returns a typed Err/Status, never the macro, never empty-success" discipline this file copies.

## Scope — in
- **`crates/core/src/maestro/tools/write.rs` (new):** the impl bodies for the 5 write tools, each behind 401's frozen schema, each routed through the strict confirmation gate before it mutates.
  - **`route_prompt_to_session(session_id, prompt)`** → resolve `session_id` to a `SessionId`, gate (see chip-gate below), then `AgentSupervisorHandle::send_input(&session_id, prompt.into_bytes())`. Return the frozen success shape (per 401's schema). A non-existent/closed session → typed `Err` mapped to the tool's structured error result (NOT a panic, NOT empty-success).
  - **`fanout_to_sessions(session_ids[], prompt)`** → one confirmation gate for the whole fanout (the user confirms "send to N sessions" once, per `design/08 §5.1` — not N separate chips), then `send_input` per target; collect per-session ok/err into the frozen result shape so a partial failure (one closed session) is reported, not swallowed.
  - **`create_workspace(spec)`** → map the frozen `spec` schema onto `WorkspaceManager::create_workspace(name, &repos, permission_mode, description, icon)`; return the new `workspace_id`. **Author the mapping as a reusable inner fn (`do_create_workspace(spec) -> Result<WorkspaceId>`)** so Task 411's `create_workspace_from_description` calls it after issue-parse/cone-suggest without re-implementing the create.
  - **`create_workarea(workspace_id, spec)`** → map onto `WorkareaManager::create_workarea(workspace_id, permission_mode)`; return the new `workarea_id`. Same **reusable-inner-fn** discipline (`do_create_workarea(workspace_id, spec) -> Result<WorkareaId>`) for 411.
  - **`set_workarea_paused(workarea_id, paused)`** → `transition_workarea(&WorkareaId, if paused { WorkareaEvent::Pause } else { WorkareaEvent::Resume })`; surface the FSM's typed `Error::Policy` (illegal transition, e.g. pausing an archived workarea) as the tool's structured error, not the macro.
  - **The chip-gate helper** (one place, used by all 5): given the tool name + a human-readable `summary` + an optional `destructive_label`, ask the `PermissionResolver` under the Maestro session's `strict` mode (`decide(tool) ⇒ MustAsk` for every non-`ReadOnly` tool), then drive the existing `AwaitingApproval` → park-on-`oneshot::Receiver<Decision>` → `ResolveApproval` flow; on `Decision::AutoDeny`/`"deny"` return a typed "user declined" tool result and **do not mutate**; on `approve`/`approve_once` proceed. Reuse `approval.rs`'s `PendingApprovals` + `user_decision_string` — do NOT fork a second approval registry.
  - **Tool classification:** assign the 5 write tools to a non-`ReadOnly` `ToolClass` (Restricted for route/fanout/create; mark `set_workarea_paused` Restricted too — it is reversible, so `destructive_label = None`) so the `(Strict, _) ⇒ MustAsk` arm fires. Do NOT add a new `ToolClass` variant or a new `SecretKind` — `ToolClass::ReadOnly` is 402's; use the existing `Restricted`/`Dangerous`.
- **`crates/core/src/maestro/tools/mod.rs` (modified — ONE line):** wire `pub mod write;` + the single registration call that binds 401's frozen write-tool schemas to this file's handlers (the lead-owned seam; 405/407 add their own one line in a distinct region — additive, auto-merges).
- Tests (Tier 2): the in-process MCP harness (401's `concerto-maestro-mcp` server + a scripted client) + a scripted approval — (1) `route_prompt_to_session` with a *scripted-approve* confirmation calls `send_input` with the exact prompt bytes against a mock/echo session; (2) the *scripted-deny* path returns "user declined" and `send_input` is **never** called; (3) `fanout_to_sessions` over 2 sessions = one gate then 2 `send_input`s, and a closed second session yields a per-session error in the result (not a swallow); (4) `create_workspace`/`create_workarea` (approve) return real ids and a row exists; (5) `set_workarea_paused(true)` drives the workarea to `Paused`, `set_workarea_paused(false)` back, and pausing an archived workarea returns the typed `Error::Policy` as a tool error; (6) **every** write tool under `strict` produces an `AwaitingApproval` before any mutation (assert the gate fired, the §4.8 invariant).

## Scope — out
- **The Maestro LLM agent loop that decides which write tool to call** — owned by **Task 402** (the provider-selection seam + spawn); this task only implements the tool *bodies* the agent invokes. 408's pre-parser routes deterministic `@workarea` directives that also land on `route_prompt_to_session`, but the pre-parser itself is **Task 408**; leave the deterministic-routing call-site as the consumer of this tool.
- **`create_workspace_from_description`** (issue parse → multi-repo detect → cone suggest → confirm chips) — owned by **Task 411**, which **wraps this file's `do_create_workspace`/`do_create_workarea` inner fns** + adds `SuggestCones` + fixes the `enterprise_data_privacy=false` debt. This task leaves the `create_*` entry points as the reusable seam 411 extends (it also writes to `tools/write.rs` — coordinate the create-flow region per §8.1).
- **`notify_user` / `propose_chip` side-channels** — owned by **Task 407** (`tools/side.rs` + the Maestro-owned slate, D11); this task does not touch the side-channel file.
- **The 11 read tools** — owned by **Task 405** (`tools/read.rs`); disjoint file, parallel-safe.
- **The Desktop confirmation-chip render + user-tap UX** — owned by **Task 415** (`apps/desktop`, mocked invoke); this task proves the *gate fires and blocks the mutation*, not the pixels. The real human tap-to-confirm is the **Phase-4 Tier-3 checklist line** "route prompts via `@workarea` and fanout" + "confirm an excluded workarea leaks only hard facts" (operator confirms the chip UX live at the phase gate).

## Public interface this task locks
This task **consumes** frozen contracts and **adds no new public surface** beyond the tool bodies + one registration line. It re-locks nothing.

- **Consumes the 5 write-tool MCP input/output schemas as FROZEN by Task 401 (PHASE4_PLANNING §4.1, `design/08 §5.1`):**
  ```text
  route_prompt_to_session(session_id, prompt)
  fanout_to_sessions([session_ids], prompt)
  create_workspace(spec)                  → workspace_id
  create_workarea(workspace_id, spec)     → workarea_id
  set_workarea_paused(workarea_id, paused: bool)
  ```
  The exact `rmcp` JSON arg/return schemas are 401's registry entries — this task fills the handler behind each; **never re-shape a schema.**
- **Consumes `ToolClass::ReadOnly` + the strict permission matrix as FROZEN by Task 402 (PHASE4_PLANNING §4.8).** The 5 write tools are classified non-`ReadOnly` (`ToolClass::Restricted`) so `PermissionResolver::decide` returns `MustAsk` under the Maestro's `strict` mode:
  ```rust
  // crates/core/src/security/permission.rs:451 — FROZEN by 402 per 400
  (PermissionMode::Strict, _) => Decision::MustAsk,
  // 402 added: (PermissionMode::Strict, ToolClass::ReadOnly) => Decision::AutoApprove,
  ```
- **Consumes the mutation targets as FROZEN by their owning V0.1/Phase-3 tasks (extend-never-break):**
  ```rust
  // crates/core/src/agent_supervisor/actor.rs:930
  pub async fn send_input(&self, session_id: &SessionId, data: Vec<u8>) -> Result<()>;
  // crates/core/src/workspace_manager/actor.rs:185
  pub async fn create_workspace(&self, name: &str, repos: &[WorkspaceRepoSpec],
      permission_mode: Option<String>, description: Option<String>, icon: Option<String>) -> Result<Workspace>;
  // crates/core/src/workspace_manager/workarea.rs:695
  pub async fn create_workarea(&self, workspace_id: &str, permission_mode: Option<String>) -> Result<Workarea>;
  // crates/core/src/workspace_manager/workarea.rs:2610
  pub async fn transition_workarea(&self, id: &WorkareaId, event: WorkareaEvent) -> Result<Workarea>;
  // crates/core/src/workspace_manager/fsm.rs:133/135 — WorkareaEvent::{Pause, Resume}
  ```
- **Consumes the confirmation-chip lifecycle as FROZEN by Task 33 / Task 402:** `AgentEvent::AwaitingApproval { session_id, approval_id, tool, summary, payload_json, urgent, destructive_label }` (`events.rs:62`) → park on `oneshot::Receiver<Decision>` (`approval.rs::PendingApprovals`) → `Sessions.ResolveApproval` → `user_decision_string(Decision)`.

## Implementation notes
- **The load-bearing rule (§4.8 / D4, `design/08 R-2`): every write tool calls the gate FIRST, mutates SECOND.** The order is `resolve args → classify (Restricted) → resolver.decide ⇒ MustAsk under strict → emit AwaitingApproval + park → on approve, mutate; on deny, typed "declined" result`. A reviewer must be able to read each tool body and see the gate strictly dominate the mutation call. A bypass (mutating before/without the gate) is the single forbidden bug here.
- **Reuse, don't reinvent, the approval registry.** `approval.rs` already owns `PendingApprovals` (the `HashMap<approval_id, oneshot::Sender<Decision>>`) and `user_decision_string`; the Maestro write tools route through the SAME machinery the workarea-session tools use, keyed by the Maestro session's `SessionId`. Do NOT create a second pending-approvals map. If the Maestro session id needs to be threaded into the gate helper, take it as a parameter from the MCP dispatch context (401's tool-call context carries the calling session) — do not hardcode.
- **`fanout_to_sessions` = one gate, N sends.** Per `design/08 §5.1` the user confirms the fanout once; do not emit N chips. Collect `Vec<(SessionId, Result<()>)>` so a single closed/invalid target surfaces as a per-session error in the frozen result instead of failing the whole call or silently dropping.
- **`set_workarea_paused` is reversible ⇒ `destructive_label = None`, `urgent = false`.** It still gates (it is a user-visible mutation) but is not red-styled. `route`/`fanout`/`create` are likewise non-destructive (they create or send, they don't delete) → `destructive_label = None`. No write tool here maps to a `destructive::PATTERNS` label; that styling is for fs/shell deletes the Maestro doesn't have.
- **Seams return a typed error, never the macro.** A closed session (`route`/`fanout`), a missing/archived workspace (`create_workarea` already returns `Error::Validation`), an illegal pause (`transition_workarea` returns `Error::Policy`) — each maps to the tool's structured MCP error result (the typed-`Err` convention 401 froze), NEVER `todo!()`/`unimplemented!()`/`unwrap()` and NEVER empty-success. The `create_*` inner fns return `Result<WorkspaceId>`/`Result<WorkareaId>` so 411 composes them.
- **Cross-platform.** The Maestro spine depends on the agent supervisor, so the MCP server + these tool handlers are `#[cfg(unix)]`-gated exactly like the `sessions`/`streams` handlers (401 establishes the gate; this file inherits it). No `std::os::unix` in the tool bodies themselves; the gate is over the supervisor handle.
- **`tools/mod.rs` is the lead-owned soft seam.** Add exactly one `pub mod write;` + one registration call in a distinct region; 405/407 add theirs in their own regions → additive, auto-merges on rebase. If you find yourself editing 405's or 407's region, stop — that's a serialize signal.
- **Regen:** this task adds no proto/SQL/`crates/*/src/api.rs` public type, so `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` should be a no-op (the tool schemas are 401's MCP registry, not a `docs/interfaces/*` artifact). If the diff is non-empty, you changed a public Rust API you shouldn't have — investigate before committing.
- **Parallel build hint:** the three disjoint sub-parts a lead may fan out and integrate into the one commit are — **(a)** `route_prompt_to_session` + `fanout_to_sessions` (the `send_input` adapters + per-session result collection), **(b)** `create_workspace` + `create_workarea` (the spec→manager mapping + the `do_create_*` reusable inner fns for 411), **(c)** `set_workarea_paused` + the shared chip-gate helper wiring (the `Pause`/`Resume` transition + the `AwaitingApproval`/`ResolveApproval` plumbing all 5 share). (c)'s gate helper is the integration point (a) and (b) both call.

## Verification
**Tier 2.** The double is **401's in-process `concerto-maestro-mcp` harness + a scripted `ResolveApproval`** (a test client that calls each write tool and a test driver that answers the parked `AwaitingApproval` with approve/deny). It proves the tool→manager wiring and the strict-gate-before-mutation invariant; it does NOT cover the real Desktop tap-to-confirm UX (415 / Tier-3).

1. `cargo check --workspace` — clean (the new `tools/write.rs` + the one `tools/mod.rs` line compile).
2. `cargo clippy --workspace --all-targets -- -D warnings` — clean; then `cargo fmt --all -- --check` clean (CI `format.yml` parity — `--all` covers every workspace member).
3. `cargo test -p concerto-core maestro::tools::write` (+ `maestro_write`) — proves: scripted-approve `route_prompt_to_session` calls `send_input` with exact bytes; scripted-deny returns "declined" and `send_input` is never called; `fanout_to_sessions` = one gate + 2 sends with a per-session error on a closed target; `create_workspace`/`create_workarea` (approve) return real ids + row exists; `set_workarea_paused` true→`Paused`→false round-trip + illegal-pause typed `Error::Policy`; **every** write tool emits `AwaitingApproval` before any mutation under `strict` (the §4.8 invariant).
4. `cargo test --workspace --no-fail-fast` — all pass (no regression in `permission`/`approval`/`workspace_manager` suites).
5. `cargo deny check` — green (this task adds no new crate pin; `rmcp` was vetted by 401).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` — **no-op expected** (no proto/SQL/`api.rs` public type changed; the tool schemas live in 401's MCP registry). A non-empty diff means you touched a public Rust API outside Outputs — investigate.
7. `scripts/smoke.sh` — **unchanged** gate (this task adds no smoke capability; the Maestro spine is not yet boot-spawned live — 414 lights it up). Exits 0.

**Tier-2 double + what it does NOT cover.** The double is the in-process MCP harness + scripted approval; it proves the 5 tools wire to `send_input`/`create_workspace`/`create_workarea`/`transition_workarea` and that the **strict gate fires and blocks the mutation until the scripted approval resolves**. It does **NOT** cover the real user tapping the confirmation chip in the Desktop UI — that is **Task 415** (mocked invoke) for the render and the **Phase-4 Tier-3 checklist line** "route prompts via `@workarea` and fanout" (the operator confirms live tap-to-confirm + real routing at the phase gate). No new Tier-3 line is added beyond that existing one.

## Definition of Done
- [x] `route_prompt_to_session` resolves to `AgentSupervisorHandle::send_input` behind a strict confirmation gate
- [x] `fanout_to_sessions` = one confirmation gate then `send_input` per target, with per-session ok/err collected (no swallow)
- [x] `create_workspace` → `WorkspaceManager::create_workspace` and `create_workarea` → `WorkareaManager::create_workarea`, each via a reusable `do_create_*` inner fn Task 411 wraps
- [x] `set_workarea_paused` → `transition_workarea(WorkareaEvent::Pause|Resume)`; illegal transition surfaces the typed `Error::Policy` as a tool error
- [x] All 5 tools classify non-`ReadOnly` (`ToolClass::Restricted`) so `(Strict, _) ⇒ MustAsk` ⇒ the existing `AwaitingApproval`/`ResolveApproval` chip flow runs **before** mutation; no bypass (`design/08 R-2`); reuses `approval.rs`'s `PendingApprovals` (no second registry)
- [x] `tools/mod.rs` gains exactly one `pub mod write;` + one registration line (lead-owned region)
- [x] Tests (Tier 2): approve/deny route, fanout partial-failure, create round-trips, pause/resume + illegal-pause, gate-before-mutation for all 5
- [x] All Verification commands pass on a clean checkout; smoke gate unchanged (green)
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (signature-frozen seams return a typed Err/Status, not the macro — documented in Handoff)
- [x] No files outside Outputs modified
- [x] Interfaces regenerated + committed if any schema/contract changed (expected no-op here)
- [x] Single commit with the message below

## Outputs
- `crates/core/src/maestro/tools/write.rs` (new — the 5 write-tool impl bodies, the shared strict chip-gate helper, the `do_create_workspace`/`do_create_workarea` reusable inner fns for Task 411, and the Tier-2 tests)
- `crates/core/src/maestro/tools/mod.rs` (modified — one `pub mod write;` + one registration line binding 401's frozen write-tool schemas to this file's handlers, in a lead-owned distinct region)

## Commit message
```
phase-4: Maestro write tool set — 5 MustAsk-gated mutation tools

Implements the 5 write tools behind 401's frozen MCP schemas
(route_prompt_to_session, fanout_to_sessions, create_workspace,
create_workarea, set_workarea_paused), each classified non-ReadOnly
so the Maestro's strict mode forces MustAsk → the existing
AwaitingApproval/ResolveApproval confirmation chip runs before any
mutation (no bypass, design/08 R-2). create_* expose reusable inner
fns Task 411 wraps. Tier-2 double: in-process MCP harness + scripted
approval; real tap-to-confirm UX is 415 / Phase-4 Tier-3.

Refs: tasks/v1.0/406-maestro-write-tools.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan** — <e.g. whether the chip-gate helper landed in `write.rs` or was factored to a shared `maestro/tools/mod.rs` helper; whether the Maestro `SessionId` threads through 401's MCP dispatch context as expected; any `ToolClass` assignment that differed (route/fanout/create/pause all Restricted vs. one promoted to Dangerous)>
- **Open questions for next task** — **Task 411 (`create_workspace_from_description`)** builds on the FROZEN `do_create_workspace`/`do_create_workarea` inner fns + the chip-gate in this file (it adds issue-parse + cone-suggest + the privacy-debt fix in front, then calls these); **Task 408**'s deterministic pre-parser is the other consumer of `route_prompt_to_session`/`fanout_to_sessions`. <note the exact inner-fn signatures + the create-flow region in `tools/write.rs` 411 must extend without conflict>
- **Deliberate debt** — <e.g. fanout partial-failure result shape if 401's frozen schema under-specifies it; any session-id resolution shortcut; confirm NO `todo!()`/`unimplemented!()` — the declined/closed-session/illegal-pause paths all return a typed tool error, name them>
- **Smoke-gate state** — <expected: unchanged; the Maestro spine is not boot-spawned live until 414, so no `scripts/smoke.d/*` change; confirm `scripts/smoke.sh` not re-run for an `unchanged` task>
