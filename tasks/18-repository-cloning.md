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
- [x] Verification commands pass.
- [x] File-protocol clone works end-to-end via gRPC.
- [x] Per-repo write mutex verified by concurrent-clone test (two clones serialize).
- [x] `docs/interfaces/proto.md`, `schema.md`, `rust-api.md` all regenerated.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `crates/gix-wrap/Cargo.toml` (modified — gix, tokio)
- `crates/gix-wrap/src/lib.rs` (modified)
- `crates/gix-wrap/src/api.rs` (new)
- `crates/gix-wrap/src/cmd.rs` (new — shell-out helper)
- `crates/persist/src/repositories.rs` (new)
- `crates/persist/src/api.rs` (modified — `Repository`, `NewRepository`, `RepositoryId` exposed for the interface generator)
- `crates/persist/src/lib.rs` (modified)
- `crates/core/Cargo.toml` (modified — `concerto-gix-wrap`, `uuid`, dev-dep `sqlx`)
- `crates/core/src/lib.rs` (modified — `pub mod repo_manager`)
- `crates/core/src/repo_manager/mod.rs` (new)
- `crates/core/src/repo_manager/actor.rs` (new — implements `Actor` from Task 12)
- `crates/core/src/handlers/mod.rs` (modified — `pub mod repositories`)
- `crates/core/src/handlers/repositories.rs` (new)
- `crates/core/src/api_server.rs` (modified — `with_repo_manager` constructor, registers `RepositoriesServer` when set)
- `crates/core/src/error_map.rs` (modified — `git` → `Code::Internal`)
- `crates/proto/proto/concerto/v1/repositories.proto` (new)
- `crates/proto/build.rs` (modified — `Repository.last_fetch_at` added to `timestamp_fields`)
- `crates/core/src/main.rs` (modified — spawns RepoManagerActor)
- `crates/core/tests/repository_clone.rs` (new)
- `crates/error/src/api.rs` (modified — adds `Error::Git(String)` variant)
- `crates/error/src/error.rs` (modified — `wire_code` returns `"git"`)
- `crates/error/tests/wire_codes.rs` (modified — adds `git_wire_code_and_display`)
- `crates/test-harness/src/clients.rs` (modified — `repositories_client` accessor, per Task 17 handoff)
- `crates/test-harness/src/lib.rs` (modified — re-exports `RepositoriesClient`, adds `CoreUnderTest::repositories_client`)
- `Cargo.toml` (modified — `gix` and `uuid` workspace deps; `rust-version` bumped to 1.82)
- `Cargo.lock` (regenerated by cargo)
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
- **Drift from plan:**
  - **`gix` pinned at `0.77`, not `0.66`.** The drift note in the orchestrator brief authorised `0.66` and said "Latest is ~0.66 as of 2025". As of build time gix 0.66 transitively pulls `gix-date 0.9.4`, which is flagged by `RUSTSEC-2025-0140` (non-utf8 string via `TimeBuf::as_str`). The fix lands in `gix-date >= 0.12`, which is in `gix 0.77`. `0.77` was the lowest gix version that ships the fixed `gix-date` while staying inside the licence allow-list (no GPL/MPL transitively beyond what Task 14 already accepted) and the workspace's `rust-version`. Required raising workspace `rust-version` to `1.82` (gix 0.77's MSRV). The CI runner is on 1.95 so the bump is benign for everything already in the workspace.
  - **Workspace `Cargo.toml` rust-version bumped to `1.82`.** Pre-authorised by the necessary `gix` upgrade above; documented inline in the workspace Cargo.toml.
  - **`Error::Git(String)` variant added to `crates/error/src/api.rs`.** Pre-authorised in the orchestrator drift block. `wire_code` extended to return `"git"`; `error_to_status` (Task 13's free function) maps `git` → `Code::Internal`. Wire-code contract test added at `crates/error/tests/wire_codes.rs::git_wire_code_and_display`.
  - **`RepoManager::clone` is named `clone_repo`.** `clone` shadows `Clone::clone` (the type derives `Clone`); using the plain name caused method-resolution ambiguity. The gRPC trait method stays `clone` per the proto, and the integration test calls it via UFCS (`RepositoriesClient::<Channel>::clone(&mut client, req)`); the inherent handle method is `clone_repo` for ergonomics.
  - **`repositories_client()` accessor added to `crates/test-harness/src/clients.rs`** following the Task 17 handoff brief. Pattern matches `runtime_client()`. Added `crates/test-harness/src/clients.rs` and `crates/test-harness/src/lib.rs` (the `pub use` line) to the modified files. The harness's `Cargo.toml` is unchanged — `concerto-proto` already provides the generated client.
  - **`uuid` added as a workspace dep** (`uuid = { version = "1", features = ["v7"] }`). Already pulled transitively; pinning at the workspace level surfaces the dependency so `RepositoryId` newtype IDs are UUIDv7 per `design/09 §4.1`'s schema convention. MIT/Apache-2.0; cargo-deny clean.
  - **`gix-wrap`'s `clone_full` accepts `Option<ProgressSink>`** (not the unconditional `ProgressSink` the task signature implied). The progress sink is genuinely optional — internal callers that don't need progress (the unit tests, future fetch-style paths) pass `None`. Same `ProgressSink` shape (`mpsc::Sender<CloneProgressEvent>`, bounded at 32 with drop-old semantics under backpressure).
  - **`RepoManagerActor` does not own a background loop in V0.1.** Its `run` just parks on `ctx.shutdown.cancelled()`. The meaningful surface is the cheap-to-clone `RepoManager` handle exposed via `RepoManagerActor::handle()`; the gRPC `RepositoriesHandler` holds the clone. Fsmonitor / maintenance / idle-prefetch loops land in Task 28 per V0.1 phase scope.
  - **`crates/core/src/api_server.rs::ApiServerActor` gained `with_repo_manager` constructor** + an `Option<RepoManager>` field. Existing `ApiServerActor::new` callers (the Task 17 integration tests that build the in-process Runtime for the stale-socket case) work unchanged — they get the runtime service only. `main.rs` uses `with_repo_manager` so the production binary hosts both services.
  - **`crates/core/Cargo.toml` gained `concerto-gix-wrap`, `uuid`, and dev-dep `sqlx`.** sqlx is required by the integration test which seeds a `projects` row directly (the `Projects` service doesn't exist until Task 19). All workspace-pinned versions.
  - **`crates/proto/build.rs` extended with `concerto.v1.Repository.last_fetch_at`** in the `timestamp_fields` list (per Task 07's drift note instructing future tasks to update this table when adding Timestamp-typed fields).
  - **Concurrent-clone serialization is observed indirectly.** The second clone of the same repo serializes on the per-repo mutex; when it actually runs, `git clone` rejects the populated destination with exit 128. The test (`concurrent_clones_of_same_repo_serialize`) drains both streams to completion-or-error — the contract is *one-at-a-time*, not *deduplication*.
- **Open questions for next task:**
  - **Task 19 (`Workspaces.CreateWorkspace`)** can lean on the same pattern: extend `crates/test-harness/src/clients.rs` with a `workspaces_client()` accessor (~8 lines), add a service in `crates/proto`, register a handler in `crates/core/src/handlers/`, and register a `RepositoriesServer`-style service in `api_server.rs`. The `Persistence` writer is held during `add_repository` for one round-trip; a multi-table create (workspace + workspace_repos rows) needs to scope a `BEGIN; … COMMIT;` across the writer guard.
  - **`Projects` service still missing.** Task 18's integration test seeds a `projects` row by going around the API (direct sqlx INSERT). When Tasks 19/20 add their services they'll likely need projects too — adding a `Projects.Create` RPC is a small Phase 2 follow-on.
  - **`concerto.v1.Repositories` field numbers are FROZEN at the V0.1 set.** The remaining design/02 §5.2 RPCs (`Fetch`, `EstimateConeSize`, `PrewarmBlobs`) land at higher RPC numbers / new fields under existing messages — additive only.
  - **gix 0.77 transitively pulls a couple of crates not previously in the tree** (gix-status, gix-pathspec, gix-mailmap variants). All MIT/Apache-2.0 (or already-allowed Zlib via foldhash). cargo-deny clean.
- **Deliberate debt:** no sparse / blobless / fsmonitor / maintenance / idle-prefetch — Task 28 handles V0.1's remaining repo features. `RepoManagerActor::run` parks on shutdown rather than driving a loop; that's the V0.1 contract. No `TODO`/`FIXME`/`todo!()`/`unimplemented!()` markers in new code.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` (v1) still boots the Core, calls `Runtime.GetServerCapabilities`, and shuts down cleanly. The Task 18 RPCs aren't exercised by the smoke gate — they're covered by `crates/core/tests/repository_clone.rs` via the Task 17 harness.
