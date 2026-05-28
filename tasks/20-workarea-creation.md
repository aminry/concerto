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
- [ ] Verification commands pass.
- [ ] On-disk layout matches spec.
- [ ] Composer-name allocation behaves correctly on collision.
- [ ] `.context/` is in every repo's git exclude (verified by `git status` showing no `.context/` files).
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/proto/proto/concerto/v1/workareas.proto` (modified)
- `crates/persist/src/workareas.rs` (new)
- `crates/persist/src/lib.rs` (modified)
- `crates/core/src/workspace_manager/composers.rs` (new)
- `crates/core/src/workspace_manager/workarea.rs` (new — the create_workarea logic)
- `crates/core/src/handlers/workareas.rs` (new)
- `crates/core/src/main.rs` (modified)
- `crates/core/tests/workarea_lifecycle.rs` (new)
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
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** no setup script execution, no files-to-copy, no sparse cones (V1.0).
- **Smoke-gate state:** unchanged.
