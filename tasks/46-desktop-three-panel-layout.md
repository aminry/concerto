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
- [ ] Verification commands pass.
- [ ] Three-panel layout matches the design doc's diagram.
- [ ] Layout state persists.
- [ ] Right-rail tabs render (real or stub per scope).
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

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
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** Chat sub-tab placeholder; Concerto chat top-bar V1.0; command palette V1.0.
- **Smoke-gate state:** unchanged.
