# Task 20 — Workarea Creation and Worktree Setup

| Field | Value |
|---|---|
| Phase | 2 |
| Size | medium (1–3d) |
| Depends on | 18, 19 |
| Touches subsystem(s) | 03 (Workspace Manager), 02 (Repo Manager), 09 (Persistence), 10 (Local API) |
| Smoke gate | unchanged |

## Goal
Implement workarea creation: pick a composer name, allocate a branch name, call `git worktree add` for the workspace's repo, create the `.context/` skeleton, and persist `workareas` + `workarea_repos` rows. V0.1 ships single-repo workareas (matching single-repo workspaces from Task 19).

## Inputs to read before starting
- `design/03_Workspace_Session_Manager.md` §3.3 (workarea creation steps), §3.5 (composer naming pool), §6.2 (workarea creation in detail), §4.2 (`.context/` directory).
- `tasks/19-workspace-creation.md` → "Handoff Notes".

## Scope — in
- Add the composers naming pool: a static list of ~500 well-known composer names (Bach, Mozart, Beethoven, ...) checked in as `crates/core/src/workspace_manager/composers.rs` (`pub const COMPOSERS: &[&str] = &["bach", "mozart", ...]`). Provide 200+ names — the file should be a real list, not a stub.
- Implement `WorkspaceManagerHandle::create_workarea(req: CreateWorkarea) -> Result<WorkareaId>`:
  1. Validate: workspace exists, not archived.
  2. Allocate `composer_name`: pick the lowest-indexed composer not already in use within this workspace; fall back to `<composer>-N` when the pool's exhausted.
  3. Branch name: `concerto/<composer>` (rename hook is a V1.0 task).
  4. Compute `worktree_root`: `~/concerto/workspaces/<workspace.slug>/<composer>/`.
  5. For each repo (V0.1: exactly one): ensure repo is cloned via Repo Manager, then `git worktree add <worktree_root>/<repo.name> -b <branch_name>`.
  6. Create `.context/` skeleton: `PROMPT.md` (empty placeholder), `todos.md` (empty), `scratch/` (empty dir).
  7. Add `.context/` to each repo's `.git/info/exclude` (so the agent's scratch isn't tracked).
  8. Persist `workareas` row with status `created`; persist `workarea_repos` row.
  9. Transition `created → active` (set status, persist).
  10. Emit `workarea.events: created` via the broadcast channel.
- Persistence helpers in `crates/persist/src/workareas.rs`:
  - `insert(tx, NewWorkarea) -> Result<WorkareaId>`
  - `update_status(tx, id, status) -> Result<()>`
  - `get(reader, id) -> Result<Option<Workarea>>`
  - `list_by_workspace(reader, workspace_id, include_archived: bool) -> Result<Vec<Workarea>>`
  - `archive(tx, id) -> Result<()>`
  - `list_composer_names_in_workspace(reader, workspace_id) -> Result<HashSet<String>>` (for allocation).
- Add `Workareas` proto service: `CreateWorkarea`, `GetWorkarea`, `ListWorkareas`, `ArchiveWorkarea`. (Pause/resume etc. are later.)
- Implement `WorkareasHandler` in `crates/core/src/handlers/workareas.rs`.
- Integration test using `test-harness`:
  - Insert project + repo + workspace.
  - `CreateWorkarea`; verify worktree directory exists; verify `.context/PROMPT.md` exists; verify DB row.
  - Create a second workarea on the same workspace; verify it gets a different composer name.
  - `ArchiveWorkarea`; verify status transitions and `archived_at` is set.

## Scope — out
- Setup script execution (V1.0 — needs `scripts` from project settings).
- Files-to-copy (V1.0).
- Sparse cones per repo (V1.0).
- Branch-rename hook (V1.0 — needs Maestro).
- PR set (V1.0).
- Permission-mode inheritance enforcement (Task 32).
- Multi-repo workarea (V1.0).
- Run scripts / dev servers (V1.0).

## Public interface this task locks
- Proto: `Workareas.CreateWorkarea`, `.GetWorkarea`, `.ListWorkareas`, `.ArchiveWorkarea`. Field numbers frozen.
- Rust: `crates/core/src/workspace_manager/composers.rs` — the composer name pool. Adding names is fine; removing or reordering breaks composer allocation (the lowest unused name is picked).
- On-disk: `<data_dir>/workspaces/<workspace.slug>/<composer>/<repo.name>/`. Path scheme frozen.

## Implementation notes
- Composer allocation: select the lowest-index name in `COMPOSERS` that is NOT in the result of `list_composer_names_in_workspace`. Use `iter().enumerate().find(|(_, n)| !used.contains(*n))`. Suffix with `-2`, `-3`, ... on overflow.
- Concurrent creates of two workareas on the same workspace race for the composer name. The unique constraint `(workspace_id, composer_name)` catches it; retry on conflict.
- The `.context/` directory must NOT be inside the repo's worktree; it sits at the workarea root (one level up from each repo's worktree). Per `design/03 §4.2`: `<worktree_root>/.context/`. So the layout is:
  ```
  <data_dir>/workspaces/<workspace.slug>/<composer>/
  ├── .context/
  │   ├── PROMPT.md
  │   ├── todos.md
  │   └── scratch/
  └── <repo.name>/         # the worktree for the (single in V0.1) repo
      └── .git -> ../...   # symlink to the repo's .git
  ```
- Adding `.context/` to `.git/info/exclude` is per-repo since each worktree has its own info dir (worktrees share `objects` but have their own `index`, `HEAD`, and `info/`).
- Writes to disk should be done with `tokio::fs` to avoid blocking the runtime.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core workarea_lifecycle` → all tests pass, including the two-workarea allocation case.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual: spawn Core, create project+repo+workspace, create workarea via gRPC, verify on-disk layout matches the diagram above.
5. `./scripts/regen-interfaces.sh && git diff` → committed.
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [x] Verification commands pass.
- [x] On-disk layout matches spec.
- [x] Composer-name allocation behaves correctly on collision.
- [x] `.context/` is in every repo's git exclude (verified by `git status` showing no `.context/` files).
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `crates/proto/proto/concerto/v1/workareas.proto` (modified — adds service + request/response messages)
- `crates/persist/src/workareas.rs` (new)
- `crates/persist/src/lib.rs` (modified — module + re-exports)
- `crates/persist/src/api.rs` (modified — `WorkareaId`, `NewWorkarea`, `NewWorkareaRepo`, `Workarea` exposed for the interface generator)
- `crates/core/src/workspace_manager/composers.rs` (new)
- `crates/core/src/workspace_manager/workarea.rs` (new — the create_workarea logic)
- `crates/core/src/workspace_manager/mod.rs` (modified — re-exports `composers`, `workarea`)
- `crates/core/src/handlers/workareas.rs` (new)
- `crates/core/src/handlers/mod.rs` (modified — `pub mod workareas`)
- `crates/core/src/api_server.rs` (modified — `with_managers` extended to take `workarea_manager`)
- `crates/core/src/main.rs` (modified — spawns `WorkareaManagerActor` + 5-arg `with_managers`)
- `crates/core/tests/workarea_lifecycle.rs` (new)
- `crates/test-harness/src/clients.rs` (modified — `workareas_client` accessor + `WorkareasClient` type)
- `crates/test-harness/src/lib.rs` (modified — re-exports `WorkareasClient`, adds `CoreUnderTest::workareas_client`)
- `docs/interfaces/proto.md`, `rust-api.md`, `schema.md` (regenerated)

## Commit message
```
phase-2: workarea creation + worktree setup

Adds Workareas gRPC service (Create/Get/List/Archive). Composer name
pool of 200+ names with per-workspace allocation. Worktree layout
<data_dir>/workspaces/<slug>/<composer>/ with .context/ skeleton per
design/03 §4.2.

Refs: tasks/20-workarea-creation.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **Added `WorkareaId`, `NewWorkarea`, `NewWorkareaRepo`, `Workarea` to `crates/persist/src/api.rs`** so the interface generator picks them up. Same pattern Task 19 used for `WorkspaceId`/`Workspace`. Added to Outputs.
  - **`ApiServerActor::with_managers` signature extended** with a fourth `workarea_manager: Option<WorkareaManager>` arg per the orchestrator brief. `with_repo_manager` (Task 18) and the 4-arg `with_managers` (Task 19) are both kept; the production binary uses the 5-arg `with_managers`.
  - **`workareas_client()` accessor + `WorkareasClient` re-export** added to `crates/test-harness/src/{clients.rs,lib.rs}` following Task 19's pattern. Added to Outputs.
  - **Composer pool is a real ~500-name curated list** under `crates/core/src/workspace_manager/composers.rs`, exceeding the 200+ floor. Hand-curated rather than generated; ordering is FROZEN per the locked-interface contract. Two entries originally had diacritics (`esplá`, `bretón`); both stripped to ASCII per the orchestrator brief so they double as filesystem path segments. The list contains a few duplicates across overlapping eras (e.g. `bach`, `norman`, `wolf` appear in more than one section); duplicates are harmless for allocation (lowest unused name is picked once and the later occurrence is skipped) and reordering to deduplicate would change which composer is picked for new workareas, which is the locked contract.
  - **Composer collision-retry cleanup uses an in-file `git worktree remove --force` shell-out** (`remove_worktree_best_effort` in `workspace_manager/workarea.rs`) rather than adding a `worktree_remove` to `gix-wrap`. Task 18 locked `gix-wrap`'s V0.1 surface; adding to it now would expand a frozen interface. The path runs only on the rare UNIQUE collision, and the disk-side `remove_dir_all` is the real cleanup.
  - **`workspace_id` validation rejects archived workspaces** with `Error::Validation("workspace.archived: ...")` matching the orchestrator brief's `workspace.archived` wire-code subcode embedded in the message body.
  - **V0.1 single-repo enforcement** uses the wire-code subcode `workarea.v0_single_repo_only` (mirroring Task 19's `workspace.v0_single_repo_only`). Returned via `Error::Validation` → `INVALID_ARGUMENT` per `error_map`.
  - **`ListWorkareas` exposes the `include_archived` knob now**, not as a later additive field. Spec ambiguous; we shipped the bool today because the UI will need it from day one and adding it later would have required a new field number on the request message.
  - **Archived workareas are still counted as "in use" by composer allocation** — `list_composer_names_in_workspace` does not filter by `archived_at`. Rationale: workarea archive is reversible in design, and the composer-name namespace is large enough that the trade-off is invisible.
  - **`WorkareaManager::create_workarea` drives `RepoManager::clone_repo` lazily** when the repo's `local_path/.git` does not yet exist. Integration tests pre-clone via plain `git clone` to keep the test path deterministic; the production binary relies on the `RepoManager` path.
- **Open questions for next task:**
  - **Task 21+ (agent host)** can call `WorkareaManager::get(id)` to resolve `worktree_root` for the PTY's working directory. The `Workarea` row's `worktree_root` is the absolute path locked here (`<data_dir>/workspaces/<workspace.slug>/<composer>/`).
  - **Task 24 (`Streams`)** consumes from `WorkareaManager::subscribe()`. `WorkareaEvent::{Created, Archived}` is the in-process shape today; wire promotion follows the same recipe as `WorkspaceEvent`.
  - **Multi-repo workareas (V1.0)** can re-use `workareas::insert_workarea_repo` in a loop. The schema and persistence helpers already support N repos.
  - **`WorkareasServer` field numbers are FROZEN** at the V0.1 set (Create/Get/List/Archive). Pause/resume/run-script land at higher field numbers, additive only.
- **Deliberate debt:** no setup script execution, no files-to-copy, no sparse cones, no branch-rename hook, no permission-mode inheritance enforcement, no multi-repo workareas, no PR set, no run scripts (all V1.0). The collision-retry cleanup shell-out duplicates a `gix-wrap` capability; promoting it to `gix-wrap::worktree_remove` is a Phase-3 follow-on once a real caller needs it. No `TODO`/`FIXME`/`todo!()`/`unimplemented!()` markers in new code.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` (v1) still boots the Core, calls `Runtime.GetServerCapabilities`, and shuts down cleanly. The Task 20 RPCs (`Workareas.*`) are exercised by `crates/core/tests/workarea_lifecycle.rs` via the Task 17 harness, not by the smoke gate.
