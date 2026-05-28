# Task 18 — Repository Cloning (Full Clone, No Sparse)

| Field | Value |
|---|---|
| Phase | 2 |
| Size | medium (1–3d) |
| Depends on | 09, 13, 17 |
| Touches subsystem(s) | 02 (Repository Manager), 09 (Persistence) |
| Smoke gate | unchanged |

## Goal
Add the `crates/gix-wrap` and `crates/core` plumbing to clone a git repository (full clone only — no sparse, no blobless in V0.1) and register it in the `repositories` table. After this task, a gRPC client can call `Repositories.AddRepository` + `Repositories.Clone` and end up with a usable `.git` directory on disk at `~/concerto/repos/<repo_id>/`.

## Inputs to read before starting
- `design/02_Repository_Manager.md` §1–3 (scope, V0.1 phase scope, command dispatcher §3.1), §4 (directory shape `~/concerto/repos/<id>/...`), §5.1 (Rust API surface — V0.1 subset), §5.2 (gRPC `Repositories` service), §6.1 (concurrency: one write per repository at a time).
- `design/09_Persistence.md` §4.1 — `repositories` table.
- `tasks/17-integration-test-harness.md` → "Handoff Notes".

## Scope — in
- Implement `crates/gix-wrap/src/lib.rs` with V0.1 operations:
  - `pub async fn clone_full(url: &str, dest: &Path, progress: ProgressSink) -> Result<()>`
  - `pub async fn fetch(repo_dir: &Path) -> Result<FetchReport>`
  - `pub async fn list_branches(repo_dir: &Path) -> Result<Vec<BranchRef>>`
  - `pub async fn rev_parse_head(repo_dir: &Path) -> Result<String>`
  - `pub async fn worktree_add(repo_dir: &Path, branch: &str, dest: &Path) -> Result<()>`
  - Internally, route per `design/02 §3.1`: clone shell-outs to `git`; `list_branches` and `rev_parse_head` use `gix`; `worktree_add` shell-outs.
  - All shell-outs go through a small `cmd::run(cmd: &str, args: &[&str], cwd: &Path) -> Result<Output>` helper that captures stdout/stderr + handles non-zero exit codes.
- Add `repositories` table CRUD in `crates/persist/src/repositories.rs`:
  - `pub async fn insert(tx, NewRepository) -> Result<RepositoryId>`
  - `pub async fn get(reader, id) -> Result<Option<Repository>>`
  - `pub async fn list_by_project(reader, project_id) -> Result<Vec<Repository>>`
  - `pub async fn update_last_fetch(tx, id, at) -> Result<()>`
- Add `RepoManagerActor` in `crates/core/src/repo_manager/`:
  - Per-repo write mutex (`HashMap<RepositoryId, Arc<Mutex<()>>>`).
  - `add_repository(project_id, url) -> RepositoryId` (persists row; no clone yet).
  - `clone(repo_id, progress) -> Result<()>` (locks the repo's mutex, runs full clone, updates `last_fetch_at`).
- Add a proto file `crates/proto/proto/concerto/v1/repositories.proto` with the V0.1 surface:
  ```proto
  service Repositories {
    rpc AddRepository(AddRepoRequest) returns (Repository);
    rpc Clone(CloneRequest) returns (stream CloneProgress);
  }
  ```
  Plus the request/response messages and a `Repository` message that mirrors the V0.1 columns of the table.
- Wire a `RepositoriesHandler` in `crates/core/src/handlers/repositories.rs` registered with the gRPC server (similar to Task 13's `RuntimeHandler`).
- Integration test using `test-harness`:
  - Spawn Core, AddRepository for a small public repo (use a tiny fixture; **do not depend on network in CI** — use `git daemon` over loopback or a bundled tarball; the design says "Use a self-hosted gitea / a public sample repo" but CI should avoid network).
  - Best CI approach: create a temp local bare repo in the test, clone from `file://`.
  - Stream `Clone` progress and assert at least one progress message arrives.
  - Verify the repo is on disk + the DB row exists.

## Scope — out
- Sparse-checkout, blobless, treeless (V1.0).
- Fsmonitor setup (Task 28).
- Git maintenance (Task 28).
- Repo-size auto-recommendation (Task 28).
- Pre-fetch (V1.0).
- Submodules / LFS (V1.0).

## Public interface this task locks
- Rust: `crates/gix-wrap/src/api.rs` — `clone_full`, `fetch`, `list_branches`, `rev_parse_head`, `worktree_add`. Signatures listed above are FROZEN.
- SQL: `repositories` table — already created by migration 0001 (Task 09). The Rust functions in `crates/persist/src/repositories.rs` are the canonical API.
- Proto: `repositories.proto` service with two RPCs above. Field numbers frozen.
- On-disk layout: `~/concerto/repos/<repository_id>/git/` for the .git dir; per `design/02 §4`.

## Implementation notes
- Use `tokio::process::Command` for shell-outs so they're async-friendly.
- For clone progress: pipe `git`'s stderr through a parser that recognizes `Receiving objects:  N%` lines and converts to `CloneProgress { bytes_received, objects_received, total_objects }`.
- The progress channel should be an mpsc with bounded capacity (32) — drop old progress events under backpressure rather than blocking the clone.
- Always set `GIT_TERMINAL_PROMPT=0` in subprocess env to fail-fast on credential prompts.
- For `rev_parse_head` and `list_branches`, prefer `gix` (faster + safer); fall back to shell-out only if `gix` doesn't support a code path.
- The CI test should not require network. Create a bare repo locally:
  ```rust
  let bare = tempdir().unwrap();
  Command::new("git").args(["init", "--bare", "."]).current_dir(&bare).status().unwrap();
  // Add a commit to it (push from a working repo, or use git fast-import).
  let url = format!("file://{}", bare.path().display());
  ```
- Wrap `gix` errors via a `From` into `concerto_error::Error::Git(String)` (add this variant to `crates/error`).

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-gix-wrap` → unit tests for each operation pass against a tempfs fixture.
3. `cargo test -p concerto-core repo_manager` → integration test passes (file:// clone, progress stream, DB row created).
4. `cargo clippy --workspace -- -D warnings` → clean.
5. `cargo deny check` → clean (gix has a transitive dep list — verify it still satisfies the license allow-list).
6. Manual: `cargo run --bin concerto-core` + a tiny test client that calls `AddRepository` + `Clone` against a local bare repo.
7. `./scripts/regen-interfaces.sh && git diff docs/interfaces/` → updated; commit.
8. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] File-protocol clone works end-to-end via gRPC.
- [ ] Per-repo write mutex verified by concurrent-clone test (two clones serialize).
- [ ] `docs/interfaces/proto.md`, `schema.md`, `rust-api.md` all regenerated.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/gix-wrap/Cargo.toml` (modified — gix, tokio)
- `crates/gix-wrap/src/lib.rs` (modified)
- `crates/gix-wrap/src/api.rs` (new)
- `crates/gix-wrap/src/cmd.rs` (new — shell-out helper)
- `crates/persist/src/repositories.rs` (new)
- `crates/persist/src/lib.rs` (modified)
- `crates/core/src/repo_manager/mod.rs` (new)
- `crates/core/src/repo_manager/actor.rs` (new — implements `Actor` from Task 12)
- `crates/core/src/handlers/repositories.rs` (new)
- `crates/proto/proto/concerto/v1/repositories.proto` (new)
- `crates/core/src/main.rs` (modified — spawns RepoManagerActor)
- `crates/core/tests/repository_clone.rs` (new)
- `crates/error/src/error.rs` (modified — adds Git variant)
- `docs/interfaces/proto.md`, `schema.md`, `rust-api.md` (regenerated)

## Commit message
```
phase-2: repository cloning (full clone only)

crates/gix-wrap exposes clone_full / fetch / worktree_add wrapping
shell-out + gix per design/02 §3.1. RepoManagerActor serializes
mutations per repo. gRPC Repositories.AddRepository + .Clone work
end-to-end against file:// origin.

Refs: tasks/18-repository-cloning.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** no sparse / blobless / fsmonitor / maintenance — Task 28 handles V0.1's remaining repo features.
- **Smoke-gate state:** unchanged.
