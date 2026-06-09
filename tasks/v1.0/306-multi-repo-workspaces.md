# Task 306 — Multi-Repo Workspaces (1..N repos): create/manage over `workspace_repos` with deterministic `position`

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | — |
| Touches subsystem(s) | 03 (Workspace/Session Manager), 09 (Persistence) |
| Smoke gate | unchanged |

## Goal
Make a workspace able to declare **1..N repositories** and make every workarea on it materialize one worktree **per repo**, lifting the V0.1 single-repo guards. Today the Core hard-rejects multi-repo workspaces in two places — `WorkspaceManager::create_workspace` returns `SINGLE_REPO_WIRE_CODE` when `repository_ids.len() != 1` (`crates/core/src/workspace_manager/actor.rs:166`), and `WorkareaManager::create_workarea` returns `workarea.v0_single_repo_only` when the workspace has ≠1 repos (`crates/core/src/workspace_manager/workarea.rs:200`) — even though the `workspace_repos` / `workarea_repos` junction tables have been N-capable since migration 0001. This task removes both guards, validates all `repository_ids` exist in the global registry (rejecting 0-repo workspaces at workarea-create per `design/03 §8`), generalizes the workarea-create worktree loop to iterate every repo in one transaction, and adds migration **0009** (`workspace_repos.position INTEGER`) so the repo order is deterministic and stable — the ordering that drives the "first-listed reference repo" for Task 309's files-to-copy and a stable multi-repo UI. After this task a workspace can be created with `[api, android, ios]` and its first workarea writes three worktrees under `<worktree_root>/`.

## Inputs to read before starting
- `design/03_Workspace_Session_Manager.md` §3.2 (workspace creation **declares** the repos, materializes nothing; "single-repo workspaces are workspaces with `len(workspace_repos) == 1`"), §3.3 + §6.2 (the heavy workarea-create step: per-repo ensure-cloned → `git worktree add` → sparse cones → files-to-copy → persist `workarea_repos` row, all in one tx), §6.1 (workspace create: validate repos exist in the global registry, persist `workspace_repos`), §4.1 (`WorkareaRepoContext` per-repo shape), §8 (the `git worktree add` fails for one of N repos → mark workarea `partial`; **0-repo workspace → reject at create** — `partial` is owned by Task 307, NOT this task; for now a single-repo failure aborts the whole create as today).
- `design/09_Persistence.md` §4.1 — the `workspaces` / `workspace_repos` / `workareas` / `workarea_repos` table shapes; `workspace_repos` is the join you extend with `position`.
- `crates/core/src/workspace_manager/actor.rs` — `create_workspace` (lines ~151–290): the `repository_ids.len() != 1` guard at line 166 + `SINGLE_REPO_WIRE_CODE` const (line 42) + the per-repo existence/ownership validation loop (~197) + the `update_repos` call (line 239). Remove the guard; keep the validation; the const stays defined but unused-by-create (mark it `#[deprecated]` or keep for back-compat — see notes).
- `crates/core/src/workspace_manager/workarea.rs` — `create_workarea` (lines 174–383): the `repo_ids.len() != 1` guard at line 200, the single `repo_id = &repo_ids[0]` path, the worktree-setup block (steps 1–6, lines ~262–356) that must become a **loop over all repos** inside the same composer-allocation retry + the same DB transaction. Note the existing `insert_workarea_repo` call (line 321) is already per-repo — call it once per repo. The `settings_json` stamp + `update_status("active")` happen once per workarea, after the loop.
- `crates/persist/src/workspaces.rs` — `update_repos` (line 125; clears + re-inserts, currently `INSERT … (workspace_id, repository_id)` with no position) and `list_repos` (line 148; `ORDER BY repository_id`). **Both must become position-aware**: `update_repos` writes `position = array index`; `list_repos` orders by `position`. `is_unique_violation` (line 168) is the slug-retry helper.
- `crates/persist/migrations/0001_initial_schema.sql` lines 91–98 — the `workspace_repos` table (`PRIMARY KEY (workspace_id, repository_id)`, no `position`). Migration 0009 adds the column; this PK stays.
- `crates/persist/migrations/0002_workareas_settings_json.sql` — the `ALTER TABLE … ADD COLUMN` precedent (a plain additive migration; no recreate needed here because `position` is a nullable/defaulted ADD COLUMN, unlike Task 307's CHECK-widen).
- `tasks/v1.0/PHASE3_PLANNING.md` §3 (migration reservation: **0009 = this task**, `workspace_repos.position INTEGER`; confirm 0008 is still the highest on `main` before writing — if a Phase-2 migration landed above 0008, shift the whole §3 block up and note it in Handoff) + §2 row 309 ("reference worktree = first repo by `workspace_repos.position`") + §1 D9(a) (the reservation table is authoritative).

## Scope — in
- **Migration 0009** (`crates/persist/migrations/0009_workspace_repos_position.sql`): `ALTER TABLE workspace_repos ADD COLUMN position INTEGER NOT NULL DEFAULT 0;`. Backfills existing single-repo rows to position 0 (correct — they are the only/first repo). Add an index `CREATE INDEX idx_workspace_repos_position ON workspace_repos(workspace_id, position);` for the ordered read.
- **Persist** (`crates/persist/src/workspaces.rs`): `update_repos` writes `position` = the 0-based index of each `RepositoryId` in the passed slice (insertion order = declaration order). `list_repos` changes `ORDER BY repository_id` → `ORDER BY position, repository_id` (the `repository_id` tiebreak keeps it deterministic if two rows ever share a position). Document the ordering contract in the fn doc-comment + the module header.
- **`create_workspace`** (`actor.rs`): delete the `repository_ids.len() != 1` guard (lines 166–171). Keep + strengthen the validation loop: every `repository_id` must exist in the global registry; the set must be **non-empty** (reject empty with a typed `workspace.no_repos` validation error → `INVALID_ARGUMENT`); de-dup is rejected (a repo listed twice → `workspace.duplicate_repo`). `update_repos` persists them in the caller's order.
- **`update_workspace_repos`** (new handle method on `WorkspaceManager`, per `design/03 §5.1`): re-validate (exist in the global registry, non-empty, no dups) then `update_repos`. Emit `workspace.events: repos updated`. This is the "edit the repo list" path §6.1 describes.
- **`create_workarea`** (`workarea.rs`): delete the `repo_ids.len() != 1` guard (lines 200–205). Read the repos via the new position-ordered `list_repos`. Inside the composer-retry loop, for **each** repo (in position order): ensure cloned (existing `clone_repo` path), `git worktree add <worktree_root>/<repo.name> -b <branch>`, append `.context/` to that worktree's `.git/info/exclude`, and `insert_workarea_repo`. The `.context/` skeleton is created once at `worktree_root`. Sparse-cone application + files-to-copy are **called per repo but their multi-repo bodies are owned by 302 / 309** — this task wires the loop and passes each repo's worktree path; it keeps the existing single-repo files-to-copy call working per repo (reference-repo selection for files-to-copy is 309's job — for now call `files_to_copy::apply(repo_local, repo_worktree)` per repo as today). All DB writes for one workarea stay in **one transaction**; the whole create rolls back on any per-repo failure (the `partial` soft-failure path is Task 307).
- Tests (Tier 1, co-located): create a 3-repo workspace (validate the 3 `workspace_repos` rows + their positions 0/1/2); `list_repos` returns them in declaration order; reject empty-repo + duplicate-repo + foreign-repo create; `update_workspace_repos` re-orders; create a workarea on a 2-repo workspace and assert two `workarea_repos` rows + two worktrees on disk (with a stubbed/echo agent — no real clone needed, use `file://` fixture repos as the existing workarea tests do).

## Scope — out
- **Widening `workareas.status` with `finished` / `partial` + the FSM wiring + the per-repo `git worktree add` soft-failure → `partial`** — Task 307 (migration 0010 + FSM). This task aborts the whole create on a per-repo failure (no `partial` yet); 307 adds the soft path.
- **Multi-repo sparse-cone inheritance resolver + `EstimateConeSize`** — Task 302 / 305. This task calls the cone-apply seam per repo but does not own the three-layer inheritance.
- **Reference-repo selection + the multi-repo `.worktreeinclude` parser** (copy/symlink/exclude across repos, Windows fallback) — Task 309 (which reads `workspace_repos.position` to pick the first-listed reference repo). This task lands `position`; 309 consumes it.
- **Desktop multi-repo UI** (per-repo selector, multi-select workspace create) — Task 322.
- A `repository_ids` field on the `Workarea` proto message — not needed here (clients read repos via `ListWorkareaRepos` / the existing per-repo diff scope); if 322 needs an explicit list surface, that is 322's to add. Do **not** churn the frozen `Workarea` message.

## Public interface this task locks
- **Migration 0009 — `workspace_repos.position` (FROZEN).** `position INTEGER NOT NULL DEFAULT 0`; the per-`(workspace_id)` ordinal (0-based) that is the canonical repo order. Index `idx_workspace_repos_position (workspace_id, position)`. The `(workspace_id, repository_id)` PK is unchanged. **This is the ordering Task 309's reference-repo (`first by position`) and any stable multi-repo UI key off — do not re-derive repo order from `repository_id` anywhere after this task.**
- **Persist ordering contract (FROZEN):** `workspaces::list_repos` returns `RepositoryId`s ordered by `(position, repository_id)`; `workspaces::update_repos(conn, ws, &[RepositoryId])` assigns `position = slice index`. Insertion order = declaration order = merge/UI order.
- **`WorkspaceManager::update_workspace_repos(WorkspaceId, Vec<RepositoryId>) -> Result<()>` (FROZEN signature, per `design/03 §5.1`):** re-validates + re-positions the set; emits `workspace.events`.
- **Validation wire-codes (FROZEN):** `workspace.no_repos` (empty set), `workspace.duplicate_repo` (repeated id) — both surfaced as `Error::Validation` → `INVALID_ARGUMENT`. The V0.1 `SINGLE_REPO_WIRE_CODE` (`workspace.v0_single_repo_only`) and `workarea.v0_single_repo_only` are **retired** as active rejections (the const may remain defined for one release for client back-compat; note in Handoff).

## Implementation notes
- **`position` not a new table.** `workspace_repos` is already N-capable; the only gap is deterministic order. A plain `ADD COLUMN … DEFAULT 0` (like migration 0002) backfills correctly — no recreate-table, no CHECK to widen (contrast Task 307). Keep the migration trivial.
- **One transaction per workarea.** The existing `create_workarea` opens the tx after building the FS artifacts for the single repo. For N repos: build all N worktrees on disk first (outside the tx), then open one tx and insert the `workareas` row + N `workarea_repos` rows + the `active` status + the `settings_json` stamp, commit. On a UNIQUE composer collision mid-tx, roll back the DB **and** clean up all N worktree dirs (extend the existing `remove_worktree_best_effort` cleanup to loop). On a `git worktree add` failure for any repo, abort the whole create (clean up the worktrees built so far) — the soft `partial` path is 307.
- **Don't break the single-repo happy path.** A 1-repo workspace must behave byte-identically to today (position 0, one worktree, the smoke gate's `workspace-workarea` check stays green). Test both the 1-repo and N-repo shapes.
- **Validation order matters for good errors.** Check non-empty → no-dups → each exists in the global registry, returning the first failure. Reuse the existing per-repo existence loop in `create_workspace`; factor it into a shared helper both `create_workspace` and `update_workspace_repos` call.
- **Cross-platform.** Worktree paths use `<worktree_root>/<repo.name>/`; `repo.name` is already filesystem-safe (the `repositories` UNIQUE(name) + folder-path use). No `std::os::unix` — the loop is pure `tokio::fs` + the existing `concerto_gix_wrap::worktree_add`. Builds on the Windows/Linux CI lanes (Task 113).
- **Regen.** Migration 0009 ⇒ `./scripts/regen-interfaces.sh` updates `docs/interfaces/schema.md`; commit it. No proto change ⇒ `proto.md` unchanged.

## Verification
Tier 1.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core workspace` and `cargo test -p concerto-persist workspaces` → multi-repo create (positions 0/1/2), `list_repos` ordering, empty/dup/foreign rejects, `update_workspace_repos` reorder, 2-repo workarea create (two `workarea_repos` rows + two worktrees) all pass; the existing single-repo tests stay green.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (no new deps).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`schema.md` gains `workspace_repos.position`).
7. `scripts/smoke.sh` → **unchanged gate** (the V0.1 `workspace-workarea` single-repo check must still pass through the relaxed path). A `multi-repo` smoke capability is deliberately deferred (the Phase-3 Tier-3 checklist line "create a multi-repo workspace" + Task 322's UI cover the end-to-end); note this in Handoff.

**Tier-1 scope.** This is pure Core + persistence logic, fully CI-provable against `file://` fixture repos with a stubbed/echo agent. The Tier-3 line it gestures at — sparse+blobless clone of a real >10 GB monorepo with <30 s p50 workspace creation — is the **Phase-3 manual checklist**'s job (and Task 301/302/303's perf gates), not this task.

## Definition of Done
- [x] Migration 0009 adds `workspace_repos.position` (+ index); `cargo test -p concerto-persist` confirms backfill of existing rows to 0
- [x] `update_repos` writes position = slice index; `list_repos` orders by `(position, repository_id)`; documented as FROZEN
- [x] `create_workspace` accepts 1..N repos; rejects empty (`workspace.no_repos`), dups (`workspace.duplicate_repo`), non-existent repos; the single-repo guard is gone
- [x] `update_workspace_repos` handle method validates + re-positions + emits `workspace.events`
- [x] `create_workarea` loops all repos in one transaction (worktree per repo, `workarea_repos` row per repo, `.context/` once); single-repo path unchanged; whole-create rollback on any per-repo failure
- [x] No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code (deliberate seams in Handoff)
- [x] No files outside Outputs modified (two mechanical forced call-site updates — `handlers/streams.rs`, `workspace_manager/mod.rs` — documented in Handoff Drift)
- [x] Interfaces regenerated + committed (`schema.md`)
- [x] Smoke gate green (unchanged)
- [x] Single commit with the message below

## Outputs
- `crates/persist/migrations/0009_workspace_repos_position.sql` (new)
- `crates/persist/src/workspaces.rs` (modified — position-aware `update_repos` + `list_repos`)
- `crates/core/src/workspace_manager/actor.rs` (modified — drop single-repo guard; non-empty/dup/foreign validation; `update_workspace_repos`)
- `crates/core/src/workspace_manager/workarea.rs` (modified — per-repo worktree loop in one tx; drop single-repo guard)
- `crates/core/src/handlers/workspaces.rs` (modified — wire `update_workspace_repos` / relaxed `create_workspace` if a handler arg shape needs it)
- `crates/core/tests/*` (new/modified — multi-repo workspace + workarea integration tests)
- `docs/interfaces/schema.md` (regenerated)

## Commit message
```
phase-3: multi-repo workspaces (1..N repos) + workspace_repos.position

Drops the V0.1 single-repo guards in create_workspace + create_workarea,
validates repos (non-empty/no-dup/exists-in-registry), and loops worktree
setup over every repo in one transaction. Adds migration 0009
(workspace_repos.position) for deterministic repo order — the ordering
Task 309's reference repo and the multi-repo UI key off.

Refs: tasks/v1.0/306-multi-repo-workspaces.md
```

## Handoff Notes (filled in when finishing)
- Drift from plan: **(1)** `update_workspace_repos` is a `WorkspaceManager` **Rust handle method only** (per `design/03 §5.1`), not a new gRPC RPC — adding a `Workspaces.UpdateWorkspaceRepos` proto RPC was avoided to keep `proto.md` unchanged as the task required; clients still edit repos via the handle (08/10 consume it; a wire RPC, if 322 needs one, is 322's to add). Tested via a direct-handle unit test. **(2)** Two files outside the listed Outputs were touched as mechanical, forced call-site updates: `crates/core/src/handlers/streams.rs` (added the exhaustive-match arm for the new `WorkspaceEvent::ReposUpdated` → `kind = "repos_updated"`, the §5.3 "repos updated" surface) and `crates/core/src/workspace_manager/mod.rs` (re-export the new `NO_REPOS_WIRE_CODE`/`DUPLICATE_REPO_WIRE_CODE` consts + `#[allow(deprecated)]` on the retained `SINGLE_REPO_WIRE_CODE` re-export). **(3)** `SINGLE_REPO_WIRE_CODE` is now `#[deprecated]` (kept defined for one release of client back-compat per the locked surface); the V0.1 `multi_repo_rejected_with_typed_wire_code` integration test was replaced by `multi_repo_create_persists_positions` + empty/dup/foreign reject tests.
- Open questions for next task: **307** owns migration `0010` (`workareas.status` CHECK-widen with `finished`/`partial`) + the soft per-repo `git worktree add` failure → `partial` path; this task aborts the whole create on any per-repo failure and cleans up all worktrees built so far (`cleanup_worktrees`). **309** reads `workspace_repos.position` — the FROZEN reference repo is `workspaces::list_repos(...)[0]` (position 0); this task seeds the default-empty cone (`"[]"`) per repo and calls `files_to_copy::apply(repo_local, repo_worktree)` per repo (each repo's own `local_path` is its reference for now — cross-repo `.worktreeinclude` reference-repo selection is 309's). **302/305** own the three-layer cone resolution that replaces the per-repo `empty_cones()` seed.
- Deliberate debt: — (no `TODO`/`FIXME`/`unimplemented!()` added; the per-repo cone-seed-as-empty and per-repo files-to-copy reference are documented seams owned by 302/305/309, not debt).
- Smoke-gate state: **unchanged / green.** `scripts/smoke.sh --only workspace-workarea` passes (31 s, all checks incl. `sparse-cone-clone`) — the V0.1 single-repo `workspace-workarea` check stays green through the relaxed 1..N path (1 repo → position 0 → one worktree, byte-identical). No `multi-repo` smoke capability added (deferred to the Phase-3 Tier-3 checklist "create a multi-repo workspace" + Task 322's UI, per the task's Verification §7).
