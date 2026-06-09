# Task 305 — `suggest_cones` Interface Seam + `ConeStats` / `EstimateConeSize` RPC (Maestro delegate wired in P4)

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 1 |
| Size | small (≤4h) |
| Depends on | 302 |
| Touches subsystem(s) | 02 (Repository Manager), 08 (Maestro — seam only) |
| Smoke gate | unchanged |

## Goal
Publish the `suggest_cones(repo, issue_text)` interface as an **unwired Maestro-delegate trait seam** (the LLM call lands in P4, Task 411 — `design/02 §3.2` says the Repo Mgr "just publishes the interface") AND implement the **cone telemetry** half for real: `list_paths_in_cone(repo, cones) → ConeStats` (file count + disk-size estimate **from the git index**) plus the `EstimateConeSize(EstimateConeSizeRequest) → ConeStats` gRPC surface. Today there is no cone-size probe and no `suggest_cones` anywhere. This mirrors the README's "stub-until-the-consuming-phase" precedent (like Maestro's `notify_user` stubbed against 14 until P5): the deterministic, CI-provable telemetry ships now; the Maestro delegate is a published seam that returns `UNIMPLEMENTED` until P4 wires it. This task adds (a) a `ConeSuggester` trait + an `Option<Arc<dyn ConeSuggester>>` seam on the Repo Mgr that returns `UNIMPLEMENTED`/empty in P3; (b) `RepoManager::list_paths_in_cone` reading the git index (NOT a filesystem walk) for `(file_count, disk_size_bytes)`; (c) the `EstimateConeSize` RPC + `EstimateConeSizeRequest`/`ConeStats` messages appended to `repositories.proto`. After this task the cone-picker UI (Task 322) and the P4 `create_workspace_from_description` (Task 411) have a real cone-size probe, and the `suggest_cones` seam exists for 411 to wire the Maestro call into without touching the proto/trait again.

## Inputs to read before starting
- `design/02_Repository_Manager.md` §3.2 — **"Plan-mode suggestion: … the Repo Mgr exposes a `suggest_cones(repo, issue_text)` interface per repo. This delegates to the Maestro Agent (08) — *not* implemented here. The Repo Mgr just publishes the interface."** AND **"File-count and size telemetry: For each cone the user considers, the Repo Mgr computes (file count, disk size) from the git index. This drives the cone-picker UI in 15."** This paragraph is the entire task: seam for `suggest_cones`, real impl for the telemetry.
- `design/02_Repository_Manager.md` §5.1 — `list_paths_in_cone(repo, cones: &[ConePath]) → ConeStats` ("file count, disk size estimate") — the Rust API signature.
- `design/02_Repository_Manager.md` §5.2 — `EstimateConeSize(EstimateRequest) → ConeStats` gRPC surface.
- `design/02_Repository_Manager.md` §9 — "**08 Maestro (V1.0+)** — for `suggest_cones` plan-mode call" — the dependency direction (Repo Mgr depends on Maestro for `suggest_cones`; the consumer in P4 is Task 411).
- `design/08_*` (Maestro) — the P4 consumer of `suggest_cones`: `create_workspace_from_description` (Task 411) calls the Repo Mgr cone-suggest + cone-size probes during issue→workspace planning. Read enough to confirm the seam shape 411 will wire (you do NOT implement any Maestro code here).
- `crates/proto/proto/concerto/v1/repositories.proto` — the header explicitly reserves the `EstimateConeSize` name; existing field numbers (`AddRepository`/`Clone`/`ListRepositories` + all messages) are FROZEN. Append the RPC + messages after Task 301's `EstimateRepoSize`, 302's `SetCones`, and 304's `PrewarmBlobs`. (**Coordinate:** 304 owns `PrewarmBlobs`/`PrewarmProgress`; 305 owns `EstimateConeSize`/`ConeStats`. Do not duplicate either.)
- `crates/core/src/handlers/repositories.rs` — the handler surface; add `estimate_cone_size` (unary, NOT streaming — it returns a single `ConeStats`).
- `crates/core/src/repo_manager/actor.rs` — `RepoManager` (where `list_paths_in_cone` + the `ConeSuggester` seam field live). The `UNIMPLEMENTED`-stub pattern to mirror: how V0.1 surfaced not-yet-built RPCs (e.g. the `UpsertProjectMcp` UNIMPLEMENTED stub) — return a `tonic::Status::unimplemented` from the handler when the seam is `None`.
- `crates/persist/src/repositories.rs` + `api.rs` — `cone_defaults_json` may not yet be in the SELECT/`Repository` struct (Task 302 adds it; if 302 landed first, it's there — confirm). The cone-size probe reads the resolved cones (302's resolver) + the git index, not this column directly.
- `crates/gix-wrap/src/api.rs` — `gix` is already a dep; reading the git index (file entries within a cone) is a `gix`-native read path (open repo + `open_index()` + iterate entries filtered to the cone prefixes) — this is the `ConeProbe → gix` arrow in `design/02 §6` and an in-cone read, which is exactly what `gix` is fast at (vs. `status`, which spike 104 routed to shell-out). Reading the index is NOT `gix::status` — no feature bump needed.
- `tasks/v1.0/302-sparse-cone-lifecycle.md` → "Handoff Notes" — the cone storage + the three-layer inheritance resolver (the probe resolves the cones the same way) + the `ConePath` type 302 froze.
- `tasks/v1.0/PHASE3_PLANNING.md` §2 (305 row: `suggest_cones` = **Rust trait seam only, unwired** (delegates to Maestro 08, P4); the telemetry `ConeStats`/`EstimateConeSize` **IS implemented in P3**, reads the git index) + §3 (305 = git index, **no migration**) + §4.4 (305's `suggest_cones` is a *separate* Maestro-delegate seam from 312's `OneShotLlm`; both unwired in P3) + §4.6 (`ConeStats { uint64 file_count = 1; uint64 disk_size_bytes = 2; }` and `EstimateConeSizeRequest { string repository_id = 1; repeated string cone_paths = 2; }` FROZEN by 305).

## Scope — in
- **The `ConeSuggester` seam:** `pub trait ConeSuggester: Send + Sync { async fn suggest_cones(&self, repo: &RepositoryId, issue_text: &str) -> Result<Vec<ConePath>>; }` + an `Option<Arc<dyn ConeSuggester>>` field on `RepoManager` (default `None` in P3). `RepoManager::suggest_cones(repo, issue_text)` delegates to the seam if present, else returns the `UNIMPLEMENTED` signal (empty/`Err` mapped to `Status::unimplemented` at the handler). P4 Task 411 constructs the Maestro-backed impl and injects it; no proto/trait change then.
- **`RepoManager::list_paths_in_cone(repo, cones) → ConeStats`** — open the repo via `gix`, read the (sparse) index, count tracked entries whose path falls under any cone prefix, and sum a disk-size estimate **from the index** (the entries' recorded sizes, NOT a filesystem `stat` walk — `design/02 §3.2` says "from the git index"). Resolve the cone set via 302's resolver when `cones` is empty; honor explicit `cones` otherwise.
- **proto + handler:** `EstimateConeSize(EstimateConeSizeRequest) → ConeStats` (unary); `estimate_cone_size` handler delegates to `list_paths_in_cone`, maps the result into `ConeStats`. (Optionally also expose `SuggestCones` at the gRPC layer if 322/411 need it on the wire — but PHASE3_PLANNING scopes 305 to "interface seam only"; the gRPC `SuggestCones` can be a follow-on if not needed in P3. Decide minimally and note: if added, it returns `UNIMPLEMENTED` until P4.)
- Tests (Tier 1): a `file://` fixture with a known dir tree + a known cone — `list_paths_in_cone([a/])` returns the exact `file_count` of in-cone tracked files + a non-zero `disk_size_bytes` from the index; a cone with no files → `{0, 0}`; the `ConeSuggester` seam returns `UNIMPLEMENTED`/empty when `None` (assert the handler maps to `Status::unimplemented`); an injected mock `ConeSuggester` is delegated to (proves the seam wires, without any Maestro).

## Scope — out
- **The Maestro LLM call behind `suggest_cones`** — **Task 411** (P4). 305 publishes the trait + the `None` stub; 411 injects the real impl. This is the README "wired in P4" pattern.
- **The sparse-cone lifecycle / cone storage / resolver** — **Task 302** (305 reads what 302 wrote).
- **Blob prewarm / `PrewarmBlobs` RPC** — **Task 304** (305 does not prefetch; it only counts).
- **Repo-level size estimate / `EstimateRepoSize` / `SizeReport`** — **Task 301** (that's the *pre-clone* probe; 305's is the *post-clone, per-cone, index-read* probe — distinct RPCs, distinct messages).
- **Desktop cone-picker UI** that renders `ConeStats` — **Task 322**.
- **Any migration** — 305 reads the git index + existing columns; **no migration** (PHASE3_PLANNING §3).
- A filesystem walk for disk size — forbidden; the estimate comes from the index (`§3.2`).

## Public interface this task locks
- **Rust (FROZEN), `crates/core/src/repo_manager`:** `pub trait ConeSuggester: Send + Sync` with `async fn suggest_cones(&self, repo: &RepositoryId, issue_text: &str) -> Result<Vec<ConePath>>`; `pub async fn list_paths_in_cone(&self, repo: &RepositoryId, cones: &[ConePath]) -> Result<ConeStats>` on `RepoManager`. A Rust `ConeStats { pub file_count: u64, pub disk_size_bytes: u64 }` mirroring the proto.
- **proto (FROZEN field numbers), `repositories.proto`:** **`ConeStats { uint64 file_count = 1; uint64 disk_size_bytes = 2; }`** and **`EstimateConeSizeRequest { string repository_id = 1; repeated string cone_paths = 2; }`** (exact per PHASE3_PLANNING §4.6); `rpc EstimateConeSize(EstimateConeSizeRequest) returns (ConeStats);` appended to `service Repositories` after the prior Phase-3 RPCs. Existing field numbers unchanged.
- **The P3 behavior of the unwired seam (FROZEN contract):** `suggest_cones` with no injected `ConeSuggester` → `Status::unimplemented` (NOT an empty success, NOT a panic) — the same convention V0.1 used for not-yet-built RPCs, so 411 wiring is a pure addition.

## Implementation notes
- **Reading the index is a `gix` read path, not `gix::status`.** Open the repo, `open_index()` (reachable under the pinned `gix 0.77` features — spike 104 §5 confirms `gix-index` is reachable), iterate entries, filter to cone prefixes, count + sum `entry.stat.size`. This is the `ConeProbe → gix` arrow in `design/02 §6`. It does NOT touch `gix-status`/`gix-dir` (no feature bump, no `cargo deny` change). If sizes-from-index are unreliable for some entry kinds, document the estimate basis in the fn doc-comment and FREEZE it (the UI wants an order-of-magnitude, per `§3.2`).
- **Honor the sparse index.** On a sparse-cone repo the index is a *sparse* index (directory entries for out-of-cone trees). Count only the in-cone file entries; if the cone you're probing is broader than what's materialized, the probe should still count by reading the cone's tree from the index/objects (the entries exist as tree objects even when blobs are lazy — count tree entries, not on-disk files). Decide and document: the count is "files the cone would materialize," from the index/tree, independent of which blobs are currently fetched.
- **The seam stub must be honest.** `None` `ConeSuggester` → the handler returns `Status::unimplemented("suggest_cones is wired in P4 (Maestro, Task 411)")`. Do NOT return an empty `Vec` (that would look like "no suggestions" and mislead 322). This is the explicit FROZEN contract above.
- **Unary, not streaming.** `EstimateConeSize` returns a single `ConeStats` — a plain unary handler (unlike 304's streaming `PrewarmBlobs`). Keep it simple.
- **Cross-platform:** `gix` index read works on Win/Linux CI lanes (Task 113); no `std::os::unix`. Cone paths are forward-slash git paths everywhere.
- Regen: proto changed ⇒ `./scripts/regen-interfaces.sh`; the new `list_paths_in_cone`/`ConeSuggester`/`ConeStats` Rust surface updates `rust-api.md`. Commit both.

## Verification
Tier 1. The `rust` §5.3 set.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core cone` (+ `estimate_cone`/`suggest_cones`) → `list_paths_in_cone` exact-count + non-zero-size test on a known `file://` fixture cone, empty-cone `{0,0}` test, `ConeSuggester`-`None`→`UNIMPLEMENTED` test, injected-mock-`ConeSuggester`-delegated test pass.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (no new crates; the index read stays inside the pinned `gix 0.77` features — no `gix-status`/`gix-dir`).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`proto.md` gains `EstimateConeSize`/`ConeStats`/`EstimateConeSizeRequest`; `rust-api.md` gains `ConeSuggester`/`list_paths_in_cone`/`ConeStats`).
7. `scripts/smoke.sh` → **unchanged** (305 touches no smoke capability).

**Tier-1 scope + what it does NOT cover.** The cone telemetry (`list_paths_in_cone`/`EstimateConeSize`/`ConeStats`) is deterministic and fully CI-provable on a fixture with a known cone. The `suggest_cones` **Maestro-delegate half is a published seam only** — there is no Maestro in P3, so CI proves the seam returns `UNIMPLEMENTED` when unwired and delegates when an impl is injected; the **real LLM cone suggestion is wired in P4 (Task 411) and judged at the Phase-4 gate** (mirrors the `notify_user`-stubbed-until-P5 precedent). No Tier-3 line is added by this task; the P4 wiring is what's verified later.

## Definition of Done
- [x] `ConeSuggester` trait + `Option<Arc<dyn ConeSuggester>>` seam on `RepoManager`; `None` → `Status::unimplemented` (not empty success)
- [x] `list_paths_in_cone → ConeStats` reads the git **index** (not a filesystem walk) for file count + disk-size estimate
- [x] `EstimateConeSize` unary RPC + `EstimateConeSizeRequest`/`ConeStats` appended; fields exactly `file_count=1`/`disk_size_bytes=2` and `repository_id=1`/`cone_paths=2`
- [x] Index read stays within the pinned `gix 0.77` features (no `gix-status`/`gix-dir`); `cargo deny` green
- [x] Tests: exact-count cone telemetry, empty cone, seam-unimplemented, seam-delegated-to-mock
- [x] All Verification commands pass on a clean checkout; smoke unchanged; interfaces regenerated
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (the `Status::unimplemented` seam is a runtime gRPC status, not the macro — note in Handoff)
- [x] No files outside Outputs modified
- [x] Single commit with the message below

## Outputs
- `crates/core/src/repo_manager/actor.rs` (modified — `ConeSuggester` field + `suggest_cones` delegate + `list_paths_in_cone`) and/or `crates/core/src/repo_manager/cone_stats.rs` (new — the index-read probe) + `crates/core/src/repo_manager/mod.rs` (modified — `pub trait ConeSuggester` export)
- `crates/proto/proto/concerto/v1/repositories.proto` (modified — `EstimateConeSize`/`EstimateConeSizeRequest`/`ConeStats`)
- `crates/core/src/handlers/repositories.rs` (modified — `estimate_cone_size` unary handler; the `suggest_cones`-unimplemented mapping)
- `crates/core/tests/cone_stats.rs` (new)
- `docs/interfaces/proto.md` + `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-3: cone-size telemetry + suggest_cones Maestro seam

Implements list_paths_in_cone → ConeStats (file_count + disk_size_bytes
read from the git index) and the unary EstimateConeSize RPC. Publishes
the ConeSuggester trait seam for plan-mode cone suggestion as an unwired
Maestro delegate (returns UNIMPLEMENTED in P3; Task 411 injects the live
impl in P4). Index read stays inside the pinned gix features — no
gix-status/gix-dir, no deny change.

Refs: tasks/v1.0/305-cone-stats-suggest-seam.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan:** (1) The index read lives in **`crates/gix-wrap/src/api.rs`** (new `pub async fn cone_index_stats(repo_dir, cone_paths) → gixw::ConeStats` + the `ConeStats` struct it mirrors), re-exported via `lib.rs`, NOT in `crates/core/src/repo_manager/cone_stats.rs` as the Outputs implied. Reason: `gix` is a dependency of `gix-wrap`, not of `concerto-core`; putting the gix index-decode there avoids adding a new `gix` workspace dep to core (which would be its own drift / larger deny surface). `cone_stats.rs` in core holds the `ConeSuggester` trait + the core-side `ConeStats` (with a `From<gixw::ConeStats>`) + `ConeSuggestError`, and `actor.rs::list_paths_in_cone` orchestrates (row lookup → resolve cone fallback → `gixw::cone_index_stats`). `crates/gix-wrap/{api.rs,lib.rs}` added to effective Outputs. No new workspace pin; `cargo deny` green. (2) The `suggest_cones` seam returns a typed **`ConeSuggestError { Unwired, Delegate(Error) }`** (a `std::result::Result<Vec<ConePath>, ConeSuggestError>`), not `Result<…, concerto_error::Error>` — `concerto_error::Error` has no `NotImplemented`/`Unimplemented` variant and `error_map.rs` is out of Outputs, so adding one would be drift. The handler-layer mapper **`cone_suggest_error_to_status`** (new `pub fn` in `handlers/repositories.rs`) maps `Unwired → Status::unimplemented(…)` and `Delegate(e) → error_to_status(e)`; the Tier-1 test asserts both. (3) Decided **minimally per Scope-in**: NO gRPC `SuggestCones` RPC in P3 (`PHASE3_PLANNING` scopes 305's `suggest_cones` to a Rust trait seam only). `EstimateConeSize` is the only new RPC. When P4/411 needs `suggest_cones` on the wire it appends the RPC and reuses `cone_suggest_error_to_status` verbatim — no trait/proto rework. (4) `rust-api.md` lists the new `gixw::ConeStats` struct but NOT the free fns `cone_index_stats`/`list_paths_in_cone`/the `ConeSuggester` trait — `regen-interfaces.sh` only captures struct/enum/type definitions from `crates/*/src/api.rs`, never free `pub fn`s (confirmed: `estimate_repo_size`/`clone_full`/`prewarm_blobs_in_cone` are all absent too) and never `src/repo_manager/*` modules. This matches the established behavior Task 302 noted for its `sparse.rs` helpers — not new drift. `proto.md` correctly gained `EstimateConeSize`/`ConeStats`/`EstimateConeSizeRequest`.
- **Open questions for next task:** (1) **322** (Desktop cone-picker) + **411** (`create_workspace_from_description`) consume `EstimateConeSize`/`ConeStats` on the wire — they get `(file_count, disk_size_bytes)` for a candidate cone, read from the index. (2) **411** is the P4 owner that injects the Maestro-backed `ConeSuggester` via `RepoManager::with_cone_suggester(Arc<dyn ConeSuggester>)` — a pure addition: the trait signature (`async fn suggest_cones(&self, repo: &RepositoryId, issue_text: &str) -> Result<Vec<ConePath>>`), the seam field, and the `Unwired → unimplemented` contract are all FROZEN here, so 411 wires the LLM with zero proto/trait change. The live-LLM `suggest_cones` is judged at the Phase-4 gate (the README `notify_user`-stubbed-until-P5 precedent). (3) **`disk_size_bytes` estimate basis is FROZEN as a lower bound:** it sums the index's recorded per-entry `stat.size`; for a **blobless** clone a not-yet-fetched blob reads as size 0, so the byte total under-reports until 304's prewarm materializes the blobs (`design/02 §3.2` wants order-of-magnitude for the picker, which this satisfies). `file_count` is exact regardless of fetch state (it counts file entries present in the index/sparse-index). 322 should label the size as an estimate. (4) `list_paths_in_cone(repo, cones)` has **no workarea/workspace context** in its FROZEN signature, so an empty `cones` falls back only to the repo-level `repositories.cone_defaults_json` layer (then to "count the whole tree"); callers wanting the full three-layer resolution call 302's `resolve_for_workarea_repo(...)` first and pass its output as `cones`. (5) Sparse-index honesty: the probe counts only true file entries (`Mode::{FILE,FILE_EXECUTABLE,SYMLINK}`) and skips sparse-collapsed directory entries (`Mode::DIR`) + submodule gitlinks (`Mode::COMMIT`), so a cone broader than what is currently materialized still counts correctly from the in-cone file entries.
- **Deliberate debt:** None. No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code. The unwired `suggest_cones` seam returns the typed `ConeSuggestError::Unwired` → runtime `tonic::Status::unimplemented(…)` at the handler (a gRPC status value, NOT the `unimplemented!()` macro) — the explicit FROZEN P3 contract (D1), wired live in P4/411.
- **Smoke-gate state:** **Unchanged.** 305 touches no smoke capability (the cone telemetry + the unwired seam are CI-provable via the in-process `RepoManager` harness on `file://` fixtures; no `scripts/smoke.d/*` or `scripts/smoke.manifest` change). Index read stays inside the pinned `gix 0.77` features (`gix.open_index()` / `index.entries()`, the same path Task 303's bench already uses) — no `gix-status`/`gix-dir`, no new workspace pin, `cargo deny check` green.
