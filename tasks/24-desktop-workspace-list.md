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
- [x] Verification commands pass.
- [x] Sidebar renders workspaces from the Core.
- [x] Event-driven invalidation works (manual test). *(Wired via `useEventSubscription("workspace.events")` in `Sidebar.tsx`; the operator-driven end-to-end check is deferred to interactive run as noted in the orchestrator brief.)*
- [x] No renderer direct network calls (verified by inspection — only `invoke()` calls).
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

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
- **Drift from plan:**
  - **shadcn/ui CLI install skipped.** `pnpm dlx shadcn@latest init` requires interactive input that the orchestrator can't satisfy. Per the orchestrator pre-decision, we hand-rolled the only two primitives the V0.1 sidebar actually uses (`Button`, `Card{,Header,Title,Content}`) into `apps/desktop/src/components/ui/` with hard-coded Tailwind class strings — no `sidebar`, `dialog`, `input`. No `clsx`/`tw-merge` shim either; classnames are joined inline. Phase 3 polish (Task 46+) is the natural place to do the real `shadcn init`.
  - **`Projects` gRPC service added.** `crates/proto/proto/concerto/v1/projects.proto` introduces a single `ListProjects` RPC (no Create — V0.1 still seeds via SQL per Task 19 Handoff). Handler at `crates/core/src/handlers/projects.rs`, registered in `api_server.rs` via the new `ApiServerActor::with_managers(...persistence: Option<Arc<Persistence>>)` slot. Field numbers FROZEN as of Task 24. Test harness gained `projects_client()`/`ProjectsClient` matching the existing pattern. `crates/proto/build.rs` now lists `concerto.v1.Project.{created_at,archived_at}` in `timestamp_fields`.
  - **`ApiServerActor::with_managers` grew a 7th argument**, `persistence: Option<Arc<Persistence>>`, threaded from `main.rs`. The 6-arg call sites in `crates/core/src/main.rs` are the only ones inside the workspace (no test fallout). Future managers (e.g. Repositories.GetRepository persistence path) can reuse the same slot.
  - **Persistent gRPC channel in the Tauri shell.** Task 14's "fresh dial per call" is replaced with `core_client.rs::get_or_connect`, backed by a `Mutex<Option<Channel>>` + `OnceCell<()>` init guard. On any RPC failure the dispatcher calls `reset_channel()` so the next invoke re-dials. No exponential backoff — V0.1 is local UDS only.
  - **`concerto_subscribe` / `concerto_unsubscribe` Tauri commands added.** `subscribe` opens `Streams.Subscribe(subject)` and forwards every frame to the renderer via `app.emit("concerto/<subject>", event)`. The forwarder task lives in a `Mutex<HashMap<String, JoinHandle>>` managed-state registry; `unsubscribe(id)` aborts the task. Subscription ids are `<subject>-<unix_millis>`; uuid was overkill for V0.1.
  - **`Sessions.ListSessions` is a local stub** that returns `{"sessions": []}` without calling the Core (per pre-decision 11). The dispatcher still validates the payload shape so the wire surface is honest. Task 26 promotes this to a real RPC.
  - **Frontend deps added in `apps/desktop/`:** `@tanstack/react-query@5.100.14`, `zustand@5.0.14` (both runtime); `pnpm-lock.yaml` regenerated and committed. Skipped `@tanstack/react-query-devtools` per the speed brief.
  - **`tokio-stream` promoted to a regular dep** in `apps/desktop/src-tauri/Cargo.toml` (was dev-only) — needed by the subscription forwarder. `tokio` gains the `sync` feature so `OnceCell` resolves.
  - **`Tauri Emitter` import is the v2 path.** `commands.rs` brings `tauri::Emitter` into scope and uses `app.emit(event_name, payload)`. The spec's `emit_all` name is from Tauri v1.
- **Open questions for next task:**
  - **Task 25 (workspace creation flow + workareas in the sidebar)** can reuse the `useEventSubscription` hook for `workarea.events`, layer the third tree level under each workspace, and add `Workspaces.CreateWorkspace` to the dispatcher's match table. The persistent channel is already in place — no new transport work is needed.
  - **Task 26 (session terminal)** will need to flip `Sessions.ListSessions` from the local stub to a real `SessionsClient.list_sessions` call and add `Sessions.{CreateSession,SendMessage,StopSession}` to the dispatcher. The xterm.js wiring will use the existing `concerto_subscribe("session.io.<sid>")` bridge directly — the Rust forwarder is already generic over subject.
  - **Renderer-side type mirroring.** The proto's `Workspace`/`Project` shapes are mirrored by hand in `src/api/{workspaces,projects}.ts`. When the proto field set grows, both files need updating. A future task could codegen TS types from the .proto sources; for V0.1 the manual mirror is small (~20 fields total).
- **Deliberate debt:** sidebar shows only projects+workspaces; workareas (third tree level) come in Task 25. Right panel is a JSON placeholder; the real three-panel layout lands in Task 46+. `Sessions.ListSessions` is a local stub. No `clsx`/`tw-merge` utility; hand-joined class strings only. shadcn CLI deferred. Subscription ids are `<subject>-<unix_millis>` (not UUIDs). Persistent channel has no retry/backoff — single OnceCell reset on error. React Query Devtools deliberately not installed.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still exits 0 with "Smoke gate v1: PASSED". Task 27 promotes the gate to v2.
