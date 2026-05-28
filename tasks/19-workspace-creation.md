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
- [x] Verification commands pass.
- [x] V0.1 single-repo restriction returns the documented error code for multi-repo requests.
- [x] Slug collision auto-suffix verified.
- [x] No `TODO` / `FIXME` / `todo!()` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `crates/proto/proto/concerto/v1/workspaces.proto` (modified — adds service + request/response messages)
- `crates/persist/src/projects.rs` (new)
- `crates/persist/src/workspaces.rs` (new)
- `crates/persist/src/lib.rs` (modified — module + re-exports)
- `crates/persist/src/api.rs` (modified — `ProjectId`, `NewProject`, `Project`, `WorkspaceId`, `NewWorkspace`, `Workspace` exposed for the interface generator)
- `crates/core/Cargo.toml` (modified — `sqlx` promoted from dev-dep to dep for `Connection::begin()`)
- `crates/core/src/lib.rs` (modified — `pub mod workspace_manager`)
- `crates/core/src/workspace_manager/mod.rs` (new)
- `crates/core/src/workspace_manager/actor.rs` (new)
- `crates/core/src/handlers/mod.rs` (modified — `pub mod workspaces`)
- `crates/core/src/handlers/workspaces.rs` (new)
- `crates/core/src/api_server.rs` (modified — `with_managers` constructor + optional `WorkspacesServer` registration)
- `crates/core/src/error_map.rs` (modified — `validation` → `InvalidArgument`, `not_found` → `NotFound`)
- `crates/core/src/main.rs` (modified — spawns `WorkspaceManagerActor` + uses `ApiServerActor::with_managers`)
- `crates/core/tests/workspace_lifecycle.rs` (new — integration tests for the V0.1 Workspaces surface)
- `crates/error/src/api.rs` (modified — adds `Error::Validation(String)` and `Error::NotFound(String)`)
- `crates/error/src/error.rs` (modified — `wire_code` returns `"validation"` / `"not_found"`)
- `crates/error/tests/wire_codes.rs` (modified — adds `validation_wire_code_and_display`, `not_found_wire_code_and_display`)
- `crates/test-harness/src/clients.rs` (modified — `workspaces_client` accessor + `WorkspacesClient` type)
- `crates/test-harness/src/lib.rs` (modified — re-exports `WorkspacesClient`, adds `CoreUnderTest::workspaces_client`)
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
- **Drift from plan:**
  - **`Error::Validation(String)` and `Error::NotFound(String)` variants added to `crates/error/src/api.rs`.** Pre-authorised in the orchestrator drift block. `wire_code` returns `"validation"` / `"not_found"`; `error_to_status` maps them to `Code::InvalidArgument` / `Code::NotFound`. Wire-code contract tests added at `crates/error/tests/wire_codes.rs::{validation_wire_code_and_display, not_found_wire_code_and_display}`. The V0.1 multi-repo subcode `workspace.v0_single_repo_only` is embedded in the `Error::Validation` message body (and therefore the `ConcertoError.message` over the wire); `ConcertoError.code` carries the generic `"validation"` per the existing wire-code contract, matching the "simpler" approach the orchestrator's drift block sketched.
  - **`ApiServerActor::with_managers` constructor added** alongside the existing `new` and `with_repo_manager`. `with_managers(started_at, view, repo_manager: Option<RepoManager>, workspace_manager: Option<WorkspaceManager>)` is the single growing-surface constructor; `with_repo_manager` is kept for back-compat with any in-flight call sites but `main.rs` now uses `with_managers`.
  - **`workspaces_client()` accessor + `WorkspacesClient` re-export added** to `crates/test-harness/src/{clients.rs,lib.rs}` per the Task 18 handoff brief. Pattern matches `runtime_client()` / `repositories_client()`.
  - **`crates/core/Cargo.toml` adds `sqlx` as a regular dependency** (already a dev-dep). The workspace manager opens a `sqlx::Connection::begin()` transaction on top of the persistence layer's writer guard so the `workspaces` + `workspace_repos` inserts commit atomically; that pulls in the `Connection` trait + the typed-error path needed for unique-constraint detection.
  - **Single-tx multi-row write** is implemented as `persistence.writer().await` → `Connection::begin()` → insert workspace → `update_repos` → `commit`. On UNIQUE(`project_id, slug`) violation (SQLite extended code `2067`, detected via `concerto_persist::workspaces::is_unique_violation`) the tx rolls back, the suffix increments, and the loop retries up to 100 times — well clear of any realistic UI rename collision rate.
  - **`Workspace` row's `permission_mode` is the lowercase SQL form** (`"strict" | "normal" | "auto" | "yolo"`); the handler converts between the proto enum and that string at the wire boundary. `None` means "inherit from project" (`design/03 §3.2`), and the proto `PermissionMode::Unspecified` is rejected as INVALID_ARGUMENT — callers must omit the `optional` field for inheritance, not set it to `UNSPECIFIED`.
  - **`WorkspaceManagerActor`'s `run` parks on shutdown** following the Task 18 `RepoManagerActor` pattern. The meaningful surface is the `WorkspaceManager` handle (`Arc<Persistence>` + `broadcast::Sender<WorkspaceEvent>(256)`) which the gRPC `WorkspacesHandler` holds directly.
- **Open questions for next task:**
  - **Task 20 (`Workareas.CreateWorkarea`)** can reuse: the harness `workspaces_client()` accessor and pattern; the `with_managers` ApiServerActor constructor (extend with `workarea_manager: Option<...>` when it lands); the same persistence-layer `Connection::begin()` writer pattern for multi-table writes (`workareas` + `workarea_repos`); the same broadcast-channel `WorkspaceEvent` pattern can be lifted to `WorkareaEvent`.
  - **`Streams` service (Task 24)** consumes from `WorkspaceManager::subscribe()`. The `WorkspaceEvent::{Created, Archived}` shape is in-process today; promoting them to wire types adds the proto message + the subscribe-and-forward task. Current capacity 256 is sized for the Task 19 spec — Task 24 may need to revisit under load.
  - **No `Projects` gRPC service yet.** Task 19's integration tests still seed a `projects` row by going around the API (direct sqlx INSERT, same pattern as Task 18's `repository_clone.rs`). Adding `Projects.Create` is a small Phase 2 follow-on; the persistence helpers (`crates/persist/src/projects.rs::{insert, get, list_all}`) are already on the canonical surface.
  - **Slug derivation is frozen.** The algorithm in `crates/core/src/workspace_manager/actor.rs::derive_slug` is the locked V0.1 surface — any change is a revision task per `tasks/README.md §8`. Unit tests in the same file pin: basic, punctuation stripping, dash-run collapsing, 64-char truncation, empty-on-punct-only, underscore/slash mapping.
- **Deliberate debt:** no Projects gRPC service in V0.1 — integration tests insert directly via persistence helpers (`projects::insert`-equivalent direct sqlx). Project management UI comes later. `WorkspaceEvent`s emit into an in-process broadcast channel until the Streams service exists (Task 24). The `PathBuf` import in `workspace_manager/actor.rs` is unused in V0.1 and kept as a forward-looking marker (`_path_marker` helper) since the design notes hint at workarea-root path involvement at later tasks; rustfmt-clean, clippy-clean. No `TODO`/`FIXME`/`todo!()`/`unimplemented!()` markers in new code.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` (v1) still boots the Core, calls `Runtime.GetServerCapabilities`, and shuts down cleanly. The Task 19 RPCs (`Workspaces.*`) are exercised by `crates/core/tests/workspace_lifecycle.rs` via the Task 17 harness, not by the smoke gate.
