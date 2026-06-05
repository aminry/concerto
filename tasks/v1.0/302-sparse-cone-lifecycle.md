# Task 302 — Sparse-Checkout + Cone + Sparse-Index Lifecycle; Per-(Workarea, Repo) Cones

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 301 |
| Touches subsystem(s) | 02 (Repository Manager), 03 (Workspace Manager — cone storage) |
| Smoke gate | new:sparse-cone-clone |

## Goal
Implement the full sparse-checkout lifecycle `design/00 §6.3` mandates — **cone-mode always, sparse-index always-on** — so a blobless+sparse clone (Task 301) gets a real, per-(workarea, repo) cone instead of an empty `--no-checkout` worktree. Today there is no sparse code anywhere: `crates/gix-wrap` has no `sparse-checkout` helpers, `crates/persist/src/workareas.rs::insert_workarea_repo` omits the `sparse_cones_json` column entirely (it exists in migration 0001 but is never written), and there is no inheritance resolver. This task adds (a) `gix-wrap` shell-out helpers `sparse_init_cone` / `sparse_set` / `sparse_add` / `sparse_reapply_index` / `sparse_disable` (cone-mode + `--sparse-index` always), (b) `RepoManager::set_workarea_repo_cones(workarea, repo, cones)` that applies the cone to the worktree AND persists it via a new `persist::workareas::update_workarea_repo_cones` writer, (c) the **three-layer cone-defaults inheritance resolver** (`repositories.cone_defaults_json` → `workspaces.settings_json.cone_defaults[repo_id]` → `workarea_repos.sparse_cones_json`), and (d) `§8` correctness handling: a pre-existing non-cone sparse config is force-set to cone-mode + audit-logged, and a bad cone path is cleanly rejected. It also adds a `SetCones` RPC and the new `sparse-cone-clone` smoke capability (a blobless+sparse `file://` clone + cone-set + `status` assertion). After this task a repo can be sparsely materialized to exactly its cone, the `--sparse-index` reapply keeps the in-memory index proportional to the cone (the lever Task 303's `<100 ms status` leans on), and cone defaults flow through the three layers.

## Inputs to read before starting
- `design/00_Architecture_Overview.md` §6.3 (Git) — **cone mode mandatory (`core.sparseCheckoutCone=true`) + sparse index**; "the non-cone path has subtle correctness bugs; we don't expose it." This is the load-bearing invariant of the whole task.
- `design/02_Repository_Manager.md` §3.1 — `git sparse-checkout init/set/add/reapply` is **`git` shell-out** ("sparse-cone behavior is git's authoritative"). Use shell-out, never `gix`, for sparse ops.
- `design/02_Repository_Manager.md` §3.2 — cones are per-**(workarea, repo)**, stored in `workarea_repos.sparse_cones_json`; **default cones inheritance**: a new workarea inherits the workspace's per-repo cone defaults (from `workspace_repos.cone_defaults_json` *in `settings_json`* — note: NOT a dedicated column), which inherit from the repository's `cone_defaults_json`. The user can override per-(workarea, repo). The file-count/size telemetry sentence is **Task 305**'s, not this task's.
- `design/02_Repository_Manager.md` §7.1 — the clone sequence: after `git clone --filter=blob:none --sparse --no-checkout` (301), the Repo Mgr runs `git sparse-checkout init --cone; set …` then `git checkout`. 302 owns those post-clone steps.
- `design/02_Repository_Manager.md` §8 — failure modes: **"Sparse-cone path doesn't exist in repo"** → reject the path with a clear error (NOT a panic); **"Non-cone-mode sparse config (pre-existing repo)"** → force-set `core.sparseCheckoutCone=true` on add + document in the audit log.
- `design/03_Workspace_Session_Manager.md` §3.2 + §12 R-2 — the three-layer inheritance chain as the Workarea Manager sees it (workarea create reads the resolved cones).
- `crates/gix-wrap/src/cmd.rs` — `cmd::run` (the shell-out primitive every sparse helper uses) + `crates/gix-wrap/src/api.rs` (the FROZEN public surface; add the sparse helpers as new `pub async fn`s alongside `clone_with_strategy` from 301).
- `crates/persist/src/workareas.rs` — `insert_workarea_repo` (line ~83) currently inserts only `workarea_id, repository_id, worktree_path, branch_override` — it **omits `sparse_cones_json`**, which is why the column is never written today. Add the `update_workarea_repo_cones` writer + extend `insert_workarea_repo` to accept an optional initial cone set. The `workarea_repos.sparse_cones_json TEXT NOT NULL DEFAULT '[]'` column (migration 0001, schema in the file header) is FROZEN — write it, don't migrate.
- `crates/persist/src/workspaces.rs` — the `workspaces.settings_json TEXT NOT NULL DEFAULT '{}'` column (schema header). The "workspace defaults" cone layer lives *inside* this JSON under a `cone_defaults` key — there is no dedicated column. Add a small read/merge helper if one doesn't exist (read-modify-write to avoid clobbering other settings keys).
- `crates/persist/src/repositories.rs` — `repositories.cone_defaults_json TEXT NOT NULL DEFAULT '[]'` exists in the schema but is NOT in the SELECT list / `Repository` struct today (`api.rs` comment: "V0.1 omits `cone_defaults_json` — it's written by a V1.0 sparse + cones task"). Add it to the SELECT + struct so the resolver can read the repository-level layer. (Task 305 also needs this read — coordinate; whichever lands first adds the column to the projection.)
- `crates/proto/proto/concerto/v1/repositories.proto` — append `SetCones` per the header's reserved intent ("`Repositories.SetCones`"). Existing field numbers FROZEN.
- `tasks/v1.0/301-blobless-treeless-clone.md` → "Handoff Notes" — the `with_sparse` clone flag (301 leaves an empty `--no-checkout` worktree for 302 to populate) + the `CloneStrategy` enum + `concerto-state.json`.
- `tasks/v1.0/PHASE3_PLANNING.md` §2 (302 row: workspace-level cone-defaults layer lives **inside `workspaces.settings_json`** as a `{ repository_id: [cone_paths] }` map, NO new column — freeze that nested shape) + §2 (305/302 smoke: 302 adds the `sparse-cone-clone` capability) + §3 (302 = existing columns, **no migration**).

## Scope — in
- **`gix-wrap` sparse helpers** (all shell-out via `cmd::run`, all cone-mode + `--sparse-index`):
  - `sparse_init_cone(worktree) → ()` — `git sparse-checkout init --cone --sparse-index`.
  - `sparse_set(worktree, &[ConePath]) → ()` — `git sparse-checkout set --sparse-index <paths…>` (replaces the cone).
  - `sparse_add(worktree, &[ConePath]) → ()` — `git sparse-checkout add <paths…>`.
  - `sparse_reapply_index(worktree) → ()` — `git sparse-checkout reapply --sparse-index` (the lever 303 needs; reapply after every cone change).
  - `sparse_disable(worktree) → ()` — `git sparse-checkout disable` (full materialization).
  - `is_cone_mode(worktree) → bool` — reads `core.sparseCheckoutCone`; `force_cone_mode(worktree)` sets it true (the §8 non-cone-force path).
- **`RepoManager::set_workarea_repo_cones(workarea, repo, cones)`** (`design/02 §5.1`): resolve the (workarea, repo) worktree path, apply `sparse_set` + `sparse_reapply_index`, then persist via `persist::workareas::update_workarea_repo_cones`. Bad cone path (git warns / no such dir in the tree) → return a clean `Error` (so the handler maps to `INVALID_ARGUMENT`), do not partially apply.
- **The inheritance resolver** `resolve_cones(repo, workspace, workarea_repo) → Vec<ConePath>`: read all three layers (repository `cone_defaults_json`, workspace `settings_json.cone_defaults[repo_id]`, workarea `sparse_cones_json`); the most specific present layer wins (workarea > workspace-default > repo-default). Pure function over the three JSON inputs — table-testable.
- **`persist::workareas::update_workarea_repo_cones(conn, workarea, repo, &[ConePath])`** writer (UPDATE `sparse_cones_json`) + extend `insert_workarea_repo` to write an initial cone set (default `[]`).
- **`persist::repositories`**: add `cone_defaults_json` to the SELECT + the `Repository` struct (so the resolver reads the repo layer).
- **`§8` correctness:** on clone post-processing (and on `set_workarea_repo_cones`), if `is_cone_mode` is false, `force_cone_mode` + emit an audit event ("forced non-cone sparse config to cone mode").
- **proto + handler:** `SetCones(SetConesRequest) → Workarea`-or-`SetConesResponse`; handler delegates to `set_workarea_repo_cones`, maps a bad path to `INVALID_ARGUMENT`.
- **Smoke:** a new `sparse-cone-clone` capability (`scripts/smoke.d/<NN>-sparse-cone-clone.sh`) — blobless+sparse clone a small CI fixture, set a cone, assert `git status`/`sparse-checkout list` reports the cone and the worktree only materializes in-cone paths.
- Tests (Tier 1): a `file://` fixture with a known dir tree — `sparse_init_cone` + `sparse_set([a/])` materializes only `a/` and the cone is `--sparse-index` (assert `git sparse-checkout list` + that out-of-cone dirs are collapsed); the three-layer resolver table test (each layer wins in turn); a bad cone path → clean `Error`, no partial write; a pre-existing non-cone config → force-set + audit; `update_workarea_repo_cones` round-trips through `sparse_cones_json`.

## Scope — out
- **Clone strategy + the `--sparse --no-checkout` clone flags** — **Task 301** (302 consumes the empty worktree 301 leaves).
- **Cone-size / file-count telemetry (`list_paths_in_cone` / `ConeStats` / `EstimateConeSize`)** — **Task 305** (`design/02 §3.2` last paragraph, §5.1).
- **`suggest_cones` (Maestro-delegate)** — **Task 305** (seam) + **Task 411** (P4 wiring).
- **Blob prewarm after a cone is set** — **Task 304** (`prewarm_blobs`).
- **Multi-repo workarea create that loops the per-repo cone setup** — **Task 306/307**; 302 ships the per-(workarea, repo) primitive they call in the loop.
- **Desktop sparse-cone picker UI** — **Task 322**.
- The real 2M-file monorepo cone latency — **Task 303** (bench) + Tier-3 checklist.

## Public interface this task locks
- **Rust (FROZEN), `gix-wrap`:** the six sparse helpers above (`sparse_init_cone`, `sparse_set`, `sparse_add`, `sparse_reapply_index`, `sparse_disable`, `is_cone_mode`/`force_cone_mode`) — each `pub async fn (worktree: &Path, …) -> Result<()>`. A `ConePath` newtype (or `&str` cone paths — pick one and FREEZE; prefer `pub type ConePath = String` to match `design/02 §5.1`'s `Vec<ConePath>`).
- **The nested cone-defaults JSON shape (FROZEN, `workspaces.settings_json`):** `settings_json.cone_defaults` is a `{ "<repository_id>": ["<cone_path>", …] }` map. The repository-level layer is `repositories.cone_defaults_json` = a flat `["<cone_path>", …]` array. The workarea layer is `workarea_repos.sparse_cones_json` = a flat `["<cone_path>", …]` array. Inheritance precedence: workarea > workspace-default > repo-default. These three JSON shapes are the FROZEN contract 305/306/307/322 read.
- **proto (FROZEN field numbers):** `SetConesRequest { string workarea_id = 1; string repository_id = 2; repeated string cone_paths = 3; }`; `rpc SetCones(SetConesRequest) returns (SetConesResponse);` (or `returns (Workarea)` — pick a minimal response and freeze). Appended to `service Repositories` after `EstimateRepoSize` (301).
- **`persist::workareas::update_workarea_repo_cones(conn, &WorkareaId, &RepositoryId, &[String])`** signature.

## Implementation notes
- **`--sparse-index` is not optional.** Every cone-changing helper (`init`, `set`, `add`) must be followed by `reapply --sparse-index` (or carry `--sparse-index` on the command itself where git accepts it). Spike 104 (`design/spikes/gix-sparse-cone-findings.md` §4a) shows latency tracks the cone only because the sparse index collapses out-of-cone paths to directory entries; without it, 303's `<100 ms` bar fails. This is the single most load-bearing implementation detail.
- **Cone-mode-only is a correctness floor, not a preference.** Never call `git sparse-checkout set --no-cone`. If a repo arrives with `core.sparseCheckoutCone=false` (a user cloned it manually), force it true + audit (`§8`). The non-cone path is a known-buggy path we do not expose.
- **Bad cone path handling (`§8`):** `git sparse-checkout set <bad/path>` does not error — it warns to stderr and produces an empty/partial materialization. Detect a path that does not exist in the tree *before* applying (e.g. `git ls-tree HEAD <path>` probe, or parse the set's stderr for the warning) and reject with a clear `Error` so the handler returns `INVALID_ARGUMENT` and nothing is half-applied. Document the detection you chose.
- **The resolver is a pure function** over three JSON strings — keep it free of IO so it's table-testable, then call it from the workarea-create path (306/307 wire it) and from `set_workarea_repo_cones` when no explicit cones are given. `serde_json` is already a workspace pin in `concerto-persist`/`concerto-core`.
- **Cross-platform:** sparse-checkout shell-out works on the Win/Linux CI lanes (Task 113); cone paths are forward-slash git paths on every OS (git normalizes). Use `std::path`/`tokio::process` only.
- **Audit:** reuse the existing audit subscriber surface (the one Phase-1 task 112 / `design/09` established) for the "forced non-cone → cone" event; do not invent a new audit channel.
- Regen: proto changed ⇒ `./scripts/regen-interfaces.sh`; the new `gix-wrap` sparse surface updates `rust-api.md`. Commit both.

## Verification
Tier 1. The `rust` §5.3 set.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-gix-wrap sparse` + `cargo test -p concerto-core cone` + `cargo test -p concerto-persist workarea_repo` → cone-materialization test (only in-cone dirs present, `--sparse-index` active), three-layer resolver table test, bad-path-reject test, force-cone+audit test, `update_workarea_repo_cones` round-trip pass.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (no new deps; reuses `tokio`/`gix`/`serde_json`).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`proto.md` gains `SetCones`; `rust-api.md` gains the sparse helpers + `update_workarea_repo_cones`).
7. `scripts/smoke.sh` → **new `sparse-cone-clone` capability green.** Add `scripts/smoke.d/<NN>-sparse-cone-clone.sh` defining `check_sparse_cone_clone`, registered in `scripts/smoke.manifest` after `project-repo-clone` (it needs `PROJECT_ID`). It: seeds a small bare repo with a known multi-dir tree (`a/`, `b/`, `c/`), adds it with `clone_strategy=blobless with_sparse=true` (Task 301's smoke-client flag — add the `--cone` flag to the smoke-client `set-cones` subcommand here if absent), sets the cone to `a/`, and asserts `sparse-checkout list` (or a `status` probe) reports only `a/` materialized. SKIPs cleanly if a future lane lacks the fixture. Exits 0.

**Tier-1 scope + what it does NOT cover.** CI proves the sparse lifecycle + inheritance + force-cone on small `file://` fixtures with known dir trees. It does **not** cover the real 2M-file monorepo cone latency (that is Task 303's bench + the Phase-3 Tier-3 line "sparse+blobless clone a real >10 GB monorepo and confirm <30 s p50"). The `sparse-cone-clone` smoke check proves the *capability wires end-to-end over the live UDS Core*, not the perf bar.

## Definition of Done
- [x] Six `gix-wrap` sparse helpers (cone-mode + `--sparse-index` always); non-cone path never invoked
- [x] `set_workarea_repo_cones` applies + persists via `update_workarea_repo_cones`; `sparse_cones_json` is now written (was never written before)
- [x] Three-layer inheritance resolver (repo → workspace-default → workarea) as a pure, table-tested function; the nested `settings_json.cone_defaults` shape FROZEN
- [x] `§8` correctness: bad cone path cleanly rejected (no partial write); pre-existing non-cone config force-set to cone + audit-logged
- [x] `repositories.cone_defaults_json` added to the SELECT + `Repository` struct
- [x] `SetCones` RPC appended; no existing field numbers renumbered
- [x] `sparse-cone-clone` smoke capability added + green; manifest updated
- [x] All Verification commands pass on a clean checkout; interfaces regenerated
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (deliberate seams in Handoff)
- [x] No files outside Outputs modified
- [x] Single commit with the message below

## Outputs
- `crates/gix-wrap/src/api.rs` (modified — sparse helpers + `ConePath`) and/or `crates/gix-wrap/src/sparse.rs` (new submodule, re-exported via `lib.rs`/`api.rs`)
- `crates/core/src/repo_manager/actor.rs` (modified — `set_workarea_repo_cones`, the resolver call, force-cone+audit on clone post-process)
- `crates/core/src/repo_manager/cones.rs` (new — the pure inheritance resolver) + `crates/core/src/repo_manager/mod.rs` (modified — `pub mod cones`)
- `crates/persist/src/workareas.rs` (modified — `update_workarea_repo_cones`; `insert_workarea_repo` writes `sparse_cones_json`)
- `crates/persist/src/workspaces.rs` (modified — `settings_json.cone_defaults` read/merge helper if absent)
- `crates/persist/src/repositories.rs` + `crates/persist/src/api.rs` (modified — `cone_defaults_json` in SELECT + `Repository` struct)
- `crates/proto/proto/concerto/v1/repositories.proto` (modified — `SetCones`)
- `crates/core/src/handlers/repositories.rs` (modified — `set_cones` handler)
- `crates/gix-wrap/tests/sparse_cone.rs` + `crates/core/tests/cone_inheritance.rs` (new)
- `tools/smoke-client/src/cmd/set_cones.rs` (new — the `set-cones`/`--cone` subcommand the smoke check drives) + `tools/smoke-client/src/cmd/mod.rs` / `main.rs` / `add_repo.rs` (modified — strategy/sparse flags)
- `scripts/smoke.d/<NN>-sparse-cone-clone.sh` (new) + `scripts/smoke.manifest` (modified)
- `docs/interfaces/proto.md` + `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-3: sparse-checkout + cone + sparse-index lifecycle

Adds the cone-mode-mandatory, sparse-index-always-on lifecycle:
gix-wrap sparse helpers, set_workarea_repo_cones (now actually writing
sparse_cones_json), the three-layer cone-defaults resolver
(repo → workspace-default → workarea), and §8 correctness (force
non-cone configs to cone + audit; reject bad cone paths). New SetCones
RPC + sparse-cone-clone smoke capability.

Refs: tasks/v1.0/302-sparse-cone-lifecycle.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan:** (1) The `sparse_cones_json` round-trip test landed in a **new** `crates/persist/tests/workarea_repo_cones.rs` (not in the original Outputs, which only named the gix-wrap + core test files). Verification step 3 requires `cargo test -p concerto-persist workarea_repo` to pass a round-trip; a persist integration test was the cleanest home (the `cones.rs` resolver is pure and lives in core, the persist writer needs a real `SqlitePool`). Added to effective Outputs. (2) `crates/persist/Cargo.toml` gained `serde_json.workspace = true` — `update_workarea_repo_cones` serializes the FROZEN flat array shape; it was NOT previously a `concerto-persist` dep (it was a `concerto-core` pin). No new *workspace* pin (already in the tree), cargo-deny-clean. Added to effective Outputs. (3) `crates/core/src/audit/event.rs` (new `AuditKind::SparseConfigForcedToCone` variant + `as_str` arm), `crates/core/src/boot.rs` (rebind `repo_handle` with `.with_audit(...)`), `crates/core/src/repo_manager/mod.rs` (`pub mod cones`), and three existing call sites that construct `NewWorkareaRepo` (`crates/core/src/workspace_manager/workarea.rs`, `crates/core/tests/files_service.rs`, `crates/core/tests/vcs_gh_cli.rs`) all needed the new `sparse_cones_json` field under the workspace compile gate — mechanical, added to effective Outputs. (4) **Bug found + fixed via the smoke gate:** the §8 HEAD-tree probe (`probe_cone_paths_exist`) ran `git ls-tree -d HEAD <cone>` with the caller's trailing slash (`a/`), which `ls-tree` matches against NOTHING — so a *valid* directory cone passed as `a/` (the form the smoke-client + git's cone syntax use) was wrongly rejected as INVALID_ARGUMENT. Fixed by trimming a trailing `/` (in addition to the existing leading-`/` trim) before the probe; added a trailing-slash regression case to `sparse_cone.rs`. (5) `rust-api.md` regen does NOT list the gix-wrap sparse helpers because `regen-interfaces.sh` only scrapes `crates/*/src/api.rs`; the sparse surface lives in `sparse.rs` (re-exported via `lib.rs`). This matches the established behavior for Task 29's `status.rs`/`diff.rs` (also absent from rust-api.md) — not new drift. proto.md correctly gained `SetCones`/`SetConesRequest`/`SetConesResponse`; rust-api.md gained the persist struct field changes.
- **Open questions for next task:** (1) **303** consumes the `--sparse-index` reapply this task wires (every cone change calls `sparse_reapply_index`); its `< 100 ms status` bench leans on the collapsed-directory sparse index the smoke check asserts (`ls-files --sparse` shows `b/`/`c/` as directory entries). The real 2M-file monorepo cone latency is the Phase-3 Tier-3 line — not provable in CI here. (2) **306/307** own multi-repo workarea create: they call `RepoManager::resolve_for_workarea_repo(...)` (the three-layer resolver wired here) to seed each repo's initial cone, then `set_workarea_repo_cones`, then `insert_workarea_repo` with the resolved `sparse_cones_json` (the single-repo V0.1 create path in `workarea.rs` still seeds `empty_cones()` — left for 306/307 to swap to the resolver). (3) The `with_sparse` clone flag (301) still does NOT thread a sparse `--no-checkout` *clone*; 302 applies cone-mode per-(workarea, repo) on the **worktree** (`git worktree add` produces a checked-out worktree, then `set_workarea_repo_cones` cones it down). The repo-level clone path remains non-sparse — the per-workarea worktree is where sparsity lives, matching `design/02 §3.2`. (4) The `repo.size_warning` broadcast subject (301's deferral) is still un-wired (audit-line only); orthogonal to this task.
- **Deliberate debt:** None. No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code. The single-repo create path seeding `empty_cones()` rather than the resolver output is not debt — it is correct V0.1 single-repo behavior (no workspace/repo cone defaults exist to inherit yet); 306/307 swap it when they wire the multi-repo loop + the resolver call, per the inline comment.
- **Smoke-gate state:** **NEW capability `sparse-cone-clone` added + GREEN.** Registered in `scripts/smoke.manifest` after `project-repo-clone`. Full `scripts/smoke.sh` run PASSED end-to-end (70s, all checks) — `PASS sparse-cone-clone` confirmed: blobless+sparse add → clone → create workspace+workarea → `SetCones a/` → assert `sparse-checkout list` reports `a`, `a/` materialized, `b/`+`c/` collapsed, and the sparse index is active (`ls-files --sparse` shows `b/`/`c/` directory entries). SKIPs cleanly when git lacks the `sparse-checkout` subcommand. `cargo deny check` green (no new workspace pins).
