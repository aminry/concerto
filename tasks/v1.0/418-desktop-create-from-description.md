# Task 418 — Desktop: create-workspace-from-description flow + cone picker (the §3.8 Maestro create UX)

| Field | Value |
|---|---|
| Phase | 4 (UI-completion addendum) |
| Task type | web-ts |
| Verification tier | 2 |
| Size | medium |
| Depends on | 411, 415 |
| Touches subsystem(s) | 15 (Desktop), 08 (Maestro), 02 (Repo Mgr) |
| Smoke gate | unchanged |

## Goal
Build the Desktop front door for `design/08 §3.8` "spawn a workspace + first workarea from a natural-language description / issue link" — the headline Maestro create flow that Task 411 shipped the backend for (`Repositories.SuggestCones` RPC + `create_workspace_from_description` + the issue-fetch + the confirmation-chip slate) but that has **no UI** today: the audit found no `SuggestCones` arm in `client.ts`, no `suggestCones` binding, and no entry point — and Task 411 explicitly assigned this Desktop UX to 415, which never built it. This task adds: a **`Repositories.SuggestCones` binding**; an **entry point** (a "Create from description / issue link" action — in the New-Workspace modal and/or the Maestro chat) where the user pastes a description or issue URL; rendering of the Maestro's **confirmation chip slate** (the §3.8 step-4 options: "create workspace + first workarea" / "just the workspace" / "edit repo set / cones"); and a **cone picker** for the suggested cones (reusing the existing `ConePicker` component). On confirm it drives the create through the Maestro write tool (`create_workspace_from_description` via `SendToMaestro`, or the existing create path) — **never a silent create** (R-2). After this task a user can turn "add SSO to the API and the iOS app — LINEAR-123" into a confirmed multi-repo workspace with sensible sparse cones, from the Desktop. `apps/desktop` only; no proto, no Rust.

## Inputs to read before starting
- `tasks/v1.0/411-create-workspace-from-description.md` → "Public interface" + Handoff — the FROZEN `Repositories.SuggestCones` RPC (`SuggestConesRequest{repository_id, issue_text}` / `SuggestConesResponse{cone_paths}`), the `create_workspace_from_description` planner flow (issue parse → multi-repo detect → cone suggest → chip slate → confirm), and what 411 says the Desktop should consume.
- `crates/proto/proto/concerto/v1/repositories.proto` (the `SuggestCones` RPC shape) + `apps/desktop/src/api/repositories.ts` + `apps/desktop/src/api/client.ts` (add `"Repositories.SuggestCones"` to the `RpcMethod` union; 411's Rust arm is merged) — mirror the existing `EstimateConeSize`/`SetCones` bindings.
- `apps/desktop/src/components/ConePicker.tsx` (+ `SparseConeDialog.tsx`, `cones.ts`) — the existing cone-selection UI to **reuse** for the suggested cones (do not build a new cone widget).
- `apps/desktop/src/components/NewWorkspaceModal.tsx` + `WorkspaceForm.tsx` (the "Description" field at `WorkspaceForm.tsx:258` is currently a plain label) + `apps/desktop/src/components/bootstrapWorkspace.ts` — the existing create-workspace flow + where the new "from description / issue link" entry point and step-through mount.
- `apps/desktop/src/components/maestro/MaestroChat.tsx` + `DigestPanel.tsx` (the chip rendering, `MaestroChip` shape) — if the create slate surfaces as Maestro chips in the chat (the §3.8 design), reuse the chip rendering; otherwise a dedicated modal stepper. **Pick one coherent UX and document it.**
- `apps/desktop/src/api/maestro.ts` + `tasks/v1.0/406-maestro-write-tools.md`/`407` Handoff — how `create_workspace`/`create_workarea`/`create_workspace_from_description` are invoked (the write-tool path + the confirmation-chip gate). The create MUST go through the confirm gate (no silent create).
- `tasks/v1.0/415-desktop-maestro-chat-ui.md` — the established `apps/desktop` Maestro UI conventions (React-Query-canonical, Zustand UI-only, mocked-invoke tests, the §7 verification override).

## Scope — in
- **`api/repositories.ts` + `client.ts`:** add `suggestCones(repositoryId, issueText): Promise<string[]>` over `callRpc("Repositories.SuggestCones", …)` + the `RpcMethod` arm.
- **Entry point:** a "Create from description / issue link" affordance — a button/option in `NewWorkspaceModal` (a description/issue-URL textarea) and/or a Maestro-chat command — that kicks off the §3.8 flow. The flow shows: detected repo subset (editable — never auto-pick silently), per-repo suggested cones (via `suggestCones`, in the reused `ConePicker`), and the confirmation step.
- **Confirmation slate (R-2):** render the §3.8 step-4 options as actionable chips/buttons ("Create workspace + first workarea" / "Just the workspace, no workarea" / "Edit repo set / cones"). On confirm, invoke the create through the Maestro write-tool path (`create_workspace_from_description` / `create_workspace`+`create_workarea`); on "edit", let the user adjust the repo set + cones before confirming. **No silent create** — creation only on explicit confirm.
- **Tests (Tier 2, mocked `invoke`):** `suggestCones` binding shape; the flow renders detected repos + suggested cones from a mocked response; "Edit repo set" lets the user change the selection; the confirm action invokes the create call (mocked) with the chosen repos/cones; a no-URL freeform description still reaches the confirm step (no silent create). Mirror `cones.test.ts`/`NewWorkspaceModal.test.tsx`/`maestro.test.ts`.

## Scope — out
- The `SuggestCones` RPC + `create_workspace_from_description` planner + issue fetch + the `MaestroConeSuggester` — **Task 411** (merged; consumed here).
- The confirmation-chip *producer* for generic write tools + the live budget meter + the visibility toggle — **Task 417** (sibling). If both tasks touch `client.ts`/`api/maestro.ts`, keep additions in distinct regions; the orchestrator serializes them on those files.
- Real issue fetch + real cone-suggestion quality — **Tier-3** (Phase-4 checklist "create a workspace from a real issue link"); this task's double is mocked `invoke` + a stubbed `suggestCones` response.

## Public interface this task locks
- No wire contract (consumes 411's `Repositories.SuggestCones`). TS surface is renderer-local.

## Implementation notes
- **Reuse `ConePicker`** — do not build a second cone widget; the suggested cones from `suggestCones` seed the existing picker so the user edits from a smart default.
- **Never silent (R-2/§3.8).** The whole point of the flow is that the user confirms the plan (repos + cones + create-workarea?) before anything is created. The create call fires only on the explicit confirm chip/button.
- **Pick one coherent UX** (modal stepper vs. in-chat chip slate) and keep it consistent with 415's Maestro chat conventions; document the choice in Handoff. The in-chat chip slate is closest to the design, but a modal stepper inside `NewWorkspaceModal` is acceptable if cleaner — either way it must be discoverable.
- **§7 verification override:** `pnpm -C apps/desktop typecheck|lint|test|build`; mocked-invoke Tier-2 double.

## Verification
**Tier 2 — §7 override.**
1. `pnpm -C apps/desktop install`.
2. `pnpm -C apps/desktop typecheck` — clean.
3. `pnpm -C apps/desktop lint` — clean.
4. `pnpm -C apps/desktop test` — the new flow tests + existing suites green.
5. `pnpm -C apps/desktop build` — clean.
6. `scripts/smoke.sh` — unchanged.

**Tier-2 double + what it does NOT cover:** mocked `invoke` + a stubbed `suggestCones`/issue-fetch prove the flow wiring (detect → suggest → edit → confirm → create) + the never-silent invariant. It does NOT cover a real GitHub/Linear issue round-trip or real cone quality — the Phase-4 Tier-3 checklist.

## Definition of Done
- [x] `Repositories.SuggestCones` binding + `RpcMethod` arm
- [x] Discoverable "create from description / issue link" entry point; flow shows editable detected repos + suggested cones (reused `ConePicker`)
- [x] Confirmation slate (§3.8 options) drives create ONLY on explicit confirm (no silent create, R-2)
- [x] Tier-2 tests pass (`pnpm -C apps/desktop typecheck|lint|test|build`); smoke unchanged
- [x] No TODO/FIXME/unimplemented in new code; no `src-tauri`/proto/Rust; only `apps/desktop/**`
- [x] Single commit with the message below

## Outputs
- `apps/desktop/src/api/repositories.ts` (+ `suggestCones`) + `apps/desktop/src/api/client.ts` (+ `Repositories.SuggestCones`)
- new create-from-description component(s) under `apps/desktop/src/components/` (e.g. `CreateFromDescription.tsx` + test) and/or additions to `NewWorkspaceModal.tsx` / `MaestroChat.tsx`
- reuse of `ConePicker.tsx` (no new cone widget)

## Commit message
```
phase-4: desktop create-workspace-from-description flow + cone picker

Builds the §3.8 Maestro create front door (Task 411's backend, no UI until
now): a Repositories.SuggestCones binding + a "create from description /
issue link" entry point that detects the repo subset, suggests sparse cones
(reusing ConePicker), and renders the confirmation slate — creating the
workspace + first workarea ONLY on explicit confirm (no silent create,
R-2). apps/desktop only; mocked-invoke Tier-2; real issue fetch + cone
quality stay Tier-3.

Refs: tasks/v1.0/418-desktop-create-from-description.md
```

## Handoff Notes (filled in when finishing)
- **UX choice + drift from plan:** **A modal STEPPER inside `NewWorkspaceModal`, not in-chat chips.** `NewWorkspaceModal` now has a segmented mode toggle ("Build manually" → the existing `WorkspaceForm`; "From description / issue link" → the new `CreateFromDescription` stepper). The stepper is a 4-step flow: (1) describe / paste an issue URL (deterministic `detectIssueUrl` surfaces a detected link, zero LLM tokens); (2) editable detected-repo subset over the global registry, pre-checked by a deterministic `detectRepos` name-match (never an auto-pick that bypasses the user); (3) per-repo suggested cones via `suggestCones`, seeding the **reused `ConePicker`** (no second cone widget); (4) the §3.8 confirmation slate — three explicit buttons ("Create workspace + first workarea" / "Just the workspace, no workarea" / "Edit repo set / cones"). **Drift:** the task allows driving the create through the Maestro write-tool path (`SendToMaestro` / `resolve_create_plan`); I deliberately drive it through the **existing renderer create path** (`createWorkspace` + the shared `bootstrapWorkspace`) instead. Rationale: (a) it stays entirely out of `MaestroChat.tsx` / `api/maestro.ts`, which sibling Task 417 is rebuilding — zero collision; (b) the never-silent R-2 invariant is satisfied structurally (steps 1–3 spend NO create side effects; create fires only on an explicit slate button, no skip-confirm path); (c) `Maestro.SendToMaestro` returns `Empty` and the live emitter/`resolve_create_plan` round-trip is 414's unmerged work, so a chip-driven create has no testable Tier-2 double today, whereas the renderer create path is fully mocked-`invoke` provable. The `Repositories.SuggestCones` wire surface (411's backend) is consumed exactly as frozen.
- **Open questions for next task — what 417 must coordinate on:** I added exactly ONE arm to the shared `apps/desktop/src/api/client.ts` `RpcMethod` union — `"Repositories.SuggestCones"` — placed in its **own clearly-commented Task-418 region INSIDE the `Repositories.*` block (right after `Repositories.SetRepoConeDefaults`), well ABOVE the `Maestro.*` block** (the `Maestro.SendToMaestro` / `GetDigest` / `SetWorkareaVisibility` arms, lines ~76–87). **417 should keep its `client.ts` additions in the `Maestro.*` region (or its own distinct region) and NOT touch the `Repositories.*` block** — the two edits are then non-adjacent and a rebase auto-merges. **I did NOT touch `apps/desktop/src/api/maestro.ts` at all** (no shared edit there). If 417 later wants the create-from-description flow to surface as in-chat Maestro chips instead of (or alongside) the modal stepper, the `CreateFromDescription` component is self-contained and re-mountable inside the chat with no API change — the `suggestCones` binding + the deterministic `detectIssueUrl`/`detectRepos` exports are reusable as-is.
- **Deliberate debt:** The issue-link is DETECTED and displayed but NOT fetched in the renderer — the real issue round-trip (GitHub/Linear/Jira) + real cone-suggestion quality stay the Phase-4 **Tier-3** line ("create a workspace from a real issue link"). The Desktop double is mocked `invoke` + a stubbed `suggestCones`. `detectRepos` is a deterministic name-keyword matcher (a smart pre-check the user always edits), not the server-side multi-repo intent detector. A `suggestCones` rejection (e.g. UNIMPLEMENTED on a suggester-less Core) degrades to an empty cone seed + manual entry rather than blocking the flow.
- **Smoke-gate state:** Unchanged (web-ts, no `src-tauri`/proto/Rust). §7 override gate all green on a clean install: `pnpm -C apps/desktop typecheck` clean, `lint` clean, `test` = **191 passed / 35 files** (incl. 9 new `CreateFromDescription` tests + 3 new `suggestCones` binding tests; existing `NewWorkspaceModal` 8 tests still green — the mode toggle defaults to `manual`), `build` clean (only the pre-existing Monaco chunk-size warning). No new devDeps; `package.json`/lockfile unchanged.
