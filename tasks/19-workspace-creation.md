# Task 19 — Workspace Creation API

| Field | Value |
|---|---|
| Phase | 2 |
| Size | medium (1–3d) |
| Depends on | 09, 18 |
| Touches subsystem(s) | 03 (Workspace Manager), 10 (Local API), 09 (Persistence) |
| Smoke gate | unchanged |

## Goal
Implement workspace creation end-to-end: a gRPC client calls `Workspaces.CreateWorkspace(project_id, name, repos)`, the server validates, persists `workspaces` + `workspace_repos` rows, and emits a `workspace.events: created` event. V0.1 ships single-repo workspaces only — multi-repo workspaces arrive in V1.0.

## Inputs to read before starting
- `design/03_Workspace_Session_Manager.md` §1 (purpose), §2 (V0.1 = single-repo workspaces only), §3.2 (workspace creation is logical — no disk yet), §5.1 (Rust API — workspace methods only for this task), §5.2 (gRPC service mapping), §6.1 (workspace creation sequence).
- `design/09_Persistence.md` §4.1 (workspaces / workspace_repos schema).
- `tasks/18-repository-cloning.md` → "Handoff Notes" (the `Repositories.AddRepository` flow must work).

## Scope — in
- Implement `crates/core/src/workspace_manager/` containing:
  - `WorkspaceManagerActor` implementing `Actor` from Task 12.
  - `create_workspace(req: CreateWorkspace) -> Result<WorkspaceId>` validates and persists.
- Persistence helpers in `crates/persist/src/workspaces.rs`:
  - `insert(tx, NewWorkspace) -> Result<WorkspaceId>`
  - `get(reader, id) -> Result<Option<Workspace>>`
  - `list_by_project(reader, project_id) -> Result<Vec<Workspace>>`
  - `archive(tx, id) -> Result<()>` (sets `archived_at`)
  - `update_repos(tx, id, repo_ids: Vec<RepositoryId>) -> Result<()>` (writes `workspace_repos`)
- Add the `Workspaces` proto service to `crates/proto/proto/concerto/v1/workspaces.proto` (the message `Workspace` already exists from Task 07; this task adds the service):
  ```proto
  service Workspaces {
    rpc CreateWorkspace(CreateWorkspaceRequest) returns (Workspace);
    rpc GetWorkspace(WorkspaceId) returns (Workspace);
    rpc ListWorkspaces(ListWorkspacesRequest) returns (ListWorkspacesResponse);
    rpc ArchiveWorkspace(WorkspaceId) returns (google.protobuf.Empty);
  }
  
  message CreateWorkspaceRequest {
    string project_id = 1;
    string name = 2;                   // user-supplied; server derives slug
    repeated string repository_ids = 3; // V0.1 enforces len == 1
    optional PermissionMode permission_mode = 4;
    optional string description = 5;
  }
  ```
- Slug derivation: lowercase, replace whitespace with `-`, strip non-`[a-z0-9-]`, max 64 chars, append `-N` on uniqueness collision within project (server-side).
- Implement `WorkspacesHandler` in `crates/core/src/handlers/workspaces.rs`.
- V0.1 validation:
  - `repository_ids.len() == 1` (return `INVALID_ARGUMENT` + `ConcertoError{code="workspace.v0_single_repo_only"}` otherwise).
  - `project_id` exists in `projects` (return `NOT_FOUND` if not).
  - All `repository_ids` exist + belong to the named `project_id`.
  - `name` non-empty and slug derives to non-empty.
- Emit `workspace.events: created` via a placeholder broadcast channel — full `Streams` service is later; for V0.1, store events in an in-process broadcast channel and accept that streaming subscription comes in Task 24.
- Add a minimal `projects` table CRUD in `crates/persist/src/projects.rs` (the `Projects` service isn't implemented in V0.1, but we need a way to insert a project row from tests):
  - `insert`, `get`, `list_all`. No gRPC surface yet.
- Integration test:
  - Use `test-harness`; insert a project via the persistence helper directly (no gRPC for projects); add a repository via Task 18's RPC; create a workspace via the new RPC; assert DB rows + the workspace can be re-fetched.

## Scope — out
- Multi-repo workspaces (V1.0 — but the schema allows it; we just enforce length 1 in the request).
- Workarea creation (Task 20).
- Permission-mode inheritance enforcement (Task 32 in Phase 3).
- The `Streams` service for subscriptions (later).
- Workspace project root path / files-to-copy (workareas own that — Task 30).

## Public interface this task locks
- Proto: `Workspaces.CreateWorkspace`, `.GetWorkspace`, `.ListWorkspaces`, `.ArchiveWorkspace`. Field numbers FROZEN.
- Rust: `crates/core/src/workspace_manager/mod.rs` exports the `WorkspaceManagerHandle` for use by Task 20 (workareas) and later.
- Rust: `crates/persist/src/projects.rs` and `crates/persist/src/workspaces.rs` are the canonical SQL surfaces — other tasks call into them, not into raw SQL.
- Slug derivation algorithm above is frozen.

## Implementation notes
- The `slug` derivation is deterministic and small enough to inline. Pattern from existing tools: `slug::slugify` crate exists but is overkill — write 20 lines.
- For "append `-N` on collision", do a SELECT-then-INSERT race-tolerantly: try INSERT, catch unique-constraint error, retry with incremented suffix. SQLite + serialized writer queue means contention is rare but possible.
- The broadcast channel for events: `tokio::sync::broadcast::channel(256)` for now. Future task (`Streams` service) consumes from it.
- `WorkspaceManagerActor` should NOT spawn anything heavy in `run()` — just hold the broadcast channel and a `PersistenceHandle`. Real workarea-creation work goes in Task 20.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core workspace_manager` → integration tests pass.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual: with Core running, use a test client to:
   - Insert project (direct SQL — there's no `Projects` service in V0.1).
   - `Repositories.AddRepository` to register a repo against the project.
   - `Workspaces.CreateWorkspace` with that repo; verify response.
   - `Workspaces.GetWorkspace` returns the same workspace.
   - `Workspaces.ListWorkspaces` for the project includes it.
   - `Workspaces.ArchiveWorkspace`; subsequent `Get` shows `archived_at` populated.
5. Slug-collision case: create two workspaces with the same name; verify second slug is `-2`.
6. `./scripts/regen-interfaces.sh && git diff` → committed.
7. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] V0.1 single-repo restriction returns the documented error code for multi-repo requests.
- [ ] Slug collision auto-suffix verified.
- [ ] No `TODO` / `FIXME` / `todo!()` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/proto/proto/concerto/v1/workspaces.proto` (modified — adds service)
- `crates/persist/src/projects.rs` (new)
- `crates/persist/src/workspaces.rs` (new)
- `crates/persist/src/lib.rs` (modified)
- `crates/core/src/workspace_manager/mod.rs` (new)
- `crates/core/src/workspace_manager/actor.rs` (new)
- `crates/core/src/handlers/workspaces.rs` (new)
- `crates/core/src/main.rs` (modified)
- `crates/core/tests/workspace_lifecycle.rs` (new)
- `docs/interfaces/proto.md`, `rust-api.md`, `schema.md` (regenerated)

## Commit message
```
phase-2: workspace creation (single-repo, V0.1)

Adds Workspaces gRPC service (Create/Get/List/Archive). V0.1
restricts to single-repo workspaces with a typed error code.
WorkspaceManagerActor holds the broadcast channel that the Streams
service will consume.

Refs: tasks/19-workspace-creation.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** no Projects gRPC service in V0.1 — tests insert directly via persistence. Project management UI comes later. Events go to an in-process broadcast channel until the Streams service exists.
- **Smoke-gate state:** unchanged.
