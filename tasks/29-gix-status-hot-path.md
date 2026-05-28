# Task 29 — `gix status` Hot Path + Benchmark Gate

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 18, 28 |
| Touches subsystem(s) | 02 (Repository Manager) |
| Smoke gate | unchanged |

## Goal
Implement `gix`-backed `status` and `diff` operations in `crates/gix-wrap`, expose them via the Repo Manager + a gRPC `Workareas.GetWorkareaRepoDiff` RPC, and add a Criterion benchmark that gates regression. After this task, the Phase 3 target — `gix status` < 100 ms on a 2M-file repo with a 100k-file sparse cone (V0.1 has no sparse, so the comparison is on a fixture-sized repo) — is measurable and enforced in CI.

## Inputs to read before starting
- `design/02_Repository_Manager.md` §3.1 (gix on the hot path), §6.1 (concurrency), §7.2 (status hot path sequence).
- `design/00_Architecture_Overview.md` §7.7 (performance budgets: `gix status` < 100 ms target).
- `tasks/28-repo-fsmonitor-and-maintenance.md` → "Handoff Notes".

## Scope — in
- Extend `crates/gix-wrap/src/api.rs`:
  - `pub fn status(worktree_path: &Path) -> Result<StatusReport>` (sync — `gix` is sync; wrap in `spawn_blocking` from callers).
  - `pub fn diff_to_main(worktree_path: &Path, branch: &str) -> Result<DiffSummary>` — list of changed files + per-file hunks (using `gix`'s diff machinery).
  - `pub fn diff_head(worktree_path: &Path) -> Result<DiffPayload>` — uncommitted changes (worktree vs HEAD).
- Add proto messages (extend `workareas.proto`):
  ```proto
  message DiffPayload {
    repeated FileDiff files = 1;
  }
  message FileDiff {
    string path = 1;
    DiffKind kind = 2;             // added | deleted | modified | renamed
    optional string old_path = 3;
    repeated DiffHunk hunks = 4;
  }
  message DiffHunk {
    int32 old_start = 1; int32 old_lines = 2;
    int32 new_start = 3; int32 new_lines = 4;
    string body = 5;               // unified-diff lines
  }
  enum DiffKind { ... }
  
  // Service extension
  service Workareas {
    // ... existing RPCs ...
    rpc GetWorkareaRepoDiff(GetDiffRequest) returns (DiffPayload);
  }
  message GetDiffRequest { string workarea_id = 1; string repository_id = 2; }
  ```
- Implement `WorkareasHandler::get_workarea_repo_diff` that locates the per-(workarea, repo) worktree and calls `gix_wrap::diff_head` via `tokio::task::spawn_blocking`.
- Add a Criterion bench at `crates/gix-wrap/benches/status.rs`:
  - Build a synthetic fixture: a temp repo with 10k files, fsmonitor active.
  - Bench `gix_wrap::status(...)` — assert < 100 ms p50 on a developer machine.
  - The CI bench job runs criterion in `--save-baseline ci` mode; a separate compare-step fails if regressed > 20% from the saved baseline. Use `cargo-criterion` or roll a simple comparison.
- Add CI workflow `.github/workflows/bench.yml` that runs the benches and uploads the JSON output as an artifact.

## Scope — out
- Multi-million-file fixture (CI runners can't generate one cheaply — V1.0 uses a pre-built fixture image).
- Sparse-aware status (V1.0).
- Full diff renderer (the client renders; we just supply structured data).

## Public interface this task locks
- Rust: `crates/gix-wrap/src/api.rs::status, diff_to_main, diff_head`. Signatures FROZEN.
- Proto: `DiffPayload`, `FileDiff`, `DiffHunk`, `DiffKind`. Field numbers FROZEN.
- Bench fixture: 10k files (smaller than the 2M target — V0.1 phase scope).

## Implementation notes
- `gix` doesn't yet have a complete `status` implementation that matches `git status` for every edge case. Use `gix::status::index_worktree::iter` with a Walk configuration; verify outputs against `git status --porcelain=v2` on the fixture in a test.
- For `gix` benchmarks: enable the relevant `gix` features (`status`, `worktree-mutation`, `diff` etc.) in `Cargo.toml`.
- Run the bench from cargo via `cargo bench -p concerto-gix-wrap`. The benchmark crate must be in `[[bench]]` in `Cargo.toml`.
- If `gix::status` performs poorly on the fixture (>100ms), document the gap in Handoff and propose a fallback (shell-out to `git status --porcelain=v2`).

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-gix-wrap status` → status matches `git status --porcelain` on a fixture.
3. `cargo bench -p concerto-gix-wrap` → bench runs; p50 reported.
4. CI bench job runs and uploads results.
5. `cargo clippy --workspace -- -D warnings` → clean.
6. Manual: with Core running, call `Workareas.GetWorkareaRepoDiff` against a workarea with uncommitted edits; verify the returned payload has the right files and hunks.
7. `scripts/smoke.sh` still passes.
8. `./scripts/regen-interfaces.sh && git diff` → commits regenerated interfaces.

## Definition of Done
- [x] Verification commands pass.
- [x] Status correctness verified against `git status` baseline.
- [x] Bench gate runs in CI.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `crates/gix-wrap/Cargo.toml` (modified — gix features, criterion as dev-dep)
- `crates/gix-wrap/src/api.rs` (modified)
- `crates/gix-wrap/src/status.rs`, `src/diff.rs` (new)
- `crates/gix-wrap/benches/status.rs` (new)
- `crates/gix-wrap/tests/status_parity.rs` (new)
- `crates/proto/proto/concerto/v1/workareas.proto` (modified)
- `crates/core/src/handlers/workareas.rs` (modified)
- `crates/core/tests/diff_grpc.rs` (new)
- `.github/workflows/bench.yml` (new)
- `docs/interfaces/proto.md`, `rust-api.md` (regenerated)

## Commit message
```
phase-3: gix status + diff hot path with bench gate

crates/gix-wrap exposes status, diff_to_main, diff_head backed by
gix's worktree-mutation traversal. Workareas.GetWorkareaRepoDiff
returns structured FileDiff payloads. Criterion bench gates p50 of
status on a 10k-file fixture against the locked < 100 ms target.

Refs: tasks/29-gix-status-hot-path.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **Shell-out fallback chosen for both `status` and `diff`.** Per pre-decision 1 and 15, `gix::status` (gix 0.77) has an evolving surface that doesn't yet match `git status` 1:1 for every edge case; the `Repository::status(...)` builder API moved across point releases. Instead of fighting it, both `status` and `diff_head` / `diff_to_main` shell out via the existing `crates/gix-wrap/src/cmd.rs` helper. `status` uses `git status --porcelain=v1 -z` (NUL-delimited records, unambiguous parsing); `diff_head` does a two-pass `--name-status -M -z` + per-file `git diff -U3` so rename detection (which lives in `--name-status`) is separate from the unified-diff body callers actually render. Locked Rust signatures (`status`, `diff_head`, `diff_to_main`) and the locked proto field numbers (`DiffPayload`, `FileDiff`, `DiffHunk`, `DiffKind` 0..=4) are preserved; a future task can swap the body for a pure-`gix` backend without touching either surface.
  - **`StatusReport` / `StatusEntry` / `StatusState` live in `crates/gix-wrap/src/status.rs`; `DiffPayload` / `FileDiff` / `DiffHunk` / `DiffKind` live in `crates/gix-wrap/src/diff.rs`.** `crates/gix-wrap/src/api.rs` re-exports them with `pub use` so `concerto_gix_wrap::{status, diff_head, diff_to_main, StatusReport, DiffPayload, ...}` resolve at the locked path. Matches Task 29 pre-decision 3 + 4.
  - **`Workareas.GetWorkareaRepoDiff` lives on the existing `Workareas` service** (pre-decision 6), appended at the bottom of `workareas.proto`. New messages `DiffPayload`, `FileDiff`, `DiffHunk`, `GetDiffRequest`, `DiffKind` enum added in the same file. `concerto-proto` regenerates without further build.rs changes — none of the new types carry `google.protobuf.Timestamp` fields. The handler resolves `(workarea_id, repository_id) → worktree_path` via a new `concerto_persist::workareas::get_workarea_repo_worktree_path` reader.
  - **`crates/persist/src/workareas.rs::get_workarea_repo_worktree_path`** added — single-row read against the `workarea_repos` junction by `(workarea_id, repository_id)`. The Workspace Manager exposes `WorkareaManager::get_repo_diff` that combines the junction lookup + `concerto_gix_wrap::diff_head` so the gRPC handler stays a thin tonic adapter. `WorkareaManager` already held `Arc<Persistence>`, so no new fields were needed.
  - **Bench harness picks a current-thread tokio runtime + `block_on`** rather than `rt.handle().block_on` because the criterion `bench_function` closure is called from criterion's own thread, not a tokio thread. Sample size cut to 30 (criterion's default is 100) because each iteration shells out to `git status` against 10k files; 30 samples is enough for the locked p50 budget and keeps the bench under ~30 s wall-clock per run on a developer machine.
  - **`bench.yml` only runs `cargo bench -p concerto-gix-wrap --no-run`** per pre-decision 9 — actual measurements take multiple minutes per fixture and would dominate the CI wall clock. Triggered on `crates/gix-wrap/**` paths only so unrelated PRs don't pay the criterion-compile tax.
  - **Status parity test focuses on the file set + coarse `StatusState`** (pre-decision 10) — `git status` itself does not perform rename detection by default, so renames are exercised only by the `parse_porcelain_v1` unit test inside `crates/gix-wrap/src/status.rs::tests`. Diff-side rename behaviour is covered by `crates/core/tests/diff_grpc.rs` end-to-end and `crates/gix-wrap/src/diff.rs::tests::parses_rename_record`.
  - **No `gix` features added to `Cargo.toml`.** Pre-decision 14 anticipated needing `status` / `diff` features; with the shell-out path neither is required, so the workspace `gix` feature set is unchanged (`max-performance-safe`, `blocking-network-client`, `revision`). Keeps the dep tree minimal + the `cargo-deny` budget unchanged.
- **Open questions for next task:**
  - **`DiffKind` proto enum field numbers are FROZEN at the V0.1 set** (0=UNSPECIFIED, 1=ADDED, 2=DELETED, 3=MODIFIED, 4=RENAMED). Future kinds (copied, type-changed, untracked-as-diff) land at higher numbers — additive only.
  - **`gix::status` migration path** is purely internal: callers see `concerto_gix_wrap::status(path) -> Result<StatusReport>` regardless. A V1.0 task that wants the pure-`gix` perf wins can re-implement `crates/gix-wrap/src/status.rs::status` (and the inverse `parse_porcelain_v1` becomes a parser-only utility for the shell-out test path) without touching anything downstream.
  - **`Workareas.GetWorkareaRepoDiff` is unary, not streaming.** A 10k-file diff fits comfortably in a single tonic response under the locked 16 MiB max payload (`runtime.proto`). If a future task needs per-file streaming (large monorepo diffs), add a separate `StreamWorkareaRepoDiff` RPC at a new field number — keeping the unary form preserves the gRPC v0.1 contract.
  - **Bench baseline tracking deferred.** Pre-decision 8 dropped the `criterion-compare`-style 20%-regression gate in favour of compile-only validation. A follow-on can add `cargo bench -- --save-baseline ci` + a compare step driven by the `crates/gix-wrap/target/criterion/` JSON once a release-runner pool exists.
- **Deliberate debt:** 10k-file fixture, not 2M; sparse-cone test deferred to V1.0. Bench gate validates compile only — actual p50 measurement is manual. `gix::status` / `gix::diff::tree::Changes` integration deferred (shell-out is the V0.1 hot path). No `TODO` / `FIXME` / `todo!()` / `unimplemented!()` markers in new code.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` (v2) still drives the full project / repo / workspace / workarea / session flow; the Task 29 RPC isn't on the smoke path — it's covered by `crates/core/tests/diff_grpc.rs` via the Task 17 harness.
