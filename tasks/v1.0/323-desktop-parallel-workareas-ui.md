# Task 323 — Desktop: parallel-workareas summary view + multi-agent session tabs

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | web-ts |
| Verification tier | 2 |
| Size | medium (1–3d) — TS only (`apps/desktop/src`); no `src-tauri` Rust |
| Depends on | 308, 218 |
| Touches subsystem(s) | 15 (Desktop Client), 03 (Workspace/Session Manager — client consumer) |
| Smoke gate | unchanged |

## Goal
Surface two V1.0 multi-X capabilities in the Desktop that the Core now permits but the UI never exposed: **parallel workareas per workspace** and **multiple concurrent agent sessions per workarea**. Today `WorkspaceDetail.tsx` (shown when a workspace, not a workarea, is selected) is a raw `JSON.stringify` dump with a bare "+ New Workarea" button — it gives no overview of a workspace's parallel attempts. And `SessionRegion.tsx` already renders a real multi-session tab strip (Task 26 built the UI ahead of the server cap), but its `NewSessionMenu` only offers `{claude, echo}` and the multi-session story was capped server-side until Task 308 lifted it. This task (1) replaces the JSON dump with the `design/15 §3.4` **"When a workspace is selected" summary** — a list of workareas with status dots, cross-workarea PR-set status placeholders, and "+ new workarea"; and (2) extends the session tab strip's agent menu to `{claude, codex, gemini}` (the `agent_kind` CHECK set), confirms per-session `agent_kind`/status chips render for >1 concurrent session, and surfaces the *effect* of the per-workarea edit mutex (a "blocked on <other session>" state) when the Core reports a write was serialized — **without** re-implementing the mutex, which is server-side (Task 308 / `design/04 §3.5`). After this task a user can compare parallel attempts at a glance and run Claude + Codex + Gemini side-by-side on the same workarea. The lightest of the three Phase-3 desktop tasks (the multi-session tab strip already exists); the main risk is over-building.

## Inputs to read before starting
- `design/15_Desktop_Client.md` §3.4 — two things you build toward: (a) the **Session region** session tabs are "Claude / Codex / Gemini / +new" (the agent set you widen the menu to); (b) **"When a workspace is selected"** the center panel "shows a workspace summary view: list of workareas with status dots, PR set status across workareas, '+ new workarea' button" — the view that replaces the JSON dump in `WorkspaceDetail.tsx`. The §3.4 sidebar tree note also shows workarea rows carry "composer name + branch chip + status dot."
- `design/03_Workspace_Session_Manager.md` §3.4 (a session = one agent on one workarea), §6.3 (multi-session coexist: independent chats/process/permission, **shared worktrees + `.context/`**), §3.1 (the workarea FSM + derived status — Task 307 wired it; the summary renders those statuses), §7.3 (two workareas on one workspace = parallel approaches, "can compare diffs"), §7.4 (add a 2nd session: "Codex runs alongside Claude on the same files; per-workarea edit mutex serializes writes").
- `design/04_Agent_Supervisor.md` §3.5 / R-5 — the **per-workarea edit mutex** (10 s timeout; on contention the second write is rejected with a clear "blocked on <other session>" error). **This is server-side (Task 308).** This task only *surfaces its effect* when the Core reports it; it must not implement any client-side serialization.
- `tasks/v1.0/308-multi-session-edit-mutex.md` → "Handoff Notes" — **confirm Task 308 actually lifted the server-side single-session cap** (V0.1 capped at 1 session/workarea) and how a serialized-write rejection surfaces (the typed error code / event the Core emits). **Do not claim done until `Sessions.ListSessions` can return >1 session** and the mutex-contention surface is known. This is the hard dependency.
- `tasks/v1.0/218-desktop-dual-transport.md` → "Handoff Notes" — the **FROZEN `CoreClient` trait** every RPC/stream dispatches through (`callRpc` / `start_stream` → `commands.rs` → `CoreClient`); the renderer never speaks gRPC. Server-canonical data stays in React Query (`design/15 §3.3`); the active Core is implicit via `callRpc` — no per-Core or `transport_kind` branching is needed here.
- `apps/desktop/src/components/center/SessionRegion.tsx` — the **already-built** session tab strip: auto-select-first, `NewSessionMenu` (today `{claude, echo}`), close-with-confirm, oldest-tab-left sort, `createSession({ workareaId, agentKind })`. **Extend `NewSessionMenu`'s items; do not rebuild the strip.**
- `apps/desktop/src/components/SessionTab.tsx` — one pill: `agent_kind` label + live status dot via `useSessionEvents(sid)`. Confirm it renders correctly for `codex`/`gemini` (it is agent-agnostic already).
- `apps/desktop/src/components/WorkspaceDetail.tsx` — the JSON-dump panel you replace (it owns the `createWorkarea(workspaceId)` button). `apps/desktop/src/components/WorkareaList.tsx` + `apps/desktop/src/lib/workareaStatus.ts` — the existing workarea-row + status-dot mapper to **reuse** in the summary. `apps/desktop/src/hooks/{useWorkareas,useSessions}.ts`, `apps/desktop/src/api/{workareas,sessions}.ts`, `apps/desktop/src/components/AppLayout.tsx` (renders `CenterPanel` when a workarea is selected, else `WorkspaceDetail`), `apps/desktop/src/state/useUiStore.ts` (`activeSessionId` is a single per-window value; `setSelectedWorkarea` clears it).
- `apps/desktop/src/hooks/useEventSubscription.ts` — the live-invalidation primitive (`workarea.events` / `workspace.events`); the summary live-updates by invalidating `["workareas", workspaceId]` on a workarea event, mirroring `Sidebar.tsx`.

## Scope — in
**Parallel-workareas summary (`WorkspaceDetail.tsx` + a new summary component):**
- Replace the `JSON.stringify` dump with the `design/15 §3.4` workspace summary: the workspace's workareas as a list of rows (composer name + branch chip + status dot — **reuse `WorkareaList.tsx` / `workareaStatusToDot`** rather than re-implementing), a "+ new workarea" affordance, and a **cross-workarea PR-set status** column/placeholder (the real PR-set aggregation is Task 324; here it is a slot that 324 fills — render "—" / "no PRs" without binding the PR-set RPC).
- Clicking a workarea row selects it (`setSelectedWorkarea`), switching the center panel to the workarea view (existing `AppLayout` behavior). Live-update the list via the existing `workarea.events` subscription (invalidate `["workareas", workspaceId]`).
- Keep the cone-picker hook for "+ New Workarea" that **Task 322** adds — this task does not own the cone picker; if 322 has landed, its `createWorkarea`-with-cones flow is reused as-is; if not, the plain `createWorkarea(workspaceId)` is fine and 322 layers the picker on later. (Coordinate so the two tasks don't both rewrite the "+ New Workarea" button — note the ordering in Handoff.)

**Multi-agent session tabs (`SessionRegion.tsx`):**
- Extend `NewSessionMenu`'s `items` from `{claude, echo}` to `{claude, codex, gemini}` (matching the `sessions.agent_kind` CHECK `('claude','codex','gemini','maestro')` — `maestro` is P4-internal, not a user-creatable tab; keep `echo` only if the smoke path still needs it — note the decision in Handoff).
- Confirm the tab strip renders >1 concurrent session correctly (per-session `agent_kind` label + status dot already in `SessionTab`) once Task 308 lifts the server cap — add a vitest case asserting N tabs render for an N-session list.
- Surface the **edit-mutex contention effect**: when a session's write is rejected as serialized (the Core surfaces it per Task 308's contract — likely a session error/event or a typed `CoreClientError`), show a non-blocking inline notice ("blocked on <other session>") on that session, reusing `SessionRegion`'s existing `actionError` banner pattern. **No client-side mutex.**

**Plumbing:** no new `RpcMethod` members are required (`Sessions.CreateSession`/`ListSessions` already exist). Add vitest unit/component tests (mock `invoke`).

## Scope — out
- The **sparse-cone picker / multi-repo New-Workspace multi-select / per-repo Level-1 selector** — **Task 322** (the "+ New Workarea" cone picker is 322's; this task only renders the summary list around it).
- The **PR-set panel + coordinated-merge UI + cross-workarea PR-set aggregation binding** — **Task 324** (this task leaves a placeholder slot for cross-workarea PR status; it binds no PR-set RPC).
- **Implementing the per-workarea edit mutex** — server-side, Task 308 / `design/04 §3.5`. This task only *displays its effect*.
- The **`maestro` agent tab / Concerto chat bar** — Maestro is P4 (Task 415). Do not add a Maestro session tab.
- A session-overflow design (first-4-tabs + overflow menu per `design/03 R-7`) beyond what the existing `overflow-x-auto` strip already does — the strip scrolls; a dedicated overflow affordance is a polish follow-on, not required here (note if you choose to add it).
- Any **Rust** in `src-tauri` — the multi-session cap lift + the mutex are upstream (Task 308). This task is the client consumer.
- Real multi-agent contention behavior on real agents (Claude + Codex actually fighting over a file) — **Tier-3** confidence item (the mutex serialization is server-tested in Task 308; the UI surfacing is the only client concern here).

## Public interface this task locks
- **TS/UI (FROZEN):** the `NewSessionMenu` agent set rendered in `SessionRegion.tsx` — `{claude, codex, gemini}` (the user-creatable subset of the `agent_kind` CHECK; `maestro` excluded; `echo` retained only if the smoke path needs it, documented in Handoff). The `agentKind` string passed to `createSession` must match the Core's `agent_kind` CHECK spelling exactly.
- **TS (FROZEN):** the workspace-summary component's public props/shape (`apps/desktop/src/components/WorkspaceSummary.tsx`) — it consumes `useWorkareas(workspaceId)` + `workareaStatusToDot`; the cross-workarea PR-set slot is a typed placeholder Task 324 fills.
- This task **does not lock any proto/SQL/RPC surface** — it consumes only already-frozen `Sessions.*` / `Workareas.*` RPCs (no new wire shapes). State that explicitly so a reviewer confirms no double-lock.

## Implementation notes
- **Don't rebuild what exists.** The session tab strip, close-confirm, auto-select, and sort in `SessionRegion.tsx` are done — the only required change there is the `NewSessionMenu.items` list + the contention notice + a test for N tabs. Resist re-architecting the strip.
- **Reuse the workarea-row + status mapper.** The summary's per-workarea rows should reuse `WorkareaList.tsx`'s rendering and `workareaStatusToDot` so the sidebar tree and the summary agree on status colors (the single-source-of-truth rule that `workareaStatus.ts` already enforces). After Task 307, the status vocabulary includes `finished` and `partial`; confirm `workareaStatusToDot` maps them (extend the mapper if 307 added statuses it doesn't handle — `finished`→idle/green-grey, `partial`→amber/warning — note the choice).
- **`activeSessionId` is per-window, single-valued, reset on workarea switch** (`setSelectedWorkarea` clears it). That interaction is correct for parallel workareas (each workarea selection re-derives the active session from its own session list via `SessionRegion`'s auto-select effect). Confirm it holds; do not key `activeSessionId` per workarea unless a test proves the single value leaks across workareas.
- **Contention surface is best-effort + non-blocking.** A serialized-write rejection is rare and informational; render it like the existing `actionError` banner (dismissible), scoped to the session that was blocked. If Task 308's contract surfaces it as a `session.events` frame rather than an RPC error, subscribe via `useSessionEvents`/`useEventSubscription` and map it — follow 308's handoff for the exact shape.
- **Dispatch through `CoreClient` only.** `callRpc`/`start_stream` → `commands.rs` → active `CoreClient` (Task 218). No raw gRPC; the active Core is implicit.
- **Verification scripts already exist.** `apps/desktop/package.json` has `typecheck`/`lint`/`test`/`build` + `vitest` + `jsdom` + `@testing-library/react` (Task 218). Write colocated `*.test.tsx` mocking `@tauri-apps/api`'s `invoke` (pattern: `ConnectCorePicker.test.tsx`).
- **Tier-2 double.** vitest + mocked `invoke` + `@testing-library/react`. It proves: the summary renders the workarea list with status dots + "+ new workarea"; `NewSessionMenu` offers claude/codex/gemini and `createSession` carries the picked `agentKind`; N concurrent sessions render N tabs; a mocked contention rejection renders the inline notice. It does **NOT** prove real multi-agent file contention on real agents.

## Verification
**Tier 2.** Verification **overrides** the orchestrator's `web-ts` default to **`apps/desktop`** per PHASE3_PLANNING §7:
1. `pnpm -C apps/desktop typecheck` → clean.
2. `pnpm -C apps/desktop lint` → clean (aliased to `tsc --noEmit`).
3. `pnpm -C apps/desktop test` → vitest green, including: `WorkspaceSummary` renders a workarea list with status dots + a "+ new workarea" affordance (replacing the JSON dump) and selecting a row calls `setSelectedWorkarea`; `SessionRegion`'s `NewSessionMenu` lists claude/codex/gemini and `createSession` is called with the picked kind; an N-session `ListSessions` mock renders N tabs; a mocked serialized-write rejection renders the inline "blocked on …" notice without tearing down the strip.
4. `pnpm -C apps/desktop build` → `tsc --noEmit && vite build` clean.

**Tier-2 double + what it does NOT cover.** The double is **vitest + mocked `invoke`** (no Core; `apps/desktop` has no Playwright). It proves the parallel-workarea summary + multi-agent tab UI logic. It does **NOT** cover real Claude+Codex+Gemini running concurrently on one workarea or real edit-mutex contention on real agents → covered by Task 308's Core tests (the serialization) + the Phase-3 Tier-3 checklist confidence ("create a multi-repo workspace"; multi-session is exercised live there).

## Definition of Done
- [x] `WorkspaceDetail.tsx` shows the `design/15 §3.4` workspace summary (workarea list + status dots + "+ new workarea" + cross-workarea PR-set placeholder), replacing the JSON dump
- [x] Summary reuses `WorkareaList`/`workareaStatusToDot`; live-updates via `workarea.events`; row click selects the workarea
- [x] `SessionRegion`'s `NewSessionMenu` offers `{claude, codex, gemini}` (echo decision documented); `createSession` carries the picked `agentKind`
- [x] >1 concurrent session renders >1 tab; per-session `agent_kind`/status chips render (vitest-covered)
- [x] Edit-mutex contention surfaced as a non-blocking inline notice (server-side mutex NOT re-implemented)
- [x] `workareaStatusToDot` handles any new 307 statuses (`finished`/`partial`); no proto/SQL/RPC locked by this task
- [x] All four `pnpm -C apps/desktop` commands pass; vitest covers the cases above
- [x] No `TODO`/`FIXME` in new code (deliberate seams in Handoff); no files outside Outputs modified
- [x] Coordination note with Task 322 on the shared "+ New Workarea" button recorded in Handoff
- [x] Single commit with the message below

## Outputs
- `apps/desktop/src/components/WorkspaceDetail.tsx` (modified — render the summary instead of the JSON dump)
- `apps/desktop/src/components/WorkspaceSummary.tsx` (new — the "When a workspace is selected" summary view)
- `apps/desktop/src/components/center/SessionRegion.tsx` (modified — `NewSessionMenu` agent set + contention notice)
- `apps/desktop/src/lib/workareaStatus.ts` (modified — map any new 307 statuses, if needed)
- `apps/desktop/src/components/WorkspaceSummary.test.tsx` + `apps/desktop/src/components/center/SessionRegion.test.tsx` (new — vitest)

## Commit message
```
phase-3: desktop parallel workareas + multi-agent session tabs

Replaces the WorkspaceDetail JSON dump with the design/15 §3.4 workspace
summary (workarea list + status dots + "+ new workarea") and widens the
session tab strip's agent menu to claude/codex/gemini, surfacing the
per-workarea edit-mutex contention effect (Task 308) without
re-implementing it. Dispatched through the Task 218 CoreClient.

Refs: tasks/v1.0/323-desktop-parallel-workareas-ui.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan.** Stayed within Outputs — no extra files. Two small implementation choices: (1) the edit-mutex contention surface is implemented as a local `useEditMutexContention` helper hook + a `EDIT_MUTEX_BLOCKED_WIRE_CODE` constant **inlined inside `SessionRegion.tsx`** (not a new `src/hooks/` file) to respect the Outputs list — it subscribes to the active session's `session.events.<sid>` via the existing `useEventSubscription` + `oneofVariant` (already exported from `api/sessions.ts`, no edit there) and parses the `ApprovalResolved.decision` string per Task 308's contract. The `api/sessions.ts` `SessionEventPayload.kind` union still doesn't list `ApprovalResolved` (a P3 frame variant); we read it dynamically via `oneofVariant`, so no type-file edit was needed — if a future task wants it statically typed, add the variant to that union (recorded as a deliberate seam, not debt). (2) The summary's "+ new workarea" button is a callback (`onNewWorkarea`) into the parent `WorkspaceDetail`, which keeps owning the Task-322 cone-picker dialog (see coordination note below).
- **Open questions for next task.** Task 324 fills the cross-workarea PR-set slot: `WorkspaceSummary.tsx`'s `renderPrSetStatus(workarea)` is the single typed seam (currently returns `"—"` per row, binds no PR-set RPC) — 324 replaces that function (or the right-hand column) with the live aggregation. The `WorkspaceSummary` public props (`{ workspaceId, onNewWorkarea }`) are FROZEN for 324. Contention contract consumed from Task 308: the blocked write rides `session.events.<sid>` as an `ApprovalResolved` whose `decision` is `"workarea.edit_mutex.blocked: blocked on session <id>"` (no new proto field) — if a future task moves it to a dedicated frame, update the parser in `useEditMutexContention`.
- **Deliberate debt.** (a) **echo dropped from the new-session menu** — `AGENT_MENU_ITEMS` is now exactly `{claude, codex, gemini}` (the user-creatable `agent_kind` CHECK subset; `maestro` excluded as P4-internal). `echo` is the V0.1 smoke agent; `scripts/smoke.sh` creates its echo session directly via the Core, not through this menu, so dropping it from the UI does not touch the smoke gate. (b) The contention notice is **best-effort + scoped to the active session only** — if a *background* (non-active) session is blocked, its notice surfaces when the user switches to that tab on the next emitted frame (the subscription follows `activeSessionId`); a per-tab persistent badge is a polish follow-on, not required here. (c) No session-overflow affordance added beyond the existing `overflow-x-auto` strip (`Scope — out`). (d) `workareaStatusToDot` maps `finished`→idle/grey and `partial`→warning/amber (the 307-added statuses), matching the design's "done = idle, partial = needs-review" semantics.
- **Coordination note with Task 322 (shared "+ New Workarea" button).** Task 322 had already landed in this worktree's base (the cone-picker + `createWorkarea({ cones })` flow live in `WorkspaceDetail.tsx`). This task did **not** rewrite that flow: it removed the standalone top-of-panel "+ New Workarea" button and routed the summary's "+ new workarea" affordance into the **same** parent-owned cone-picker dialog/mutation via the `onNewWorkarea` callback. So there is exactly one create path; 322's cone picker is reused as-is. **Smoke-gate state: unchanged/green** — no `src-tauri` Rust touched, smoke gate (Core-side) is out of scope for this TS-only task. Tier-2 double = vitest + mocked `invoke` + `@testing-library/react`; it proves the summary/tab/contention **UI logic** but does NOT cover real Claude+Codex+Gemini running concurrently on one workarea or real edit-mutex contention on real agents (Task 308 Core tests + the Phase-3 Tier-3 multi-repo checklist cover that). Gate: `typecheck` + `lint` + `test` (63 passed) + `build` all clean.
