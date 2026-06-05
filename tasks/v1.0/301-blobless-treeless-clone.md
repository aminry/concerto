# Task 301 — Blobless / Treeless Clone Strategies + Repo-Size → Strategy Recommendation

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | — |
| Touches subsystem(s) | 02 (Repository Manager) |
| Smoke gate | unchanged |

## Goal
Replace the V0.1 hardcoded full-clone-only path with the three clone strategies `design/02 §2` promises for V1.0 — **full**, **blobless** (`--filter=blob:none`), **treeless** (`--filter=tree:0`) — and add a pre-clone **repo-size → strategy recommendation** so the Desktop New-Project dialog can suggest blobless / blobless+sparse before any bytes are cloned. Today `crates/gix-wrap/src/api.rs::clone_full` is the only clone primitive and `RepoManager::add_repository` (`crates/core/src/repo_manager/actor.rs`) hardcodes `clone_strategy: "full"`. This task: (a) adds a `clone_with_strategy(url, dest, CloneStrategy, progress)` helper + a `CloneStrategy` enum in `gix-wrap` that reuses the FROZEN `cmd::run_streaming` progress machinery without touching `clone_full`; (b) adds an `estimate_repo_size(url) → SizeReport` probe (`git ls-remote --heads` + a HEAD `rev-list --objects --count`) and the `<1 GB → Full / 1–10 GB → Blobless / >10 GB → Blobless+Sparse` heuristic from `design/02 §3.5`; (c) changes `add_repository` to accept + persist a real `CloneStrategy` instead of the string literal, and `clone_repo` to route through `clone_with_strategy`; (d) adds a new **`EstimateRepoSize(EstimateRepoSizeRequest) → SizeReport`** RPC to `repositories.proto`, appended after the three FROZEN RPCs, that `AddRepository` callers invoke *before* adding the repo. After this task a caller can ask "how big is this URL and what should I clone" and then add+clone with the recommended (or overridden) strategy; the real >10 GB-monorepo `<30 s p50` number is a Phase-3 Tier-3 checklist line (no real monorepo in CI).

## Inputs to read before starting
- `design/02_Repository_Manager.md` §3.1 — the routing table: **clone of any strategy is `git` shell-out** (sparse + blobless + depth flags work cleanly only there). The `git status` row's V1.0 amendment is for 303, not this task.
- `design/02_Repository_Manager.md` §3.5 — the **size auto-recommendation heuristic** you implement verbatim: `git ls-remote --heads` + an estimated-size probe (HEAD of default branch + `git rev-list --objects --count`), then `<1 GB → Full`, `1–10 GB → Blobless`, `>10 GB → Blobless + Sparse (with cone picker)`. The user sees it in the New-Project dialog and can override.
- `design/02_Repository_Manager.md` §7.1 — the first-time-clone sequence diagram: `add_project_repository → estimate size (ls-remote) → SizeReport (recommend) → confirmation → Clone(strategy, cones) → git clone --filter=blob:none --sparse --no-checkout → …`. This is the ordering the RPCs must support (estimate is a *separate* call before add+clone).
- `design/02_Repository_Manager.md` §4 — `concerto-state.json` (durable repo-local state, NOT SQLite) carries `size_bytes` + `object_count`; persist the probe's measured numbers there.
- `design/02_Repository_Manager.md` §5.3 — emit `repo.size_warning` (broadcast) when a repo crosses the >10 GB threshold and a non-sparse strategy was chosen.
- `design/02_Repository_Manager.md` §12 R-1 — **treeless is hidden from the UI for V1.0** (available only via `concerto.json` / an explicit strategy arg). The recommendation heuristic NEVER returns treeless; it is reachable only when a caller passes it explicitly. Do not surface it in any recommendation or UI-facing field.
- `crates/gix-wrap/src/api.rs` — `clone_full` (FROZEN, Task 18 — **add a sibling, never edit it**) + the `progress::parse_line` stderr parser + the `ProgressSink`/`CloneProgressEvent` types you reuse. `crates/gix-wrap/src/cmd.rs` — `run` and `run_streaming` (reuse for the new strategy clone + the size probe).
- `crates/core/src/repo_manager/actor.rs` — `add_repository` (line ~95, hardcodes `clone_strategy: "full"` in two places — the `NewRepository` row and the returned `Repository`) and `clone_repo` (line ~171, calls `gixw::clone_full`). Both change to honor a real strategy.
- `crates/persist/src/repositories.rs` + `crates/persist/src/api.rs` — `NewRepository`/`Repository` structs; the `clone_strategy` column already exists (migration 0001, accepts `full | blobless | treeless`). No migration needed — you write a real value into the existing column.
- `crates/proto/proto/concerto/v1/repositories.proto` — the header reserves `EstimateConeSize`/`PrewarmBlobs`/`Fetch` names; `EstimateRepoSize` is NOT pre-reserved there but the three RPCs (`AddRepository`/`Clone`/`ListByProject`) and all field numbers ARE FROZEN. Append the new RPC + messages without renumbering.
- `crates/core/src/handlers/repositories.rs` — the handler surface (`add_repository`/`clone`/`list_by_project`); add the `estimate_repo_size` handler here.
- `tasks/v1.0/PHASE3_PLANNING.md` §2 (the 301 rows: separate `EstimateRepoSize` RPC before `AddRepository`; `AddRepository` gains a real `strategy` arg) + §4.6 (`CloneStrategy`/`SizeReport` FROZEN by 301).

## Scope — in
- **`gix-wrap`:** a `CloneStrategy` enum (`Full`, `Blobless`, `Treeless`) + a `with_sparse: bool` companion (the recommendation's "Blobless + Sparse" is `Blobless` strategy + sparse flag; the actual sparse-checkout init/set is **Task 302's** job — 301 only passes `--sparse --no-checkout` flags to the clone when `with_sparse` is set, so the worktree lands empty for 302 to populate). A `clone_with_strategy(url, dest, strategy, with_sparse, progress)` fn that maps the strategy to git filter flags and reuses `cmd::run_streaming` + the existing progress parser. `clone_full` stays byte-for-byte unchanged (it becomes the `Full`/no-sparse path that `clone_with_strategy` may delegate to, or a thin wrapper — keep `clone_full` callable for back-compat).
- **`gix-wrap`:** `estimate_repo_size(url) → SizeReport` — `git ls-remote --heads` for the branch list + ref count, and a HEAD object-count probe (`git rev-list --objects --count` against a `--filter=blob:none --bare --depth=…`-style cheap probe, OR `ls-remote` size hints where a true count is too expensive — pick the cheapest probe that yields a usable byte estimate; document the exact commands in the fn doc-comment). `SizeReport` carries the estimated `size_bytes`, `object_count`, branch count, and the recommended `CloneStrategy` + `with_sparse` flag.
- **`RepoManager`:** `add_repository` gains a `strategy: CloneStrategy` (+ `with_sparse`) parameter; persists the real strategy string into the `repositories.clone_strategy` column and returns it. `clone_repo` routes through `clone_with_strategy`. A new `estimate_size(url) → SizeReport` method on the handle.
- **`RepoManager`:** after a successful clone, write `size_bytes` + `object_count` (from the probe, or measured post-clone via `git count-objects -v`) into the repo's on-disk `concerto-state.json` per `design/02 §4`. Emit `repo.size_warning` on broadcast when a >10 GB repo was cloned non-sparse.
- **proto + handler:** `EstimateRepoSize` RPC + `EstimateRepoSizeRequest`/`SizeReport` messages, appended; `AddRepoRequest` gains a `clone_strategy` field (+ `with_sparse`) appended at the next free field number. The `estimate_repo_size` handler delegates to `RepoManager::estimate_size`.
- Tests (Tier 1, co-located): clone a small `file://` fixture with each of the three strategies, assert the worktree/object-db shape (blobless: commits+trees present, blobs lazy; treeless: trees lazy; with_sparse: empty worktree, `--no-checkout` honored); `estimate_repo_size` against a small fixture returns a populated `SizeReport` with the `<1 GB → Full` recommendation; the heuristic boundary table (`size_bytes → strategy`) is a pure table-driven unit test; treeless is never returned by the recommendation; `concerto-state.json` is written with `size_bytes`/`object_count`.

## Scope — out
- **Sparse-checkout init / set / cone configuration / sparse-index** — **Task 302** (`design/02 §3.2`). 301 only passes the `--sparse --no-checkout` clone flags so 302 can `sparse-checkout init --cone` into an empty worktree; 301 does NOT write cones.
- **Idle / eager blob prewarm + the `PrewarmBlobs` RPC** — **Task 304** (`design/02 §3.3`). A blobless clone leaves blobs lazy; 301 does not prefetch them.
- **The cone-size telemetry RPC (`EstimateConeSize`/`ConeStats`)** — **Task 305**. 301's `EstimateRepoSize` is the *repo-level* pre-clone probe; 305's is the *cone-level* index probe. Distinct RPCs, distinct messages.
- **Multi-repo workspace wiring** — **Task 306**. 301 is per-repository.
- **Desktop New-Project dialog UI** that renders the `SizeReport` recommendation — **Task 322** (`design/02 §15` / `design/15`). 301 ships the RPC the dialog calls.
- The real >10 GB monorepo `<30 s p50` clone — **Tier-3** Phase-3 checklist; 301 proves strategy + recommendation logic on small `file://` fixtures.
- `treeless` in any UI surface (R-1).

## Public interface this task locks
- **Rust (FROZEN), `crates/gix-wrap/src/api.rs`:**
  - `pub enum CloneStrategy { Full, Blobless, Treeless }` (serializes to the existing `repositories.clone_strategy` TEXT values `full | blobless | treeless` — implement `as_str`/`FromStr`/`Display` mapping to exactly those lowercase strings; an unknown string is a hard error, not a silent `Full`).
  - `pub async fn clone_with_strategy(url: &str, dest: &Path, strategy: CloneStrategy, with_sparse: bool, progress: Option<ProgressSink>) -> Result<()>` — `clone_full`'s signature and body stay unchanged.
  - `pub struct SizeReport { pub size_bytes: u64, pub object_count: u64, pub branch_count: u32, pub recommended: CloneStrategy, pub recommend_sparse: bool }` + `pub async fn estimate_repo_size(url: &str) -> Result<SizeReport>`.
- **proto (FROZEN field numbers), `repositories.proto`:** `EstimateRepoSizeRequest { string url = 1; }`; `SizeReport { uint64 size_bytes = 1; uint64 object_count = 2; uint32 branch_count = 3; string recommended_strategy = 4; bool recommend_sparse = 5; }`; `rpc EstimateRepoSize(EstimateRepoSizeRequest) returns (SizeReport);` appended to `service Repositories` after `ListByProject`. `AddRepoRequest` gains `string clone_strategy = 5; bool with_sparse = 6;` (next free numbers after `default_branch = 4`). The existing 3 RPCs + all existing field numbers are unchanged.
- **The heuristic (FROZEN, `design/02 §3.5`):** `< 1 GB → Full`, `1–10 GB → Blobless`, `> 10 GB → Blobless + sparse`. The recommendation never returns `Treeless`.

## Implementation notes
- **`clone_full` is FROZEN** (Task 18, in `docs/interfaces/rust-api.md`). Add `clone_with_strategy` as a sibling; if it shares the spawn/progress plumbing, factor a private `clone_inner(args, …)` that both call — but `clone_full`'s public signature and observable behavior must not change (its tests stay green).
- **Filter flags:** `Blobless → --filter=blob:none`, `Treeless → --filter=tree:0`, `Full →` no filter. `with_sparse → --sparse --no-checkout` (per the §7.1 diagram). Append `--progress` (the parser keys off it) and keep `GIT_TERMINAL_PROMPT=0` (already set in `cmd.rs`).
- **The size probe must be cheap and must not block boot** — it runs per-add (and on the explicit `EstimateRepoSize` call), never on the Core boot path. `git ls-remote` is one network round-trip; do not do a full clone to measure. If a true byte estimate is not cheaply obtainable from the remote, document the approximation you use (e.g. object count × an average-object-size constant, or git's own ls-remote size advertisement when present) and FREEZE that approximation in the fn doc-comment. A probe failure (private repo, offline) surfaces as `Error::Git` — the caller falls back to letting the user pick a strategy manually (do not default-recommend on a failed probe).
- **Cross-platform:** shell-out `git` must work on the Windows + Linux CI lanes (Task 113). Use only `tokio::process` / `std::path`; no `std::os::unix`. Filter clones are supported by git ≥ 2.19 everywhere in the matrix.
- **Handler thinness (`§6.1`):** the `estimate_repo_size` handler validates `url` non-empty, delegates to `RepoManager::estimate_size`, maps the result into `SizeReport`. The strategy string from `AddRepoRequest.clone_strategy` is parsed via `CloneStrategy::from_str` in the handler/manager; an unrecognized strategy → `INVALID_ARGUMENT`.
- **`concerto-state.json`:** a small repo-local JSON at `<repo.local_path>/.git/concerto-state.json` (the §4 layout puts it under `git/`). Write `size_bytes`/`object_count`/`last_fetch_at`; read-modify-write so a future field (304's `prefetch_cursor`) is not clobbered. This is NOT SQLite — it travels with the repo dir.
- Regen: proto changed ⇒ `./scripts/regen-interfaces.sh` updates `docs/interfaces/proto.md`; the new `CloneStrategy`/`SizeReport` Rust surface updates `docs/interfaces/rust-api.md`. Commit both.

## Verification
Tier 1. The `rust` §5.3 set.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-gix-wrap clone` + `cargo test -p concerto-core repo` → strategy-shape tests (blobless/treeless/with_sparse object-db assertions on a `file://` fixture), the heuristic table test, `estimate_repo_size` populated-report test, `concerto-state.json` write test, treeless-never-recommended test pass.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (no new workspace deps expected — reuses `tokio`/`gix`/`serde_json`; if `serde_json` isn't already a `gix-wrap` dep, prefer hand-rolling the tiny `concerto-state.json` writer in `concerto-core` where `serde_json` is already pinned rather than adding a dep to `gix-wrap` — note the choice in Handoff).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`proto.md` gains `EstimateRepoSize`/`SizeReport`; `rust-api.md` gains `CloneStrategy`/`SizeReport`/`clone_with_strategy`).
7. `scripts/smoke.sh` → **unchanged** (302 adds the `sparse-cone-clone` capability; 301 does not touch the gate). Confirm the existing `project-repo-clone` check still passes (it goes through `add-repository`/`clone`, whose signatures changed — the smoke-client `add-repo` defaults `clone_strategy` empty → parses as `Full`, so behavior is preserved).

**Tier-1 scope.** All logic is CI-provable on `file://` fixtures: strategy → git filter flags, the size heuristic, the recommendation, persisted strategy, `concerto-state.json`. The real >10 GB-monorepo `<30 s p50` clone (`design/00 §7.7`) is **not** covered here — it is the Phase-3 Tier-3 checklist line "sparse+blobless clone a real >10 GB monorepo and confirm <30 s p50 workspace creation."

## Definition of Done
- [ ] `CloneStrategy` enum + `clone_with_strategy` added to `gix-wrap`; `clone_full` byte-for-byte unchanged
- [ ] `estimate_repo_size` + `SizeReport` implement the `§3.5` heuristic; treeless never recommended
- [ ] `RepoManager::add_repository` accepts + persists a real strategy (no more hardcoded `"full"`); `clone_repo` routes through `clone_with_strategy`
- [ ] `concerto-state.json` written with `size_bytes`/`object_count` (read-modify-write); `repo.size_warning` emitted on a >10 GB non-sparse clone
- [ ] `EstimateRepoSize` RPC + messages appended to `repositories.proto`; `AddRepoRequest.clone_strategy`/`with_sparse` appended; existing field numbers unchanged
- [ ] All Verification commands pass on a clean checkout; smoke gate unchanged + still green
- [ ] No TODO/FIXME/unimplemented!()/todo!() in new code (deliberate seams in Handoff)
- [ ] No files outside Outputs modified
- [ ] Interfaces regenerated + committed
- [ ] Single commit with the message below

## Outputs
- `crates/gix-wrap/src/api.rs` (modified — `CloneStrategy`, `clone_with_strategy`, `estimate_repo_size`, `SizeReport`; `clone_full` untouched)
- `crates/gix-wrap/src/cmd.rs` (modified only if a new env/arg shape is needed — prefer reuse)
- `crates/core/src/repo_manager/actor.rs` (modified — `add_repository`/`clone_repo` honor a real strategy; `estimate_size`; `concerto-state.json` write; `repo.size_warning`)
- `crates/core/src/repo_manager/mod.rs` (modified only if a new submodule for the state-json writer is added)
- `crates/proto/proto/concerto/v1/repositories.proto` (modified — `EstimateRepoSize` RPC + messages; `AddRepoRequest` fields)
- `crates/core/src/handlers/repositories.rs` (modified — `estimate_repo_size` handler; parse `clone_strategy`)
- `crates/persist/src/repositories.rs` / `crates/persist/src/api.rs` (modified only if `NewRepository` needs the strategy threaded — likely just the existing `clone_strategy` field set to a real value, no struct change)
- `crates/gix-wrap/tests/clone_strategies.rs` + `crates/core/tests/repo_size_estimate.rs` (new)
- `docs/interfaces/proto.md` + `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-3: blobless/treeless clone strategies + size→strategy recommendation

Adds CloneStrategy (full|blobless|treeless) + clone_with_strategy to
gix-wrap (clone_full untouched) and estimate_repo_size → SizeReport
implementing the design/02 §3.5 heuristic. RepoManager now persists +
honors a real strategy instead of hardcoded "full"; new EstimateRepoSize
RPC lets a caller probe a URL before adding. Sparse init is Task 302,
prewarm is Task 304. treeless stays hidden from UI (R-1).

Refs: tasks/v1.0/301-blobless-treeless-clone.md
```

## Handoff Notes (filled in when finishing)
- Drift from plan / Open questions for next task / Deliberate debt / Smoke-gate state —
