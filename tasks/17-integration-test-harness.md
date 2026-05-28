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
- [x] Verification commands pass.
- [x] Existing Task 13 integration test successfully migrated to harness.
- [x] Concurrent harness instances verified isolation-safe.
- [x] `crates/test-harness/README.md` documents usage.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

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
- **Drift from plan:**
  - **`workspaces_client()` / `workareas_client()` / `sessions_client()` accessors NOT shipped.** Task 07 added the messages but the gRPC *services* arrive in Phase 2 (Tasks 19/20/23). Per the orchestrator brief Option (b), only `runtime_client()` is exposed; Phase 2 tasks add the other accessors as the services they front come online. README documents the omission and the upgrade path. Outputs list unchanged.
  - **Package name `concerto-test-harness`** (lib name `concerto_test_harness`) per workspace naming convention; Task spec's pseudocode used `test-harness`. Dev-deps pin path `crates/test-harness`.
  - **Subprocess handle is `tokio::process::Child`, not `std::process::Child`.** Spawn / wait paths are `async`; `Drop` uses `Child::start_kill()` (tokio's documented sync-safe SIGKILL queue) plus the `Command::kill_on_drop(true)` belt-and-suspenders. SIGTERM is sent via `libc::kill(pid, SIGTERM)` because `Child::kill` is SIGKILL.
  - **Stale-socket test kept in-process.** `crates/core/tests/grpc_runtime.rs::stale_socket_file_is_replaced` exercises the actor's stale-socket-handling branch by planting a socket file *before* the Core starts. The harness owns its tempdir only after `spawn()` returns, so the pre-state can't be expressed via the harness's surface. The other three tests in `grpc_runtime.rs` (`get_capabilities_returns_uds_transport`, `get_status_reports_uptime`, `socket_permissions_are_owner_only`) migrated cleanly.
  - **`Handle::exited()` uses `libc::kill(pid, 0)` (ESRCH probe) instead of `Child::try_wait`.** `try_wait` takes `&mut Child`; the polling loop in `wait_for_socket` holds `&self`. The probe loses the actual exit code, but the caller only uses it to decide whether to bail with `EarlyExit` — the synthesised exit-code-0 `ExitStatus` is diagnostics-only.
  - **`spawn()` does NOT invoke `cargo build` itself.** It relies on `assert_cmd::cargo::cargo_bin("concerto-core")` returning a pre-built binary path. Within `cargo test --workspace` this is automatic via the workspace dependency graph; for ad-hoc runs the README documents `cargo build -p concerto-core` first.
  - **`db()` returns a read-only pool (`SqliteConnectOptions::read_only(true)`)** rather than the read+write pool the task signature implied. WAL allows concurrent readers while the Core's writer is live; opening writable would race the Core's writer connection. `max_connections(2)` is enough for assertion patterns.
  - **`assert_cmd = "2"`, `sqlx` (`runtime-tokio + sqlite`), `thiserror`, `libc` (cfg-unix) added to `crates/test-harness` deps.** All MIT/Apache-2.0; cargo-deny clean. `tempfile`, `tonic`, `tower`, `hyper-util`, `tokio`, `tracing`, `concerto-proto` come from the workspace and pre-existing pins.
  - **`crates/test-harness` is `publish = false`.** Dev-deps-only — should never land on crates.io even if the workspace ever publishes.
  - **`concerto-core`'s `[dev-dependencies]` gained `concerto-test-harness = { path = "../test-harness" }`.** Required for the migrated `grpc_runtime.rs` tests. No production-graph cycle: test-harness only depends on `concerto-proto`, and is consumed under `dev-dependencies`.
  - **Self-test wall-clock target.** Task spec called out `<5s/spawn` on a clean machine. Observed on this machine: 5 sequential self-tests (each: spawn + RPC + shutdown) in ~0.08s wall-clock after warm cache; 4 migrated `grpc_runtime` tests in ~1.55s. Well under the budget.
- **Open questions for next task:**
  - **Task 19 / 20 / 23 add the `Workspaces` / `Workareas` / `Sessions` services.** The matching client accessors should land alongside each service in the same task, following the `runtime_client` pattern in `crates/test-harness/src/clients.rs`. Each accessor is ~8 lines (declare a `pub type FooClient = ...`, add a `pub async fn foo_client` that calls into `uds_channel`).
  - **In-process harness variant (the "fast spawn") is deferred to V1.5** per the task spec's Scope — out. The current subprocess-based harness is the only blessed integration-test entry point for V0.1.
  - **`Handle::exited()`'s synthesised `ExitStatus` is diagnostics-only.** If a future caller needs the real exit code on early-exit, switch the poll loop to take `&mut self` and use `try_wait` directly; the change is local.
- **Deliberate debt:** harness spawns a full Core subprocess; in-process Core variant for fast unit tests deferred to V1.5 per scope. `Handle::exited()` synthesises an `ExitStatus` via `kill(pid, 0)` rather than reaping a real exit code — diagnostics-only path, intentional (see Drift). No `TODO`/`FIXME`/`todo!()` markers in new code.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still uses `tools/smoke-client/` (not the harness) per the orchestrator brief; the smoke gate's scope and runtime are untouched. Re-ran `scripts/smoke.sh` after this task — green.
