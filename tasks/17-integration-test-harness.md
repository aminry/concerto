# Task 17 — Integration Test Harness

| Field | Value |
|---|---|
| Phase | 1 |
| Size | small (≤4h) |
| Depends on | 13, 15 |
| Touches subsystem(s) | 01 (Runtime), 10 (Local API), 09 (Persistence) |
| Smoke gate | unchanged |

## Goal
Build a reusable test harness, `crates/test-harness`, that spawns a `concerto-core` in a tempdir, returns a connected gRPC client, and tears down cleanly. Every later Phase 2/3 task that adds integration tests uses this harness instead of reinventing the spawn-and-connect dance. Without this, integration tests will diverge.

## Inputs to read before starting
- `design/01_Core_Daemon_Runtime.md` §10 (testing — `concerto-core-test` harness).
- `design/10_Local_API_Protocol.md` §10 (integration testing — full RPC round-trips for every service over UDS).
- `design/09_Persistence.md` §10 (testing — `sqlx::test` for in-memory DB; integration tests use real SQLite on tempfs).
- `tasks/15-smoke-gate-v1.md` → "Handoff Notes".
- `tasks/16-logging-discipline.md` → "Handoff Notes".

## Scope — in
Create `crates/test-harness/` (a `dev-deps`-only crate — not a member of the production build, but a workspace member).

API:

```rust
pub struct CoreUnderTest {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub socket_path: PathBuf,
    pub db_path: PathBuf,
    process: Child,
}

impl CoreUnderTest {
    /// Spawns a fresh concerto-core in a tempdir, waits for the UDS socket,
    /// returns a handle. Drop kills the process and cleans up.
    pub async fn spawn() -> Result<Self>;

    /// Returns a connected gRPC client for the given service. The client
    /// uses a fresh connection per call.
    pub async fn runtime_client(&self) -> Result<RuntimeClient<Channel>>;
    pub async fn workspaces_client(&self) -> Result<WorkspacesClient<Channel>>;
    pub async fn workareas_client(&self) -> Result<WorkareasClient<Channel>>;
    pub async fn sessions_client(&self) -> Result<SessionsClient<Channel>>;

    /// Direct DB read access (read-only) for assertions.
    pub async fn db(&self) -> Result<SqlitePool>;

    /// Graceful shutdown; returns when the core process has exited.
    pub async fn shutdown(self) -> Result<()>;
}

impl Drop for CoreUnderTest {
    fn drop(&mut self) { /* SIGKILL fallback */ }
}
```

Implementation:
- `spawn` uses `std::process::Command` (or `tokio::process::Command`) to run `cargo run --bin concerto-core` with `CONCERTO_CONFIG_DIR` and `CONCERTO_DATA_DIR` set to a fresh tempdir (use `tempfile::TempDir`).
- Polls for the UDS socket to appear with a 15s timeout.
- The client constructors build a `tonic::transport::Channel` using `service_fn` over a `UnixStream` connector.
- `shutdown` sends SIGTERM, waits for exit with a 10s timeout, then SIGKILL.
- `Drop` falls back to SIGKILL if the process is still alive.

Migrate `crates/core/tests/grpc_runtime.rs` (Task 13's integration test) to use the new harness as a sanity check.

Add a `crates/test-harness/README.md` documenting:
- The harness is intended for crate-level integration tests (`tests/*.rs`), not for unit tests.
- Each test that uses the harness should `#[tokio::test(flavor = "multi_thread")]`.
- Tests share no state — each call to `spawn()` produces a fully isolated Core.

## Scope — out
- No Desktop / Tauri test harness (V1.0).
- No "fast spawn" (`#[sqlx::test]`-style) variant that uses an in-process Core; that's a V1.5 optimization.
- No record/replay / mock Core.

## Public interface this task locks
- Rust: `crates/test-harness/src/lib.rs` — `pub struct CoreUnderTest`, `pub async fn spawn() -> Result<Self>`, plus the client accessors.
- Convention: every Phase 2+ integration test that needs a live Core uses this harness.

## Implementation notes
- Use `cargo build --bin concerto-core` at harness build time (`build.rs`) to ensure the binary exists when tests run; OR rely on cargo's test target compilation (preferred).
- The harness's `spawn()` should find the `concerto-core` binary via `env!("CARGO_BIN_EXE_concerto-core")` — cargo sets this env var for test binaries that depend on the crate.
- Actually: `CARGO_BIN_EXE_<name>` is only set if the test crate itself defines the binary or depends on the bin. Since `crates/test-harness` is a separate crate, the right pattern is to have callers' `Cargo.toml` include `[[bin]]` ... no wait. The simplest pattern: `cargo build --bin concerto-core` runs as part of the test setup if needed; or the harness expects `CARGO_TARGET_DIR/debug/concerto-core` to exist and uses `cargo build` as a fallback.
- A cleaner pattern: rely on `assert_cmd::cargo::cargo_bin("concerto-core")` (`assert_cmd` is a popular test-only dep). Add `assert_cmd = "2"` as a dev-dep.
- The DB read accessor opens a separate SqlitePool (read-only) to the core's DB file — fine because WAL allows readers while the writer is running.
- Use `tracing::warn!` if shutdown takes more than 5s.

## Verification
1. `cargo build --workspace` → succeeds (test-harness compiles as a dev-dep target).
2. `cargo test -p test-harness` → harness's own self-tests pass (spawn-shutdown round-trip; spawn-and-grpc-call; concurrent harness instances don't collide).
3. `cargo test -p concerto-core grpc_runtime` → still passes after migration.
4. Time the harness: a single `spawn()` should take < 5 seconds on a clean machine; if slower, document as Open Question.
5. `cargo clippy --workspace -- -D warnings` → clean.
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] Existing Task 13 integration test successfully migrated to harness.
- [ ] Concurrent harness instances verified isolation-safe.
- [ ] `crates/test-harness/README.md` documents usage.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/test-harness/Cargo.toml` (new)
- `crates/test-harness/src/lib.rs` (new)
- `crates/test-harness/src/process.rs` (new — Child wrangling)
- `crates/test-harness/src/clients.rs` (new — gRPC client builders)
- `crates/test-harness/README.md` (new)
- `crates/core/tests/grpc_runtime.rs` (modified — uses harness)
- `Cargo.toml` (workspace root, modified — adds test-harness as a member)

## Commit message
```
phase-1: integration test harness

crates/test-harness exposes CoreUnderTest::spawn() — a reusable
helper that boots a fresh concerto-core in a tempdir and returns
connected gRPC clients. Migrates the Task 13 integration test.

Refs: tasks/17-integration-test-harness.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** harness spawns a full Core subprocess; in-process Core variant for fast unit tests deferred.
- **Smoke-gate state:** unchanged.
