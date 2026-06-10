# Task 415 — Desktop: Concerto chat top bar + `Digest` rendering + `@workarea` routing UX + confirmation chips (consumes 401.5's frozen proto, mocked `invoke`)

| Field | Value |
|---|---|
| Phase | 4 |
| Task type | web-ts |
| Verification tier | 2 |
| Size | medium — TS only (`apps/desktop/src`); no `src-tauri` Rust |
| Depends on | 401.5, 218 |
| Touches subsystem(s) | 15 (Desktop Client), 08 (Maestro) |
| Smoke gate | unchanged |

## Goal
Build the **Desktop renderer surface for the Maestro chat** — the always-present "Concerto chat" top bar + composer, the digest panel, the `@workarea`/fanout routing affordances, and the write-tool confirmation chips — entirely in TypeScript against **401.5's FROZEN wire surface** (`design/08 §3.6/§3.8` + `design/15`). Today there is **zero Maestro UI**: `apps/desktop/src/App.tsx` mounts only `AppLayout` (sidebar | center | right-rail, `design/15 §3.4`) with **no top bar above the three-panel split**, the `client.ts` `RpcMethod` union has **no `Maestro.*` arm** (`apps/desktop/src/api/client.ts:12`), there is no `src/api/maestro.ts` binding, and `SessionRegion.tsx:300` even notes "`maestro` is the P4-internal orchestrator (Task 415), not a user-creatable tab" — i.e. the seam is reserved and unbuilt. This task adds, in `apps/desktop/src` only: (a) `src/api/maestro.ts` — the typed `callRpc("Maestro.SendToMaestro"|"Maestro.GetDigest"|"Maestro.SetWorkareaVisibility", …)` binding + the `maestro.events` subscription wiring (over `useEventSubscription`), **consuming 401.5's `maestro.proto`/`Digest`/`Chip` shapes as frozen**; (b) a `<MaestroChat>` top-bar component cluster (composer, message transcript, `<DigestPanel>` rendering the Finished/Blocked/Working groups + the one-line next step + chips, `<ConfirmationChip>` rendering the write-tool `AwaitingApproval`→`ResolveApproval` flow, the **budget-exhausted yellow banner** + the **stale-digest badge** of `design/08 R-7`); (c) a Zustand UI-only slice (`useMaestroStore`) for composer draft + digest-panel open/collapsed + pending-confirmation selection. **This task adds NO `RpcMethod` to the Rust shell dispatch table and NO `src-tauri` code** — 401.5 froze the wire types and the (initially `Status::unimplemented`) handler; the `Maestro.*` shell dispatch arm + live data are **414's** job. Because 401.5 freezes the surface, **415 depends on 401.5, NOT 414**, so it runs in parallel with the entire Rust Maestro spine behind a **mocked `@tauri-apps/api` `invoke`** double. After this task the Desktop has a fully-rendered, test-doubled Maestro chat that 414 lights up with live data with **zero UI rework**; what stays out is real cross-machine live-Maestro rendering (the Phase-4 Tier-3 checklist line).

## Inputs to read before starting
- `tasks/v1.0/PHASE4_PLANNING.md` §7 — **AUTHORITATIVE** the `Verification` override (D… for 415): the orchestrator `web-ts` set targets `apps/web` which does not exist until P5/519, so 415 MUST run `pnpm -C apps/desktop typecheck|lint|test|build` instead; the Tier-2 double is mocked `invoke` + component tests.
- `tasks/v1.0/PHASE4_PLANNING.md` §4.2 — **AUTHORITATIVE** the FROZEN `maestro.proto` + `MaestroHandle` + `maestro.events` surface owned by **401.5**; this task **CONSUMES** it, never re-locks it (`Maestro { SendToMaestro / GetDigest / SetWorkareaVisibility }`, the `Digest`/`Chip`-bearing messages, payloads ride `Event.checks_opaque=17` under `Subject::MaestroEvents`).
- `tasks/v1.0/PHASE4_PLANNING.md` §1 D11 — digest chips are persisted by the Maestro on the digest's `chat_messages` row (a Maestro-owned slate), NOT the volatile suggestion buffer; D4/D7 (write tools surface as the existing `AwaitingApproval`/`ResolveApproval` confirmation chip; events ride the opaque carrier).
- `tasks/v1.0/401.5-maestro-wire-contract-freeze.md` → "Public interface this task locks" + "Handoff Notes" — **the single source of truth for the exact proto field names/numbers + the `maestro.events` JSON frame shape this renderer parses.** Read its FROZEN block; transcribe the `Digest`/`Chip`/`MaestroState` field names verbatim into `src/api/maestro.ts`. If 401.5's built proto diverges from `design/08 §5.3`, **401.5's handoff governs** (it transcribes the built shape).
- `tasks/v1.0/218-desktop-dual-transport.md` → "Handoff Notes" — the `CoreClient` trait the shell routes through, the `cores.json` registry, and crucially **the pnpm scripts 218 added** (`typecheck`/`test: vitest run`/`lint: tsc --noEmit`, `vitest.config.ts`, `node` env, mocked `@tauri-apps/api`); 415 reuses these verbatim (do NOT re-add them).
- `design/08_Maestro_Agent.md` §3.6 — the digest UX: grouped **Finished / Blocked / Still-working** + a one-line proposed next step, rendered **above the standard chat composer**; chips come from 07; target <5 s p50 to UI render.
- `design/08_Maestro_Agent.md` §3.3 — the `WorkareaSummary`/`SessionSummary`/`RepoSummary` shapes the digest/transcript surface (status, branch, composer, hard facts: commits_ahead/files_changed/lines/pr_state/ci_state) + the privacy-blanked `[private workarea, name only]` rendering (consumes 404/413's shape via the wire).
- `design/08_Maestro_Agent.md` §3.5 + §3.8 — the routing grammar the UX affords (`@workarea`, `@workarea/session`, `@a,@b` fanout, `@all`/`@idle`/`@blocked`, `/digest`/`/pause`/`/new`); the session response surfaced back as **quoted lines**; the create-workspace-from-description confirmation-chip slate. **The parse is server-side (408); the renderer only affords + displays it.**
- `design/08_Maestro_Agent.md` §3.9 (R-7, R-10) — budget-exhausted **yellow banner** "Maestro budget exhausted; routing still works"; the **last good digest with a stale badge** when inert; 80% amber / 100% red budget thresholds; §3.10 the `enterpriseDataPrivacy`-disabled banner.
- `apps/desktop/src/api/client.ts` — the data-layer conventions to mirror: `callRpc<Req,Res>(method, payload)`, `subscribe`/`unsubscribe`/`onConcertoEvent`, the **dot→slash subject mapping** (`eventNameForSubject`), and `errorMessage(e)` reading the `{kind,message}` envelope. **Do NOT add a `Maestro.*` arm to the `RpcMethod` union here unless 414 has not yet** — see Implementation notes (the string is a renderer↔shell contract; 414 owns the shell dispatch arm).
- `apps/desktop/src/hooks/useEventSubscription.ts` — the long-lived stream hook (`subject="" ⇒ skip`); the `maestro.events` subscription mounts through this.
- `apps/desktop/src/components/center/SessionRegion.tsx:300`/`310`-`359` + `apps/desktop/src/api/sessions.ts` — the **existing `AwaitingApproval`/`ResolveApproval` precedent** the confirmation chip reuses: the `oneofVariant` PascalCase/snake_case dual-spelling reader, the `session.events.<sid>` approval frame, and the `Sessions.ResolveApproval` resolve path; mirror this for the Maestro write-tool chip (proto `AwaitingApproval.urgent=5`/`destructive_label=6`, `crates/proto/proto/concerto/v1/streams.proto:286`).
- `apps/desktop/src/components/AppLayout.tsx` + `apps/desktop/src/App.tsx` — where the top bar mounts: `<MaestroChat>` sits **above** the `PanelGroup` (or as a collapsible top region of the App root), so it is **always present** across workspace/workarea selection (`design/08 §1`: "the Concerto chat at the top of the app"); the existing `useUiStore` Zustand pattern (UI-only state, server-canonical state in React Query) is the slice convention to follow.
- `apps/desktop/src/components/ui/{badge,button,card,icon-button}.tsx` — the existing primitives (the `Badge`/chip, `Button`) to reuse for chips/banners; do NOT introduce a new UI primitive library.

## Scope — in
**`apps/desktop/src/api/maestro.ts` (the binding + event wiring):**
- Typed `sendToMaestro(text, attachments)`, `getDigest()`, `setWorkareaVisibility(workareaId, vis)` over `callRpc("Maestro.SendToMaestro"|"Maestro.GetDigest"|"Maestro.SetWorkareaVisibility", …)`, transcribing **401.5's frozen** request/response field names (consumes §4.2; do not invent fields).
- TS mirror types `Digest` (`text`/`chips`/`generated_at_ms`/`stale`), `Chip` (mirrors `suggestions.proto Chip{rule_id=1..action=6}` as 401.5 reuses it), `MaestroState` (the `MaestroStateView` read-model 401.5 froze: `enabled`/`daily_in_today`/`daily_out_today`/`last_digest_at_ms`), `MaestroVisibility` — each a **comment-cited mirror of the frozen proto**, prost-serde snake_case on the wire. **All timestamps are `int64`-ms plain numbers (401.5 froze `generated_at_ms`/`last_digest_at_ms` as unix-ms, no `google.protobuf.Timestamp`) — NOT `[seconds, nanos]` tuples.** The Finished/Blocked/Working **grouping is textual** — it lives inside `Digest.text` (the LLM-grouped prose, `design/08 §3.6`), NOT as wire sub-messages; there is no `DigestGroup` on the frozen wire.
- The `maestro.events` subscription helper (subject `maestro.events`, via `onConcertoEvent`/`useEventSubscription`) decoding the **opaque `checks_opaque=17` frame** 414 publishes (`maestro.message`/`routing_executed`/`digest_generated`/`budget_exhausted`/`disabled_by_policy`) into a typed `MaestroEvent` union; parse defensively (dual PascalCase/snake_case via the `oneofVariant` helper) since the live emitter is 414.

**`apps/desktop/src/components/maestro/*` (the chat top bar + composer):**
- `<MaestroChat>` — the always-present top region (mounted in `App.tsx`/`AppLayout`, collapsible), holding the transcript, composer, digest panel, banners.
- `<MaestroComposer>` — multi-line input mirroring `SessionComposer` (Cmd/Ctrl+Enter submits) calling `sendToMaestro`; renders **routing affordances**: live `@`-token highlighting + a workarea-name autocomplete sourced from `Workareas.ListWorkareas` (React Query), `/`-directive hints (`/digest` `/pause` `/new`). The composer does **not** parse routing (408 does, server-side); it only affords + previews the target set.
- `<MaestroTranscript>` — renders `maestro.message` chat lines + the **quoted session-response surfacing** (`design/08 §3.5`: routed session output shown back as quoted lines, e.g. "Routed to bach / Claude → …").

**`apps/desktop/src/components/maestro/DigestPanel.tsx` (digest rendering):**
- Render the frozen `Digest.text` (the LLM-grouped **Finished / Blocked / Still-working** prose + the **one-line proposed next step**, `design/08 §3.6`). **The grouping is textual** — it is carried in `text`, NOT as wire fields — so the panel styles the prose (optionally section-splitting on the known headers) rather than mapping structured wire groups. Any richer **per-workarea hard-fact rows** (status dot, branch chip, commits_ahead/PR/CI, the privacy-blanked `[private workarea, name only]` case) are sourced from the Desktop's **existing workarea state** (React Query over `Workareas.ListWorkareas`), **NOT** from the `Digest` message — keep that an optional enhancement; the digest body itself is `text` + `chips`.
- Render the digest's persisted **chips** (D11) as actionable `<MaestroChip>`s; the **stale-digest badge** (R-7) when `MaestroState.enabled=false`/inert (show the last good digest dimmed + a "stale" badge); a manual `/digest` / refresh affordance.

**`apps/desktop/src/components/maestro/ConfirmationChip.tsx` (write-tool confirmation):**
- Render the write-tool `AwaitingApproval` frame (the 5 write tools `route_prompt_to_session`/`fanout_to_sessions`/`create_workspace`/`create_workarea`/`set_workarea_paused` + `propose_chip` route through the existing chip flow per D4) with `urgent` red styling + the `destructive_label`; Approve/Deny call `Sessions.ResolveApproval` (the existing path, reused verbatim — no new RPC). **No bypass** (`design/08 R-2`): every user-visible side effect confirms.

**`apps/desktop/src/components/maestro/BudgetBanner.tsx`:**
- The **yellow** budget-exhausted banner ("Maestro budget exhausted; routing still works") on `maestro.budget_exhausted`/`MaestroState` budget tripped; the `enterpriseDataPrivacy`-disabled banner on `maestro.disabled_by_policy`; 80% amber / 100% red thresholds (R-10) computed from `MaestroState` daily counters vs the budget.

**`apps/desktop/src/state/useMaestroStore.ts` (Zustand UI-only):**
- Composer draft text, digest-panel open/collapsed, pending-confirmation selection — UI ephemera only; the digest/state/transcript are **React-Query-canonical** keyed off the bindings (per `design/15 §3.3`; never duplicate server state into Zustand).

- Tests (Tier 2): `src/api/maestro.test.ts` — the binding shape + `maestro.events` frame decode against a **mocked `invoke`** (vitest, mirroring `cores.test.ts`); `DigestPanel.test.tsx` — grouping render (Finished/Blocked/Working + next step), the stale-badge (R-7) render when inert, the privacy-blanked row; `ConfirmationChip.test.tsx` — `AwaitingApproval` render + Approve→`ResolveApproval` invoke (mocked), `urgent`/`destructive_label` styling; `BudgetBanner.test.tsx` — yellow exhausted banner + amber/red thresholds; `useMaestroStore.test.ts` — the UI slice; `MaestroComposer.test.tsx` — `@`-token affordance + Cmd+Enter submit (mocked `sendToMaestro`).

## Scope — out
- **The live `Maestro.*` shell dispatch arm + real data** — **Task 414** (fills 401.5's `Status::unimplemented` handler, adds the `Maestro.*` arm to the Rust shell dispatch + the `RpcMethod` union, publishes `maestro.events`). 415 builds entirely against the **mocked `invoke`** double; this task leaves the live data path as the seam 414 wires (zero UI rework when it lands).
- **The `maestro.proto` / `MaestroHandle` / `maestro.events` subject + the `MaestroServer` registration** — **Task 401.5** (FROZEN, §4.2). 415 consumes them; it adds no proto, no `src-tauri` Rust, no migration.
- **Server-side routing parse / composer→session resolution** — **Task 408** (`pre_parse` + the resolver). The renderer only affords `@`/`/` syntax + previews; it never resolves or routes.
- **The summary cache + digest content + privacy blanking + chip persistence** — **Tasks 404/409/413**. 415 renders whatever the frozen `Digest`/summary wire shapes carry; it derives no facts and applies no privacy gate client-side.
- **`notify_user` push surfaces / lock-screen chips** — **Tasks 407/507** (P5, mobile). 415 renders only the in-chat confirmation chips + banners.
- **Settings → Concerto Chat backend/model picker + budget config UI** — out of this task's surface (the provider seam is 402/412; a settings panel for it is a later DX/desktop task). 415 only **reads** `MaestroState` to render banners/badges.
- **Real cross-machine live-Maestro rendering / digest-quality judgement** — **Tier-3** Phase-4 checklist line ("leave for >30 min, return, judge digest quality + measure latency; route prompts via `@workarea` and fanout"). The mocked-`invoke` double cannot prove live rendering or latency.

## Public interface this task locks
- **This task locks NO wire contract.** It **consumes** the FROZEN `maestro.proto` (`service Maestro { SendToMaestro / GetDigest / SetWorkareaVisibility }` + the `Digest`/`Chip`-bearing messages), the `maestro.events` subject (payloads on `Event.checks_opaque = 17` under `Subject::MaestroEvents`), and `MaestroState`, **as frozen by Task 401.5 (PHASE4_PLANNING §4.2)** — see 401.5's "Public interface this task locks". It also consumes the `AwaitingApproval`/`Sessions.ResolveApproval` flow **as frozen by Task 33** (`streams.proto:286`, `urgent=5`/`destructive_label=6`) and the `Chip` shape **as frozen by Task 07** (`suggestions.proto:29`).
- **TS-internal surface (renderer-local, append-friendly — not a cross-process contract):** the `src/api/maestro.ts` binding (`sendToMaestro`/`getDigest`/`setWorkareaVisibility` + the `Digest`/`Chip`/`MaestroState`/`MaestroEvent` mirror types) is the surface Task 414's live data flows into unchanged and any later desktop Maestro task imports. The TS mirror types track the frozen proto field names verbatim:
  ```ts
  // Mirrors concerto.v1.Digest as FROZEN by Task 401.5 (PHASE4_PLANNING §4.2).
  // prost-serde keeps proto snake_case on the wire; field names transcribed
  // from 401.5's maestro.proto — DO NOT diverge (414 emits this exact shape).
  export type Digest = {
    text: string;                 // the 3-5 sentence grouped digest body (groups are TEXTUAL, not wire fields)
    chips: Chip[];                // persisted on the digest chat_messages row (D11)
    generated_at_ms?: number;     // 401.5 froze `int64 generated_at_ms = 3` (unix epoch ms; NO google.protobuf.Timestamp)
    stale?: boolean;              // R-7: true when shown while Maestro inert
  };
  // Mirrors concerto.v1.Chip (401.5's local copy of suggestions.proto:29, Task 07): rule_id=1 … action=6.
  export type Chip = {
    rule_id: string;
    workarea_id?: string | null;
    title: string;
    priority: number;
    created_at_ms?: number;       // Chip.created_at_ms = 5 (unix ms)
    action?: ChipAction;
  };
  // Mirrors 401.5's `MaestroStateView` read-model (filled by 414 from maestro_state, migration 0015, Task 403).
  // All timestamps are i64 unix-ms plain numbers — NOT [seconds, nanos] tuples (401.5 §4.2 / PHASE4_PLANNING §2).
  export type MaestroState = {
    enabled: boolean;
    daily_in_today: number;       // i64
    daily_out_today: number;      // i64
    last_digest_at_ms?: number | null;  // Option<i64> unix-ms
  };
  ```
  > The exact `Digest`/`MaestroMessageRequest`/`VisibilityRequest` field names MUST be reconciled against 401.5's committed `maestro.proto` before coding — 401.5's handoff is authoritative where it diverges from `design/08 §5.3`'s sketch above.

## Implementation notes
- **The load-bearing rule: 415 is wire-frozen-consumer, NOT wire-author.** Every type in `src/api/maestro.ts` is a **mirror** of 401.5's frozen proto; the file adds no proto, no migration, no `src-tauri` Rust. The whole point of the 401.5 split (PHASE4_PLANNING §6 "the 415 unlock is the headline") is that this task overlaps the entire Rust spine and 414 lights it up with zero rework.
- **Who owns the `Maestro.*` `RpcMethod` arm + shell dispatch — 414, not 415.** The `client.ts` `RpcMethod` union string (`"Maestro.SendToMaestro"` …) is a renderer↔Rust-shell contract: it only works once the Rust `commands.rs` dispatch table has the matching `Maestro.*` arm, which is **414's** Rust work (414 owns `handlers/maestro.rs`). Resolve this **in-task and record in Handoff**: either (preferred) add the `Maestro.*` arms to the `RpcMethod` union in `client.ts` and have the mocked `invoke` answer them (the union is a pure TS type + the live arm is 414's Rust dispatch — additive, no collision, mirrors how `Repositories.EstimateConeSize` was typed in 415-style tasks ahead of full wiring), **or** keep the strings local to `maestro.ts` until 414 lands. Do NOT touch `src-tauri/src/commands.rs` either way — that is 414's file (write-set disjoint).
- **Reuse, don't reinvent, the approval chip.** The write-tool confirmation chip is the **existing** `AwaitingApproval`→`Sessions.ResolveApproval` flow (`SessionRegion.tsx`'s `oneofVariant` dual-spelling reader + the resolve call), not a new RPC. The 5 write tools + `propose_chip` surface through it under strict mode (D4); render `urgent` red + `destructive_label`. **No bypass** (R-2).
- **Mocked-`invoke` double is the Tier-2 spine.** All tests stub `@tauri-apps/api`'s `invoke` (the 218 vitest + `node`/jsdom convention; mirror `cores.test.ts`/`runtime.test.ts`). The `maestro.events` decode test feeds a hand-built opaque-`checks_opaque` frame (the shape 414 emits) through the helper and asserts the typed `MaestroEvent` union — parse **defensively** (dual PascalCase/snake_case) since the live emitter is a sibling task not yet merged.
- **Always-present top bar, server-canonical state.** `<MaestroChat>` mounts above the `PanelGroup` so it persists across selection (`design/08 §1`). The digest/state/transcript are React-Query-canonical (keyed off `getDigest`/the `maestro.events` invalidation pattern in `useEventSubscription`); `useMaestroStore` holds only composer draft + panel-collapse + pending-confirmation (UI ephemera, `design/15 §3.3`). Do not duplicate server state into Zustand.
- **Cross-platform / no Rust gate.** This is `apps/desktop/src` only — no `#[cfg(unix)]`, no proto regen, no `cargo` step. The renderer never speaks gRPC/keychain/fs directly (Tauri capabilities forbid it); all Core traffic is through the mocked `invoke` here and the real shell at runtime.
- **Regen:** none — no proto/schema/rust-api change in this task (415's Outputs are `apps/desktop/src/**` only); `./scripts/regen-interfaces.sh` produces no diff. Do NOT run it expecting a change.
- **Parallel build hint:** the three sub-surfaces are file-disjoint and fan out to helper sub-agents, integrated into one commit (DAG `fanout`): **chat-topbar+composer** (`MaestroChat`/`MaestroComposer`/`MaestroTranscript` + `useMaestroStore` + `api/maestro.ts` binding) ∥ **digest-rendering** (`DigestPanel` + the summary-row + stale-badge) ∥ **routing-UX+confirmation-chips** (`@`-affordance preview + `ConfirmationChip` + `BudgetBanner`). All three share `api/maestro.ts`'s type mirrors (the topbar lead owns that file; the other two import it) — a soft seam, additive on merge.

## Verification
**Tier 2.** **Verification OVERRIDE (PHASE4_PLANNING §7):** the orchestrator's `web-ts` command set (README §5.3) targets `apps/web`, which **does not exist until Phase 5 (Task 519)**. 415 therefore runs against **`apps/desktop`** (the scripts Task 218 added: `typecheck`/`lint`/`test`/`build`, `vitest` + `node`/jsdom, mocked `@tauri-apps/api`; there is **no** Playwright in `apps/desktop`). Run, in order:

1. `pnpm -C apps/desktop install` — ensure deps present (no new devDep needed; 218 added `vitest`; add `@testing-library/react`/`jsdom` to `package.json` + lockfile ONLY if the component tests need them and 218/322/323 did not already — record in Handoff).
2. `pnpm -C apps/desktop typecheck` — clean (`tsc --noEmit`); proves `src/api/maestro.ts` mirror types + all `maestro/*` components compile against the existing `callRpc`/`useEventSubscription` signatures.
3. `pnpm -C apps/desktop lint` — clean (218 aliases `lint` to `tsc --noEmit`).
4. `pnpm -C apps/desktop test` — vitest green; proves: `maestro.ts` binding shape + `maestro.events` frame decode (mocked `invoke`); `DigestPanel` Finished/Blocked/Working grouping + one-line next step + R-7 stale-badge + privacy-blanked row; `ConfirmationChip` `AwaitingApproval` render + Approve→`Sessions.ResolveApproval` invoke + `urgent`/`destructive_label` styling; `BudgetBanner` yellow-exhausted + amber/red thresholds; `useMaestroStore` slice; `MaestroComposer` `@`-affordance + Cmd+Enter submit.
5. `pnpm -C apps/desktop build` — `tsc --noEmit && vite build` clean (the top bar mounts in the real `App.tsx`/`AppLayout` tree without breaking the existing three-panel build).
6. `scripts/smoke.sh` — **unchanged gate** (this task touches no smoke capability; `apps/desktop/src` only, no Core/boot change). It is the operator/CI gate for an `unchanged` task and is not re-run in-worktree.

**Tier-2 double + what it does NOT cover.** The double is the **mocked `@tauri-apps/api` `invoke` + React-Query/Zustand component tests** (jsdom + `@testing-library/react`). It proves the binding shapes, the `maestro.events` frame decode, the digest/chip/banner/confirmation render logic, and the composer affordances against hand-built frames. It does **NOT** cover **real cross-machine live-Maestro rendering** — a real `Maestro.*` round-trip, real digest content + latency, real `@workarea` routing through 408/414, or live `maestro.events` from a running Maestro session. Those are the **Phase-4 Tier-3 checklist** lines ("leave for >30 min across active workareas, return, judge digest quality + measure latency; route prompts via `@workarea` and fanout"), signed off at the phase gate after 414 lights up live data. (Task 414 is the Tier-1 capstone that wires the live `Maestro.*` shell arm this UI consumes.)

## Definition of Done
- [x] `apps/desktop/src/api/maestro.ts` — typed `sendToMaestro`/`getDigest`/`setWorkareaVisibility` bindings + `maestro.events` decode + the `Digest`/`Chip`/`MaestroState`/`MaestroEvent` TS mirror types, transcribed from 401.5's FROZEN `maestro.proto` (consumes §4.2; no proto/migration authored)
- [x] `<MaestroChat>` always-present top bar (mounted in `App.tsx`/`AppLayout`) + `<MaestroComposer>` (Cmd+Enter, `@`/`/` routing affordances + workarea autocomplete) + `<MaestroTranscript>` (messages + quoted session-response surfacing)
- [x] `<DigestPanel>` renders Finished/Blocked/Still-working groups + one-line next step + persisted chips (D11); stale-digest badge (R-7) when inert; privacy-blanked `[private workarea, name only]` row
- [x] `<ConfirmationChip>` renders the write-tool `AwaitingApproval` flow (urgent + `destructive_label`) and resolves via the existing `Sessions.ResolveApproval` path (no bypass, R-2; no new RPC)
- [x] `<BudgetBanner>` yellow budget-exhausted banner + `enterpriseDataPrivacy`-disabled banner + 80% amber / 100% red thresholds from `MaestroState`
- [x] `useMaestroStore` UI-only Zustand slice (composer draft / panel-collapse / pending confirmation); digest/state/transcript stay React-Query-canonical
- [x] No `src-tauri` Rust, no proto, no migration touched; the `Maestro.*` shell dispatch arm is left to Task 414 (the `RpcMethod`-union decision recorded in Handoff)
- [x] All Verification commands pass on a clean checkout (`pnpm -C apps/desktop typecheck && lint && test && build`, the §7 override)
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (the unwired-live-data seams degrade to mocked `invoke`/empty-state renders documented in Handoff, not stubs)
- [x] No files outside Outputs modified
- [x] Interfaces regenerated + committed if any schema/contract changed — **N/A** (no proto/schema/rust-api change; `regen-interfaces.sh` produces no diff)
- [x] Single commit with the message below

## Outputs
- `apps/desktop/src/api/maestro.ts` (new — `Maestro.*` bindings + `maestro.events` decode + the `Digest`/`Chip`/`MaestroState`/`MaestroEvent` mirror types)
- `apps/desktop/src/api/maestro.test.ts` (new — binding shape + event-frame decode against mocked `invoke`)
- `apps/desktop/src/components/maestro/MaestroChat.tsx` (new — the always-present top-bar cluster)
- `apps/desktop/src/components/maestro/MaestroComposer.tsx` (new — composer + `@`/`/` routing affordances) + `MaestroComposer.test.tsx`
- `apps/desktop/src/components/maestro/MaestroTranscript.tsx` (new — messages + quoted session responses)
- `apps/desktop/src/components/maestro/DigestPanel.tsx` (new — grouped digest + chips + stale badge) + `DigestPanel.test.tsx`
- `apps/desktop/src/components/maestro/ConfirmationChip.tsx` (new — write-tool `AwaitingApproval`→`ResolveApproval` chip) + `ConfirmationChip.test.tsx`
- `apps/desktop/src/components/maestro/BudgetBanner.tsx` (new — yellow/amber/red budget + policy-disabled banners) + `BudgetBanner.test.tsx`
- `apps/desktop/src/state/useMaestroStore.ts` (new — UI-only Zustand slice) + `useMaestroStore.test.ts`
- `apps/desktop/src/App.tsx` and/or `apps/desktop/src/components/AppLayout.tsx` (modified — mount `<MaestroChat>` above the three-panel split)
- `apps/desktop/src/api/client.ts` (modified ONLY if the in-task decision adds the `Maestro.*` arms to the `RpcMethod` union — see Implementation notes / Handoff)
- `apps/desktop/package.json` + `apps/desktop/pnpm-lock.yaml` (modified ONLY if `@testing-library/react`/`jsdom` are newly needed and not already present — record in Handoff)

## Commit message
```
phase-4: desktop Concerto chat top bar + digest + routing UX + confirmation chips

Adds the apps/desktop Maestro chat surface against 401.5's FROZEN
maestro.proto / Digest / Chip / maestro.events (consumes §4.2): an
always-present top bar + composer with @workarea/fanout routing
affordances, the Finished/Blocked/Working digest panel + one-line next
step + chips, the write-tool AwaitingApproval→ResolveApproval confirmation
chip (no bypass, R-2), and the budget-exhausted yellow banner + stale-digest
badge (R-7). Tier-2 double = mocked @tauri-apps/api invoke + component tests
(pnpm -C apps/desktop, the §7 web-ts override). Live Maestro.* data is Task
414; real live-Maestro rendering is the Phase-4 Tier-3 gate.

Refs: tasks/v1.0/415-desktop-maestro-chat-ui.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan** —
  - **`<MaestroChat>` mounted in `App.tsx`** (not as a top region of `AppLayout`): the App root `<div>` was changed to a `flex h-screen flex-col`; `<MaestroChat />` renders FIRST, then `<AppLayout />` inside a `min-h-0 flex-1` wrapper. This keeps the chat ALWAYS-PRESENT above the three-panel split and inside the existing `QueryClientProvider` (it uses React Query for the digest). `AppLayout.tsx` was NOT touched.
  - **The `Maestro.*` arms ARE in the `client.ts` `RpcMethod` union** (the preferred decision): `"Maestro.SendToMaestro" | "Maestro.GetDigest" | "Maestro.SetWorkareaVisibility"` added as a pure additive TS type with a comment that the matching Rust `src-tauri/src/commands.rs` dispatch arm is **Task 414's** work. `src-tauri` was NOT touched. Until 414 lands, the renderer drives these against a mocked `@tauri-apps/api` `invoke` double (the Tier-2 spine). NOTE: `client.ts` (the union) + `sessions.ts` (the `AwaitingApproval`/`resolveApproval`/`ApprovalDecision` reuse surface + the exported `oneofVariant` helper) arrived on this worktree base already staged (a prior partial run); they are in `Outputs` and verified correct against the frozen proto, kept verbatim.
  - **Field names reconciled against 401.5's COMMITTED `crates/proto/proto/concerto/v1/maestro.proto` (not the design sketch):** the wire chip message is **`MaestroChip`** (renamed from `Chip` to avoid the flat-package collision with `suggestions.proto`'s `Chip`); same six fields/numbers `rule_id`/`workarea_id`/`title`/`priority`/`created_at_ms`/`action`. The TS mirror type is named `MaestroChip` to match. `Digest = { text, chips: MaestroChip[], generated_at_ms, stale }`. `MaestroMessageRequest = { text, attachments: MaestroAttachment[] }` (attachments = empty R-9 seam in V1.0). `VisibilityRequest = { workarea_id, visibility }` with `MaestroVisibility { UNSPECIFIED=0, FULL=1, HARD_FACTS_ONLY=2 }`. All timestamps are **`int64` unix-ms plain numbers** (`generated_at_ms`/`created_at_ms`/`last_digest_at_ms`) — NOT `[seconds,nanos]` tuples, NOT `google.protobuf.Timestamp`. The Finished/Blocked/Still-working grouping is **textual inside `Digest.text`** (split for styling in `DigestPanel.splitDigestSections`), not a wire field. `MaestroState` (`enabled`/`daily_in_today`/`daily_out_today`/`last_digest_at_ms`) mirrors 401.5's Rust-side `MaestroStateView` read-model — it is NOT yet a `maestro.proto` message nor an exposed RPC (414 surfaces it), so the banners currently derive from the `maestro.events` `budget_exhausted`/`disabled_by_policy` frames; the `MaestroState`-counter path (`computeBannerLevel`) is ready for 414/412 to feed.
  - **No devDeps added:** `@testing-library/react`/`@testing-library/user-event`/`@testing-library/jest-dom`/`jsdom` were all already in `apps/desktop/package.json` devDependencies from 218; `package.json`/`pnpm-lock.yaml` were NOT modified. `pnpm -C apps/desktop install` reports "Already up to date".
- **Open questions for next task** — **Task 414** is the consumer that lights up this UI: it fills 401.5's `Status::unimplemented` `MaestroServer`, adds the `Maestro.*` arm to the Rust shell `src-tauri/src/commands.rs` dispatch table (the strings already typed in `client.ts`), and publishes `maestro.events` on `Event.checks_opaque=17`. **414 must match:** (1) the `Maestro.GetDigest` response = the `Digest` shape mirrored in `api/maestro.ts` (snake_case `text`/`chips`(`MaestroChip`)/`generated_at_ms`/`stale`); (2) each `maestro.events` opaque frame must JSON-decode to one of `decodeMaestroEvent`'s discriminated shapes — `message{text,role}` / `routing_executed{targets,summary}` / `digest_generated{digest}` / `budget_exhausted` / `disabled_by_policy{reason}` (the decoder reads dual PascalCase/snake_case via `oneofVariant` and degrades unknown frames to `{kind:"unknown"}`, so an added frame kind won't crash, but a renamed field would be silently dropped — keep these names). No field is known to be missing; `Digest.stale` + `generated_at_ms` are already present for the R-7 stale-badge. If 414 surfaces `MaestroState` via a new `Maestro.GetState`-style RPC, wire it into `<BudgetBanner state=… budget=…>` (the `computeBannerLevel` counter path is already implemented and tested).
- **Deliberate debt** — the live-data path is **mocked (`invoke` double) by design until 414**; the empty-state renders are deliberate UX seams (NOT `todo!()`): `DigestPanel` `digest-empty` ("No digest yet…") when `getDigest` rejects with `Status::unimplemented`; `MaestroTranscript` `transcript-empty` ("No messages yet…") while the `maestro.events` stream is empty (no producer until 414). Verified: NO `TODO`/`FIXME`/`todo!()`/`unimplemented!()` anywhere in new code. The per-workarea hard-fact summary rows in `DigestPanel` are an optional enhancement (the `summaryRows` prop, sourced from the Desktop's existing `Workareas.ListWorkareas` state, NOT the `Digest` wire) — currently not populated by `<MaestroChat>` (the digest body is `text`+`chips` per §4.2); a later task can feed it from React Query. Settings → Concerto Chat backend/budget config UI is deliberately out (provider seam is 402/412); 415 only READS state to render banners/badges.
- **Smoke-gate state** — unchanged; `apps/desktop/src`-only task (plus the pre-staged `client.ts`/`sessions.ts` TS-type additions), no smoke capability / Core / boot change touched; `scripts/smoke.sh` is the CI/operator gate for an `unchanged` task and was not re-run in-worktree. The §7 override gate is **all green**: `pnpm -C apps/desktop install` (up to date, no devDep added), `typecheck` (clean `tsc --noEmit`), `lint` (clean), `test` (vitest: 34 files / 179 tests pass, incl. the 6 new maestro suites — `maestro.ts` binding+event-decode, `DigestPanel`, `ConfirmationChip`, `BudgetBanner`, `MaestroComposer`, `useMaestroStore`), `build` (`tsc --noEmit && vite build` clean; the Monaco chunk-size warning is pre-existing).
