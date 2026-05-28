# `concerto-test-harness`

Shared integration-test harness for Concerto. Spawns a real `concerto-core`
subprocess in a tempdir and returns a connected gRPC client. Every later
Phase 2/3 task that adds an integration test uses this crate; without
that convention integration tests will diverge.

Locked in Task 17 (`tasks/17-integration-test-harness.md`).

## Scope

- **Use for crate-level integration tests** (`tests/*.rs`), not for unit
  tests. Spawning a real subprocess costs ~1–3 seconds and is wasted
  effort for anything that can run in-process.
- **Tests share no state.** Each `CoreUnderTest::spawn()` produces a
  fully isolated Core: fresh tempdir, fresh `CONCERTO_CONFIG_DIR` and
  `CONCERTO_DATA_DIR`, fresh socket, fresh database. Concurrent harness
  instances are isolation-safe.
- **Use the multi-thread tokio flavour.** The harness spawns Tonic
  channels and a tokio process; `#[tokio::test(flavor = "multi_thread")]`
  is the right annotation for tests that exercise it.

## Example

```rust,no_run
use concerto_test_harness::CoreUnderTest;

#[tokio::test(flavor = "multi_thread")]
async fn round_trip() {
    let core = CoreUnderTest::spawn().await.expect("spawn");
    let mut client = core.runtime_client().await.expect("client");
    let caps = client
        .get_server_capabilities(())
        .await
        .expect("rpc")
        .into_inner();
    assert_eq!(caps.schema_version, "concerto.v1");
    core.shutdown().await.expect("shutdown");
}
```

## Cargo wiring

Callers depend on the harness as a `[dev-dependencies]` path entry:

```toml
[dev-dependencies]
concerto-test-harness = { path = "../test-harness" }
```

Production crates **must not** depend on this crate. It is a workspace
member so `cargo test --workspace` builds its self-tests, but nothing in
the production graph links against it.

## API surface

- `CoreUnderTest::spawn() -> Self` — boots `concerto-core` in a tempdir,
  waits up to 15 s for `<config>/core.sock`.
- `CoreUnderTest::runtime_client() -> RuntimeClient<Channel>` —
  Tonic client over UDS. Each call dials a fresh channel.
- `CoreUnderTest::db() -> SqlitePool` — read-only pool to the Core's
  database. WAL mode allows concurrent readers.
- `CoreUnderTest::shutdown(self)` — SIGTERM, wait 10 s, SIGKILL fallback.
- `Drop` — last-resort `start_kill()` if the test forgot `shutdown`.

### What about `workspaces_client()` / `workareas_client()` / `sessions_client()`?

The task spec sketched these accessors for forward-compatibility. They
are **not** shipped in Phase 1: the `Workspace`, `Workarea`, and `Session`
*messages* exist (Task 07) but the gRPC *services* that expose them
arrive in Phase 2 (Tasks 19, 20, 23). The Phase 1 harness exposes
`runtime_client()` only; Phase 2 tasks add the other accessors as the
services they front come online.

## How `spawn` finds the binary

`assert_cmd::cargo::cargo_bin("concerto-core")` locates the workspace
binary. This relies on cargo having built `concerto-core` before the
test runs — `cargo test --workspace` does this automatically.

For ad-hoc usage, `cargo build -p concerto-core` first.

## Shutdown semantics

`shutdown()` sends SIGTERM, waits up to 10 s, then escalates to SIGKILL.
A `tracing::warn!` fires if shutdown takes longer than 5 s — set
`RUST_LOG=concerto_test_harness=warn` (or higher) in the test process
to see it.
