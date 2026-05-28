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
- [ ] Verification commands pass.
- [ ] Status correctness verified against `git status` baseline.
- [ ] Bench gate runs in CI.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

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
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** 10k-file fixture, not 2M; sparse-cone test deferred to V1.0.
- **Smoke-gate state:** unchanged.
