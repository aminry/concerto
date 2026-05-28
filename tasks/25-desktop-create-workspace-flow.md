# Task 25 — Desktop Create-Workspace + Workarea Flow

| Field | Value |
|---|---|
| Phase | 2 |
| Size | medium (1–3d) |
| Depends on | 18, 20, 24 |
| Touches subsystem(s) | 15 (Desktop) |
| Smoke gate | unchanged |

## Goal
Add the UI to create a workspace (modal) and a workarea inside it (button on the workspace detail). After this task, a user can: open Desktop → click "New Workspace" → pick a repo + name → click "+ New Workarea" → see the new workarea appear in the sidebar tree. Sidebar now shows all three levels (project → workspace → workarea).

## Inputs to read before starting
- `design/15_Desktop_Client.md` §3.4 (3-level sidebar tree), §3.13 (first-run dependency check — V0.1 skips most; just probe `gh`+`claude` presence).
- `design/03_Workspace_Session_Manager.md` §3.2 (workspace creation: pick name + slug + repos), §3.3 (workarea creation).
- `tasks/24-desktop-workspace-list.md` → "Handoff Notes".

## Scope — in
- Add a "New Repository" flow (the simplest path to get a repo into the project): Settings → Project (placeholder Settings page) shows an "Add Repository" button. The form takes a git URL, calls `Repositories.AddRepository` + `Repositories.Clone` (streaming progress as a small progress bar). On done, the repo appears in a list.
- Add a "New Workspace" modal triggered by a button in the sidebar:
  - Inputs: name (text), repository picker (single-select; V0.1 enforces 1 repo per workspace), optional description.
  - On submit: calls `Workspaces.CreateWorkspace`; closes; sidebar auto-updates via the existing `workspace.events` subscription from Task 24.
- Add a "+ New Workarea" button on the workspace detail panel (right side):
  - On click: calls `Workareas.CreateWorkarea` for the current workspace. No additional form input in V0.1 — the workarea uses default settings.
  - The new workarea appears under its workspace in the sidebar tree (third level).
- Extend the sidebar to render the third level:
  - When a workspace is expanded, list its workareas (call `Workareas.ListWorkareas(workspace_id)`).
  - Each workarea shows: composer name + branch chip + status dot.
  - Status dot colors per `design/15 §3.4`: green = active, amber = awaiting, blue = running, grey = archived/finished.
- Subscribe to `workarea.events` and invalidate `['workareas', workspaceId]` on each.
- Selecting a workarea (click) updates `useUiStore.selectedWorkareaId`. The right panel shows `<pre>{JSON.stringify(workarea)}</pre>` for now — terminal panel arrives in Task 26.
- Add a minimal first-run check: on Desktop launch, check that `claude` is on PATH (`which claude` via a Tauri command); if missing, show a non-blocking toast "Install Claude Code to use sessions." Skip the full checklist from `design/15 §3.13` for V0.1.

## Scope — out
- Sparse cone picker (V1.0 — no sparse in V0.1).
- Multi-repo workspace creation (V1.0).
- Workspace-from-Linear-issue (V1.0 — needs Maestro).
- Workarea branch name input (uses auto `concerto/<composer>`; rename hook is V1.0).
- Full first-run setup screen (V1.0 — V0.1 just toasts).
- Drag-to-reorder, archive UI, history pane (Phase 3 / V1.0).

## Public interface this task locks
- `useUiStore` exposes `selectedProjectId`, `selectedWorkspaceId`, `selectedWorkareaId`. Frozen as the canonical selection state.
- Form contract: the "New Workspace" modal validates name non-empty, repo selected, before enabling submit. Slug derivation is server-side (Task 19).

## Implementation notes
- Use shadcn `Dialog` for modals.
- Use React Query mutations for `CreateWorkspace` and `CreateWorkarea`; on success, invalidate the relevant list queries.
- The first-run `which claude` probe: a small Tauri command `check_command(name: String) -> Result<Option<String>>` that runs `which <name>` (or `where` on Windows) and returns the path or None.
- Don't fetch workareas eagerly for every workspace — only when the workspace tree node is expanded.
- The "Add Repository" form's progress bar: subscribe to the `Clone` server-stream's `CloneProgress` messages and update a `<Progress>` component. Don't block the modal.

## Verification
1. `pnpm tauri build --debug` → succeeds.
2. `cargo clippy --workspace -- -D warnings` → clean.
3. Manual end-to-end:
   - Start Core (clean tempdir).
   - Start Desktop.
   - Add a local bare-repo via Add Repository (use `file://` URL of a local bare repo created by hand for testing).
   - Watch clone progress complete.
   - Create a workspace named "Test 1", picking the just-added repo.
   - Workspace appears in sidebar.
   - Click workspace; click "+ New Workarea"; workarea appears under workspace.
   - Click workarea; right panel shows its JSON.
4. With Desktop running, archive the workarea via smoke-client; verify sidebar updates.
5. With `claude` removed from PATH, restart Desktop; verify the toast appears.
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [x] Verification commands pass.
- [x] Three-level sidebar tree renders correctly.
- [x] Workspace + workarea creation works end-to-end.
- [x] Event-driven updates propagate to sidebar.
- [x] First-run claude probe shows toast on missing CLI.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `apps/desktop/src/components/NewWorkspaceModal.tsx` (new)
- `apps/desktop/src/components/AddRepositoryForm.tsx` (new)
- `apps/desktop/src/components/Sidebar.tsx` (modified — third level)
- `apps/desktop/src/components/WorkareaDetail.tsx` (new)
- `apps/desktop/src/components/SettingsPanel.tsx` (new — placeholder + Add Repository)
- `apps/desktop/src/api/workareas.ts` (new)
- `apps/desktop/src/api/repositories.ts` (new)
- `apps/desktop/src/hooks/useWorkareas.ts` (new)
- `apps/desktop/src/state/useUiStore.ts` (modified)
- `apps/desktop/src-tauri/src/commands.rs` (modified — adds check_command, expands dispatcher)
- `apps/desktop/src/App.tsx` (modified — routes detail panel by selection)

## Commit message
```
phase-2: desktop workspace + workarea creation

New Workspace modal + New Workarea button. Sidebar now renders the
full 3-level tree (project → workspace → workarea) with status dots.
Add Repository flow with streaming clone progress. First-run toast
when claude isn't on PATH.

Refs: tasks/25-desktop-create-workspace-flow.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **shadcn CLI still skipped.** Per the orchestrator pre-decision (and continuing Task 24's drift), `Dialog`, `Input`, and `Progress` are hand-rolled in `apps/desktop/src/components/ui/` with hard-coded Tailwind class strings. No `@radix-ui/react-dialog`, no `clsx`/`tw-merge`. Phase 3 polish (Task 46+) is the natural place to swap in the real shadcn set.
  - **`Repositories.ListByProject` RPC added to proto.** The Desktop "Add Repository" form needs to render the existing repos so the New Workspace modal's picker has data. The new `ListRepositoriesRequest` / `ListRepositoriesResponse` messages and the third Repositories RPC are appended to `crates/proto/proto/concerto/v1/repositories.proto`; existing field numbers and RPC ordering are untouched. Field numbers FROZEN as of Task 25 for the new messages. The handler reuses the existing `RepoManager` (added `list_by_project` passthrough) so no new persistence path was needed.
  - **`clone_repository` Tauri command added** as a dedicated streaming bridge for the `Repositories.Clone` server-stream. The generic `concerto_subscribe` bridge speaks `Streams.Event` keyed by subject; clone is a typed `CloneProgress` stream with no pub/sub subject. The command drains the stream and emits each frame as `concerto/clone-progress/<repository_id>`. Resolves once the stream terminates. UFCS (`RepositoriesClient::<Channel>::clone`) is required because the generated method name collides with `Clone::clone`.
  - **`check_command` Tauri command added** — runs `which <name>` (Unix only). Rejects names containing `/`, `\`, newline, or NUL up front so the surface is predictable. Windows path (`where`) deferred to V1.0 per the orchestrator pre-decision.
  - **`tokio` gains the `process` feature** in `apps/desktop/src-tauri/Cargo.toml` to back `check_command`. No other dep adds.
  - **`useUiStore` grew `selectedWorkareaId`, `expandedWorkspaces: Set<string>`, `newWorkspaceModalOpen`, `settingsOpen`** plus the toggle/setters. `setSelectedWorkspace` now clears `selectedWorkareaId` (selection invariants: picking a workspace drops the active workarea). The selection trio (`selectedProjectId`, `selectedWorkspaceId`, `selectedWorkareaId`) is the locked canonical UI selection state.
  - **Repository list rendering lives in `AddRepositoryForm.tsx`** rather than a separate `RepositoryList.tsx`; the form and the list share the same `["repositories", projectId]` cache key and clone-progress map, so co-locating them keeps the state thin.
  - **Workspace creation expansion side-effect.** `WorkspaceDetail.tsx`'s "+ New Workarea" mutation calls `setWorkspaceExpanded(id, true)` on success so the new workarea is immediately visible under its parent. The workarea then appears on the next `workarea.events` invalidation pass.
- **Open questions for next task:**
  - **Task 26 (session terminal)** plugs into `WorkareaDetail.tsx`: when `selectedWorkareaId` is set, the JSON `<pre>` is replaced by the xterm.js panel. The detail-panel routing in `App.tsx::DetailRouter` already prefers workarea > workspace, so Task 26 only needs to swap the panel body, not the route logic.
  - **`Repositories.Clone` re-clone behavior.** V0.1 lets the user kick off a clone for a repo whose `last_fetch_at` is already populated; the gix-wrap layer will likely fail with "destination already exists". The UI shows the resulting `CloneClientError::Rpc` string. Task 28 (fsmonitor + maintenance) is the natural place to add an explicit "is already cloned" check or to route a re-clone through a separate Fetch RPC.
  - **First-run probe binds to `claude` only.** `design/15 §3.13` lists `gh`, `claude`, and others. V0.1 only probes `claude` per the task spec; the toast surface is generic (`FirstRunClaudeToast`) and Task 33+ can extend it to a checklist.
- **Deliberate debt:** workarea detail panel shows JSON; terminal arrives Task 26. Full first-run setup screen V1.0. Repository list and Add Repository form co-located in one component. Workarea branch name is auto-derived; no rename hook. Clone-progress event channel is per-repo; no global "all clones" feed. Settings panel is single-section (Add Repository) — full settings tree is V1.0. Hand-rolled Dialog/Input/Progress primitives carry forward the Task 24 shadcn-skip drift. New Workspace modal enforces single-repo per the V0.1 server contract (multi-repo is V1.0).
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still exits 0 with "Smoke gate v1: PASSED". Task 27 promotes the gate to v2.
