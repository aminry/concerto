# Task 24 — Desktop Workspace List Screen

| Field | Value |
|---|---|
| Phase | 2 |
| Size | medium (1–3d) |
| Depends on | 14, 19 |
| Touches subsystem(s) | 15 (Desktop) |
| Smoke gate | unchanged |

## Goal
Build the first real React UI: a sidebar listing projects + their workspaces (the top two levels of the 3-level tree in `design/15 §3.4`). After this task, the Desktop window shows a sidebar populated from `Workspaces.ListWorkspaces`, and clicking a workspace selects it (right side shows JSON detail for now — full center panel arrives in Task 30+).

## Inputs to read before starting
- `design/15_Desktop_Client.md` §3.3 (state management — Zustand for UI, React Query for data, server events for invalidation), §3.4 (three-panel layout — top half: sidebar tree; just sidebar for V0.1), §5.1 (Tauri command surface), §6.2 (subscription multiplexer).
- `design/00_Architecture_Overview.md` §6.8 (shadcn/ui + Tailwind locked).
- `tasks/19-workspace-creation.md` and `tasks/23-sessions-grpc-service.md` → "Handoff Notes".

## Scope — in
- Install React Query (`@tanstack/react-query`), Zustand, shadcn/ui (`pnpm dlx shadcn@latest init` + add `button`, `sidebar`, `card`, `dialog`, `input`).
- Extend `apps/desktop/src-tauri/src/commands.rs` to add a method dispatcher in `concerto_rpc`:
  - `"Workspaces.ListWorkspaces"` with `payload: { project_id?: string }` → gRPC call.
  - `"Workspaces.GetWorkspace"` with `payload: { id: string }`.
  - `"Sessions.ListSessions"` with `payload: { workarea_id: string }` (for future use; stub if needed).
  - The other methods from Tasks 13, 18, 19, 20, 22 must already be reachable — extend the dispatcher's match table.
- Add a `concerto_subscribe(subject, filter)` Tauri command that opens a `Streams.Subscribe` server-stream on the Rust side; forwards each event to the renderer via `app.emit_all(concerto/<subject>, event)`. Returns a `SubscriptionId` so the renderer can `concerto_unsubscribe` it.
- React app structure:
  - `src/state/` — Zustand stores: `useUiStore` (sidebar collapsed, selected workspace ID), `useDraftStore` (later, placeholder).
  - `src/api/` — typed wrappers around `invoke('concerto_rpc', ...)`. One function per RPC: `listWorkspaces(projectId?)`, `getWorkspace(id)`, etc.
  - `src/hooks/` — React Query hooks: `useWorkspaces()`, `useWorkspace(id)`.
  - `src/hooks/useEventSubscription.ts` — subscribes to a subject via Tauri event; invalidates the relevant React Query cache key on each event.
  - `src/components/Sidebar.tsx` — renders the project + workspaces tree using shadcn `Sidebar`.
  - `src/components/WorkspaceDetail.tsx` — placeholder right panel showing `<pre>{JSON.stringify(workspace, null, 2)}</pre>`.
  - `src/App.tsx` — split layout: sidebar on left, detail panel on right. Hard-coded single project (V0.1 doesn't yet have a project-creation UI; the developer creates one via SQL or a test fixture).
- React Query setup: `QueryClient` with default `staleTime: 1000 * 30`, `gcTime: 1000 * 60 * 5`.
- Subscribe to `workspace.events` at app mount; invalidate `['workspaces']` query on every event.
- A small "Refresh" button on the sidebar manually invalidates the query.

## Scope — out
- Workareas in the sidebar (Task 25 adds the third tree level).
- Sessions (Task 26).
- Workspace creation UI (Task 25).
- Real shadcn theming / dark mode polish (V1.0).
- Right rail (V1.0).
- Top-bar Concerto chat (V1.0).
- Three-panel layout center region — V0.1 just has sidebar + detail.

## Public interface this task locks
- Tauri command method-string convention: `"<Service>.<Method>"` (matches Task 14). Every gRPC method exposed to the renderer follows this pattern.
- Subscription event channel: `concerto/<subject>` on the Tauri event bus.
- Renderer state: Zustand for UI ephemera, React Query for server state. No Redux, no IndexedDB.

## Implementation notes
- The Rust shell keeps a persistent gRPC channel after this task. Replace Task 14's "fresh connection per call" with a lazy `OnceCell<Channel>`. Refresh on disconnect.
- Tauri events: use `app.emit_all` from Rust; `listen` on the React side via `@tauri-apps/api/event`.
- Shadcn install: it scaffolds into `src/components/ui/`. Use these as-is; don't customize until the Phase 3 polish task.
- React Query Devtools (optional dev-only) helps debugging — keep out of production bundle.
- For now the "current project" is hardcoded — display the first project from `SELECT * FROM projects LIMIT 1` via a placeholder `Projects.GetCurrent` gRPC method, OR just hardcode the project_id in the renderer for V0.1 (document in Handoff). The cleaner option: add a tiny `Projects.ListProjects` RPC to the proto in this task (the proto has no Projects service yet because Task 19 deferred it). Choose the cleaner option.

## Verification
1. `cd apps/desktop && pnpm install && pnpm tauri build --debug` → succeeds (verifies the renderer builds).
2. `cargo check --workspace` → clean.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual end-to-end: start Core in one terminal; manually insert a project row + a workspace via the smoke-client (Task 15 extended); start Desktop; verify the sidebar shows the workspace.
5. Manual: archive the workspace from the smoke client; verify the sidebar updates (via the `workspace.events` subscription).
6. `./scripts/regen-interfaces.sh && git diff` → if a Projects RPC was added, commit the regenerated interfaces.
7. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] Sidebar renders workspaces from the Core.
- [ ] Event-driven invalidation works (manual test).
- [ ] No renderer direct network calls (verified by inspection — only `invoke()` calls).
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `apps/desktop/package.json` (modified — React Query, Zustand, shadcn deps)
- `apps/desktop/src/state/useUiStore.ts` (new)
- `apps/desktop/src/api/client.ts` (new)
- `apps/desktop/src/api/workspaces.ts` (new)
- `apps/desktop/src/api/projects.ts` (new)
- `apps/desktop/src/hooks/useWorkspaces.ts` (new)
- `apps/desktop/src/hooks/useEventSubscription.ts` (new)
- `apps/desktop/src/components/Sidebar.tsx` (new)
- `apps/desktop/src/components/WorkspaceDetail.tsx` (new)
- `apps/desktop/src/components/ui/*` (new — shadcn components)
- `apps/desktop/src/App.tsx` (modified)
- `apps/desktop/src-tauri/src/commands.rs` (modified — wider dispatcher + subscription handler)
- `apps/desktop/src-tauri/src/core_client.rs` (modified — persistent channel)
- Possibly `crates/proto/proto/concerto/v1/projects.proto` (new) + handler
- `docs/interfaces/proto.md` (regenerated)

## Commit message
```
phase-2: desktop workspace list with event-driven invalidation

React Query + Zustand wired through Tauri commands. Sidebar renders
projects + workspaces from Core. Streams.Subscribe(workspace.events)
invalidates the cache live. shadcn/ui scaffolded.

Refs: tasks/24-desktop-workspace-list.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** sidebar shows only projects+workspaces; workareas (third tree level) come in Task 25. Right panel is a JSON placeholder.
- **Smoke-gate state:** unchanged.
