# Task 46 — Desktop Three-Panel Layout

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 25, 26 |
| Touches subsystem(s) | 15 (Desktop) |
| Smoke gate | unchanged |

## Goal
Replace the V0.1 sidebar+detail layout with the full three-panel layout from `design/15 §3.4`: left sidebar with the 3-level tree, center panel with session tabs + Code & PRs region, right rail. After this task, the Desktop looks like the design doc's diagram (V0.1 polish — not every right-rail tab works yet; placeholders are fine).

## Inputs to read before starting
- `design/15_Desktop_Client.md` §3.4 (full layout diagram + behavior).
- `design/15_Desktop_Client.md` §3.5 (Monaco diff is Task 47 — placeholders here).

## Scope — in
- Refactor `App.tsx` into a three-column flexbox: sidebar (resizable, persists width in localStorage) + center + right rail (collapsible).
- Sidebar from Task 25 is preserved; ensure it's resizable.
- Center panel:
  - Header: workarea composer + branch chip + status dot.
  - Top region (default ~55% height): session tabs (one per session in the workarea). Within selected session: `Chat` / `Terminal` sub-tabs (V0.1 ships Terminal only; Chat is a stub placeholder card "Chat view comes in V1.0").
  - Bottom region (default ~45% height): per-repo tabs (`design/15 §3.4` "Code & PRs region"). V0.1 has single-repo workareas, so one tab per repo. Within each repo tab: `Diff` / `Checks` / `PR` sub-tabs (V0.1 stubs for Checks and PR — placeholder cards; Diff is real via Task 47).
- Right rail: vertical tab strip with collapsible drawer. V0.1 tabs:
  - `Scheduler` — list `/loop`s for the workarea (uses Task 38's `Schedules.ListSchedules`).
  - `Skills` — list enabled skills for the project (uses Task 39).
  - `Todos` — V0.1 stub.
  - `MCP` — list MCP servers (uses Task 35).
  - `Files` — V0.1 stub.
- Layout state (sidebar width, region heights, region collapsed booleans) persists in `localStorage`.
- Use shadcn `Resizable` panels (or a thin wrapper around `react-resizable-panels`).
- Tests:
  - Vitest unit tests for layout component (panel resize, persistence to localStorage).
  - Manual smoke: window resize stress test.

## Scope — out
- Concerto chat top bar (V1.0 — Maestro is V1.0).
- Cmd+K command palette (V1.0).
- Workflow Explorer / Diagnostics windows (V1.0).
- Multi-window detach (V1.0).
- Status bar (V1.0).

## Public interface this task locks
- Layout state schema in `localStorage`: `{ sidebarWidth: number, sessionRegionHeight: number, rightRailCollapsed: boolean, rightRailTab: string }`. Frozen as the V0.1 shape.

## Implementation notes
- `react-resizable-panels` ships with shadcn's resizable component; good default.
- Don't use `IframeWindow` or `Tab` from a non-shadcn library; stay with shadcn primitives to keep the bundle small.
- Persist layout state with a debounced (300ms) `localStorage.setItem`.

## Verification
1. `pnpm tauri build --debug` → succeeds.
2. `pnpm test` (Vitest) → unit tests pass.
3. `cargo check --workspace` → clean.
4. Manual: open Desktop, resize each panel, refresh, verify state persists.
5. Manual: select different workareas; verify each shows its own session tabs.
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [x] Verification commands pass. *(Vitest skipped per orchestrator pre-decision 6; `pnpm build` succeeds; `cargo check`, `clippy --all-targets -D warnings`, `cargo deny`, `cargo fmt` clean; `cargo test --workspace` green after one flaky-test retry on the pre-existing `hot_reconnect::adopts_surviving_host_after_supervisor_restart`.)*
- [x] Three-panel layout matches the design doc's diagram. *(`AppLayout` horizontal split = sidebar | center | right rail; `CenterPanel` vertical split = SessionRegion | CodePrRegion; right rail = vertical tab strip + collapsible drawer.)*
- [x] Layout state persists. *(`useLayoutPersistence` in `App.tsx` debounces 300ms then writes `LAYOUT_STORAGE_KEY` JSON.)*
- [x] Right-rail tabs render (real or stub per scope). *(Scheduler / Skills / MCP wired to live RPCs; Todos / Files are stub cards.)*
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green. *(`scripts/smoke.sh` → "Smoke gate v2: PASSED".)*
- [x] Single commit created.

## Outputs
- `apps/desktop/src/components/AppLayout.tsx` (new)
- `apps/desktop/src/components/CenterPanel.tsx` (new)
- `apps/desktop/src/components/RightRail.tsx` (new)
- `apps/desktop/src/components/right-rail/SchedulerTab.tsx`, `SkillsTab.tsx`, `TodosTab.tsx`, `McpTab.tsx`, `FilesTab.tsx` (new)
- `apps/desktop/src/components/center/SessionRegion.tsx`, `CodePrRegion.tsx` (new)
- `apps/desktop/src/App.tsx` (modified)
- `apps/desktop/src/state/useUiStore.ts` (modified — layout state)
- `apps/desktop/src/components/ui/resizable.tsx` (new via shadcn add)

## Commit message
```
phase-3: desktop three-panel layout

Sidebar (3-level tree) + center (session region + code/PR region) +
right rail (Scheduler/Skills/Todos/MCP/Files) per design/15 §3.4.
Resizable panels persist layout state to localStorage. Monaco diff
arrives Task 47.

Refs: tasks/46-desktop-three-panel-layout.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **shadcn `Resizable` swap-in skipped (orchestrator pre-decision 1).** `react-resizable-panels` is consumed directly via `pnpm add` — no shadcn CLI invocation, no new `apps/desktop/src/components/ui/resizable.tsx`. The library API (`PanelGroup` / `Panel` / `PanelResizeHandle`) is exactly what shadcn's wrapper exports, so the swap is a future textual rename if Phase 4 polish picks up the wrapper.
  - **Vitest unit tests skipped (pre-decision 6).** Adding `vitest` + `@testing-library/react` + jsdom would land ~30MB of JS test infra for a layout component that is already exercised by `pnpm build`. The persisted-state path is a pure function (`loadLayoutState` / `clampPercent` / `isRightRailTab`) inside `useUiStore.ts`; promoting it to a Vitest harness is a one-task lift when Phase 4 sets up the JS test infra.
  - **`Task 25` `WorkareaDetail.tsx` deleted.** Its body moved into `CenterPanel.tsx` (header) + `center/SessionRegion.tsx` (session tab strip + xterm panel + composer). The `useSessions` hook comment still references `WorkareaDetail` (kept on purpose — it's the historical anchor for the tab-strip-reads-from-this-hook rule).
  - **Three new dispatcher arms** (`Schedules.ListSchedules`, `Skills.ListSkills`, `Sessions.ListMcpServers`) added to `commands.rs::dispatch`. `SessionsClient` already lived in scope; `SchedulesClient` / `SkillsClient` are fresh `use` imports. Method strings follow the locked `"<Service>.<Method>"` convention.
  - **`useUiStore` grew four layout fields** (`sidebarWidth`, `sessionRegionHeight`, `rightRailCollapsed`, `rightRailTab`) plus matching setters. The store's `LAYOUT_STORAGE_KEY` constant (`"concerto.layout.v1"`) plus the four-field JSON shape is the locked V0.1 wire shape; loader (`loadLayoutState`) clamps numbers to 5..=95 and validates the tab enum so corrupt `localStorage` falls back to the design defaults (20% sidebar, 55% session region, drawer open on `scheduler`).
  - **Right-rail collapsed state is wired through a fixed `defaultSize` swap**, not a Panel `collapsible` prop. `react-resizable-panels` v2 supports `collapsible`, but the Task 46 surface needs a chevron-free "click the active tab to collapse" affordance — wiring that through `collapsible` would fight the controlled state. Instead, the rail Panel reads `rightRailCollapsed` from the store and renders a narrow 3% Panel (tab strip only) when collapsed.
  - **Right rail loses `repository_id`-scoped MCP discovery in V0.1.** The MCP tab queries the personal scope only because the V0.1 workarea selection doesn't surface a `repository_id` to the renderer (Task 25 picks one repo per workspace, but it doesn't propagate to the right-rail context). Project-scope MCP discovery lands when the right rail learns the active repo handle.
  - **Center-panel `CodePrRegion` is shape-only.** The per-repo tab strip currently shows the workarea's `branch_name` as the (single) repo label because the V0.1 wire surface doesn't expose the repository's `name` on the workarea response. Task 47 (Monaco diff) is the natural place to swap this for the real repo handle.
- **Open questions for next task:**
  - **Task 47 (Monaco diff)** plugs into `center/CodePrRegion.tsx`'s `Diff` sub-tab placeholder — replace the placeholder body with the Monaco panel. The sub-tab state (`activeSubTab`) is already wired; only the placeholder renderer for `case "diff"` needs to swap to the Monaco component. The repo tab label TODO above is the right moment to extend the workarea wire surface or add a `Repositories.GetRepository` fetch by id.
  - **Right-rail width is non-resizable in the collapsed state.** The collapsed Panel's `minSize === maxSize === 3` pins it; uncollapsing reverts to the configured range. If V1.0 wants the rail width to also persist across collapses, the store needs a separate `rightRailWidth` field — V0.1 keeps the schema thin with `rightRailCollapsed` only.
  - **`Schedules.CreateSchedule` / `PauseSchedule` / `DeleteSchedule`** are not wired through the dispatcher. The Scheduler tab is read-only in V0.1; the write path can land when the right-rail polish task adds the "+" buttons.
- **Deliberate debt:**
  - Chat sub-tab is a disabled placeholder ("Chat view comes in V1.0"); the Concerto-chat top-bar is V1.0; Cmd+K command palette is V1.0; Workflow Explorer / Diagnostics / multi-window detach / status bar are all V1.0 per the Scope — out section.
  - `Diff` / `Checks` / `PR` sub-tabs in `CodePrRegion` are stub placeholder cards. Diff arrives in Task 47; Checks + PR arrive with the V1.0 CI / VCS polish.
  - `Todos` and `Files` right-rail tabs are stub cards. Todos arrives with Maestro (V1.0); Files arrives with the filesystem allow/deny surface.
  - No Vitest test file. The layout persistence path is pure (loader + clamp + enum-check) and a single hand smoke confirms drag-resize then reload restores the layout; the next JS-test-infra task adds the harness.
  - 638KB JS bundle warning continues from Task 26 (xterm.js dominates). `react-resizable-panels` adds ~15KB to the bundle; no code-splitting yet.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still exits 0 with "Smoke gate v2: PASSED".
