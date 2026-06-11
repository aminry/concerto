# Task 417 — Desktop Maestro "go-live": write-tool confirmation-chip producer + live budget/state feed + per-workarea visibility toggle

| Field | Value |
|---|---|
| Phase | 4 (UI-completion addendum) |
| Task type | web-ts |
| Verification tier | 2 |
| Size | medium |
| Depends on | 416, 414, 415 |
| Touches subsystem(s) | 15 (Desktop), 08 (Maestro) |
| Smoke gate | unchanged |

## Goal
Make three already-built-but-unreachable Maestro UI surfaces actually work for a user. The Phase-4 audit found: (1) `<ConfirmationChip>` exists (`apps/desktop/src/components/maestro/ConfirmationChip.tsx`) and the `pendingConfirmation` store slot exists (`apps/desktop/src/state/useMaestroStore.ts`) **but nothing ever populates it**, so the design's R-2 "every Maestro write tool confirms before executing" UX can never appear; (2) `<BudgetBanner>` is fed `state={null} budget={null}` (`MaestroChat.tsx:146-147`), so the 80%-amber / 100%-red token meter and the privacy-blanked digest rows are coded + tested but starved of data; (3) `setWorkareaVisibility` is bound in `api/maestro.ts` but **no UI control** anywhere lets a user toggle a workarea's Maestro visibility (`exclude_from_maestro`). This task wires all three live: it adds the `Maestro.GetState` binding (Task 416) and feeds the real `MaestroState` into the banner + digest; it adds a **confirmation-chip producer** that subscribes to the Maestro session's `session.events.<sid>` (session id from `GetState.maestro_session_id`) for write-tool `AwaitingApproval` frames and lifts them into `pendingConfirmation` (resolving via the existing `Sessions.ResolveApproval` path); and it adds a **per-workarea visibility toggle** in the workarea UI. After this task the Maestro chat shows a real budget meter, surfaces a real confirmation chip when the Maestro proposes a write, and lets the user mark a workarea private — entirely against live data (Task 414's emitter is merged). `apps/desktop` only; no proto, no Rust.

## Inputs to read before starting
- `tasks/v1.0/416-maestro-getstate-rpc.md` → the FROZEN `Maestro.GetState` / `MaestroState` proto surface this task binds + renders. **Read 416's committed proto field names; transcribe verbatim.**
- `apps/desktop/src/components/maestro/MaestroChat.tsx` — the mount + the `state={null} budget={null}` / `summaryRows` props to wire; the `useEventSubscription("maestro.events")` fold (`:82-110`); the `pendingConfirmation` render (`:161-169`).
- `apps/desktop/src/components/maestro/BudgetBanner.tsx` (`computeBannerLevel`, the amber/red thresholds — already built + tested, just starved) + `DigestPanel.tsx` (`summaryRows` prop, the `[private workarea, name only]` row at `:208-214`).
- `apps/desktop/src/components/maestro/ConfirmationChip.tsx` + `apps/desktop/src/state/useMaestroStore.ts` (`pendingConfirmation`, `setPendingConfirmation`) — the consumer + the slot to fill.
- `apps/desktop/src/components/center/SessionRegion.tsx` — **the existing `AwaitingApproval` → `Sessions.ResolveApproval` producer pattern to mirror**: the `oneofVariant` dual-spelling reader, reading the approval frame off `session.events.<sid>`, and the resolve call. The confirmation-chip producer is the same pattern pointed at the **Maestro** session id (from `GetState`).
- `apps/desktop/src/api/maestro.ts` (the bindings + `MaestroState` TS type at `:69-89`, currently un-RPC'd) + `apps/desktop/src/api/client.ts` (add `"Maestro.GetState"` to the `RpcMethod` union — 416 adds the matching Rust arm) + `apps/desktop/src/api/sessions.ts` (the `ResolveApproval` binding + event subscription helpers) + `apps/desktop/src/hooks/useEventSubscription.ts`.
- `apps/desktop/src/components/WorkareaList.tsx` (+ any workarea row/menu) — where the per-workarea visibility toggle mounts; `apps/desktop/src/api/workareas.ts` for the workarea shape (the `exclude_from_maestro` field, if surfaced) + how to read current visibility.
- `tasks/v1.0/415-desktop-maestro-chat-ui.md` + `tasks/v1.0/413-privacy-enforcement.md` → Handoff — the `MaestroVisibility` FULL/HARD_FACTS_ONLY semantics + what 413 enforces server-side (this task only drives the toggle + renders blanked rows).

## Scope — in
- **`api/maestro.ts` + `client.ts`:** add `getState(): Promise<MaestroState>` over `callRpc("Maestro.GetState", {})`, mapping 416's frozen `MaestroState` fields; add `"Maestro.GetState"` to the `RpcMethod` union.
- **Live budget/state feed:** in `MaestroChat.tsx`, replace the hardcoded `null`s — fetch `getState` (React Query, keyed; invalidate on `maestro.budget_exhausted`/`digest_generated`/`disabled_by_policy` events) and pass the real `MaestroState` (+ derived budget caps) into `<BudgetBanner state budget>` so `computeBannerLevel` lights the amber/red tiers; pass the privacy-blanked `summaryRows` into `<DigestPanel>` (derive from the digest/state as available; if a field is absent keep the existing empty-state — do not fabricate). Honor `inert`/`inert_reason` for the stale badge + the policy-disabled banner consistently with the event path.
- **Confirmation-chip producer:** a hook/effect that, when `GetState.maestro_session_id` is non-empty, subscribes to that session's `session.events.<sid>` and lifts each Maestro write-tool `AwaitingApproval` frame into `setPendingConfirmation` (mirror `SessionRegion.tsx`'s reader + the `oneofVariant` dual-spelling). `<ConfirmationChip>` then renders it (urgent/`destructive_label` styling already built) and Approve/Deny calls `Sessions.ResolveApproval` (existing path — **no new RPC, no bypass**, design/08 R-2). Clear `pendingConfirmation` on resolve.
- **Per-workarea visibility toggle:** add a control (toggle/menu item) in `WorkareaList.tsx` (or a small workarea-row menu) that reads the workarea's current Maestro visibility and calls `setWorkareaVisibility(workareaId, FULL | HARD_FACTS_ONLY)`; reflect the state (e.g. a "private" badge). Optimistic update + invalidate.
- **Tests (Tier 2, mocked `invoke`):** `getState` binding shape; the budget meter renders amber/red from a mocked `MaestroState` (counts vs caps); the confirmation-chip producer lifts a hand-built `AwaitingApproval` frame into the chip and Approve calls `ResolveApproval` (mocked); the visibility toggle calls `setWorkareaVisibility` with the right enum. Mirror `cores.test.ts`/`maestro.test.ts` conventions.

## Scope — out
- The `Maestro.GetState` RPC + Rust read-model — **Task 416** (consumed here).
- The create-workspace-from-description flow + cone picker — **Task 418**.
- The server-side privacy blanking / budget tripwire — **Tasks 413 / 412** (already merged); this task only renders what the wire reports + drives the toggle.
- Backend/model/daily-budget config UI — still deferred to a later DX/desktop task.
- **Tier-3 (live):** the real cross-machine confirmation round-trip + real budget-meter behavior under a live LLM is the Phase-4 Tier-3 checklist's job; this task's double is mocked `invoke`.

## Public interface this task locks
- No wire contract (consumes 416's `Maestro.GetState`). The TS surface (`getState` binding, the producer hook, the toggle) is renderer-local + append-friendly.

## Implementation notes
- **The confirmation-chip producer is the highest-impact fix** — without it the Maestro can silently execute writes, contradicting R-2. Mirror the *exact* `SessionRegion.tsx` approval reader (dual PascalCase/snake_case `oneofVariant`, the `session.events.<sid>` frame, `Sessions.ResolveApproval`); the only difference is the session id comes from `GetState.maestro_session_id`. Handle the empty-session-id case (Maestro disabled) gracefully — no subscription, no chip.
- **Don't fabricate data for the banner/rows.** Feed exactly what `GetState`/the digest carry; where a value is genuinely absent keep the built empty-state. The goal is "the coded paths now receive real data," not inventing values.
- React-Query-canonical for `getState`/digest; `useMaestroStore` holds only UI ephemera (pending confirmation, composer draft). Don't duplicate server state into Zustand.
- **Verification override (PHASE4_PLANNING §7 precedent, same as 415):** run `pnpm -C apps/desktop typecheck && pnpm -C apps/desktop lint && pnpm -C apps/desktop test && pnpm -C apps/desktop build` (NOT `apps/web`). Mocked `@tauri-apps/api` invoke + vitest component tests are the Tier-2 double.

## Verification
**Tier 2 — §7 override.**
1. `pnpm -C apps/desktop install` (no new devDeps expected — 415/218 added vitest/jsdom/@testing-library).
2. `pnpm -C apps/desktop typecheck` — clean.
3. `pnpm -C apps/desktop lint` — clean.
4. `pnpm -C apps/desktop test` — the new tests (getState binding, amber/red meter, confirmation-chip producer + ResolveApproval, visibility toggle) + the existing maestro suites stay green.
5. `pnpm -C apps/desktop build` — `tsc --noEmit && vite build` clean.
6. `scripts/smoke.sh` — unchanged gate (apps/desktop only).

**Tier-2 double + what it does NOT cover:** mocked `invoke` + component tests prove the binding/producer/toggle wiring against hand-built frames. It does NOT cover the real cross-machine confirmation round-trip, real budget-meter values under a live LLM, or real privacy blanking — the Phase-4 Tier-3 checklist.

## Definition of Done
- [x] `Maestro.GetState` binding + `"Maestro.GetState"` in the `RpcMethod` union; `<BudgetBanner>` + `<DigestPanel>` fed real `MaestroState` (amber/red meter + blanked rows light up)
- [x] Confirmation-chip producer subscribes to `GetState.maestro_session_id`'s `session.events`, lifts write-tool `AwaitingApproval` into `pendingConfirmation`, resolves via `Sessions.ResolveApproval` (no bypass, R-2)
- [x] Per-workarea Maestro-visibility toggle calls `setWorkareaVisibility(FULL | HARD_FACTS_ONLY)` and reflects state
- [x] Tier-2 tests pass (`pnpm -C apps/desktop typecheck|lint|test|build`); smoke unchanged
- [x] No TODO/FIXME/unimplemented in new code; no `src-tauri`/proto/Rust touched; only `apps/desktop/**`
- [x] Single commit with the message below

## Outputs
- `apps/desktop/src/api/maestro.ts` (+ `getState`) + `apps/desktop/src/api/client.ts` (+ `Maestro.GetState`)
- `apps/desktop/src/components/maestro/MaestroChat.tsx` (live state feed + producer wiring) + a new producer hook (e.g. `apps/desktop/src/components/maestro/useMaestroConfirmations.ts`) + tests
- `apps/desktop/src/components/maestro/BudgetBanner.tsx` / `DigestPanel.tsx` (only if a prop shape needs adjusting) + tests
- `apps/desktop/src/components/WorkareaList.tsx` (+ visibility toggle) + test
- `apps/desktop/src/state/useMaestroStore.ts` (only if the slot needs adjusting)

## Commit message
```
phase-4: desktop Maestro go-live — confirmation chips + live budget + visibility toggle

Wires three built-but-unreachable Maestro surfaces against live data:
(1) a confirmation-chip producer subscribing to the Maestro session's
AwaitingApproval frames (via Maestro.GetState.maestro_session_id) →
pendingConfirmation → Sessions.ResolveApproval (no bypass, R-2);
(2) the live MaestroState (Task 416) fed into BudgetBanner (80% amber /
100% red) + the privacy-blanked DigestPanel rows; (3) a per-workarea
Maestro-visibility toggle calling SetWorkareaVisibility. apps/desktop
only; mocked-invoke Tier-2.

Refs: tasks/v1.0/417-desktop-maestro-go-live.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan:** (1) **Maestro session id source** — sourced exactly as specified from `Maestro.GetState.maestro_session_id` (the new React-Query `MAESTRO_STATE_QUERY_KEY` query over `getState()`); `useMaestroConfirmations(maestroState?.maestro_session_id)` subscribes to `session.events.<sid>` only when non-empty (empty ⇒ Maestro disabled / no live session ⇒ no subscription, no chip — proven by the "opens no subscription" test). The reader mirrors `SessionRegion.tsx` verbatim: `body.{Session|session}.kind.{AwaitingApproval|awaiting_approval}` via the dual-spelling `oneofVariant` (`AwaitingApproval` rides `SessionEvent.kind` field 13, streams.proto). Resolution is the unchanged `<ConfirmationChip>` → `Sessions.ResolveApproval` path (no new RPC, no bypass, R-2). (2) **`MaestroState` extended 4→9 fields** to the frozen 416 wire shape (`enabled`/`daily_in_today`/`daily_out_today`/`in_cap`/`out_cap`/`last_digest_at_ms`/`inert`/`inert_reason`/`maestro_session_id`); `BudgetBanner.test.tsx`'s helper was extended to the full shape (no behavior change). (3) **`exclude_from_maestro` added to the desktop `Workarea` TS type** — it was already on the wire (`workareas.proto` `optional bool exclude_from_maestro = 11`, derived by Task 311) but not surfaced in the renderer; the toggle reads it to render the per-row "private" badge and drive the optimistic update. Additive, `apps/desktop` only. (4) **Budget derivation** — `<BudgetBanner>.computeBannerLevel` takes a single `budget` compared against `max(daily_in, daily_out)`; `deriveBudget` pairs the larger counter with *its own* cap (`in_cap` when input-bound, `out_cap` when output-bound) so amber/red lights on whichever dimension is closest to its cap — faithful, no fabricated value.
- **Open questions for next task (banner/rows fields that couldn't be fed yet):** The `<DigestPanel>` **`summaryRows`** (the privacy-blanked `[private workarea, name only]` rows) are **NOT fed** — per the panel's own contract they derive from the Desktop's per-workarea state (`Workareas.ListWorkareas`), not from the `Digest`/`MaestroState` wire, and `MaestroChat` mounts above the panel group without a workspace/workarea context to enumerate. Rather than fabricate rows, the built empty-state is kept (the task explicitly permits this). What *is* now fed live: the **budget meter** (counts vs caps → amber/red), the **stale badge** (`DigestPanel inert={state.inert}`), and the **policy banner** (`inert_reason === "disabled_by_policy"`, consistent with the `disabled_by_policy` event path). A future task wanting blanked digest rows should thread the selected-workspace workarea list (with `exclude_from_maestro`) into `MaestroChat` and map it to `SummaryRow[]`.
- **Deliberate debt:** None. The Tier-2 double is mocked `@tauri-apps/api` `invoke` + vitest (no new devDeps); the real cross-machine confirmation round-trip + real budget-meter values under a live LLM remain the Phase-4 Tier-3 checklist's job (Scope — out).
- **Smoke-gate state:** Unchanged — no `scripts/smoke.sh` edits, no boot/wire/`src-tauri` changes; all work is renderer web-ts. Gate: `pnpm -C apps/desktop typecheck` (clean) · `lint` (clean) · `test` (37 files / 195 tests pass, incl. the new `getState` binding, the amber/red meter, the confirmation-chip producer + `ResolveApproval`, and the visibility toggle) · `build` (`tsc --noEmit && vite build` clean).
