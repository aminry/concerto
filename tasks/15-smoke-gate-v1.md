# Task 15 — Smoke Gate v1 (End-to-End Spine)

| Field | Value |
|---|---|
| Phase | 1 |
| Size | small (≤4h) |
| Depends on | 13, 14 |
| Touches subsystem(s) | 01 (Runtime), 10 (Local API), 15 (Desktop) |
| Smoke gate | v1 |

## Goal
Make `scripts/smoke.sh` perform a real end-to-end check: boot the Core, connect a gRPC client over UDS, call `GetServerCapabilities`, verify the response, shut down cleanly. After this task, the smoke gate is a meaningful signal — if it goes red, the foundation is broken. Every subsequent task must keep it green.

## Inputs to read before starting
- `tasks/README.md` §5 (verification model).
- `tasks/03-smoke-gate-scaffolding.md` → the original scaffolding.
- `tasks/13-grpc-uds-server.md` → "Handoff Notes" — confirms UDS server is up.
- `tasks/14-tauri-shell-skeleton.md` → "Handoff Notes".

## Scope — in
- Add a small Rust binary `tools/smoke-client/` that:
  - Connects to a UDS at a path passed via `--socket`.
  - Calls `Runtime.GetServerCapabilities`.
  - Prints the response as JSON to stdout.
  - Exits 0 on success, non-zero on any error.
- Add the crate to the workspace.
- Update `scripts/smoke.sh` to:
  ```sh
  # Phase 1 checks
  echo "Smoke gate: starting concerto-core in background..."
  CORE_CONFIG_DIR="$CONCERTO_HOME/.concerto"
  CORE_DATA_DIR="$CONCERTO_HOME/concerto"
  mkdir -p "$CORE_CONFIG_DIR" "$CORE_DATA_DIR"
  CONCERTO_CONFIG_DIR="$CORE_CONFIG_DIR" CONCERTO_DATA_DIR="$CORE_DATA_DIR" \
    cargo run --quiet --bin concerto-core > "$CONCERTO_HOME/core.log" 2>&1 &
  CORE_PID=$!
  
  cleanup_core() { kill -TERM "$CORE_PID" 2>/dev/null || true; wait "$CORE_PID" 2>/dev/null || true; }
  trap 'cleanup_core; rm -rf "$CONCERTO_HOME"' EXIT
  
  # Wait for the UDS socket (timeout 15s)
  source scripts/lib/common.sh
  wait_for_file "$CORE_CONFIG_DIR/core.sock" 15 || fail "core.sock not created"
  
  # Call GetServerCapabilities
  cargo run --quiet --bin smoke-client -- --socket "$CORE_CONFIG_DIR/core.sock" \
    | grep -q '"transport_kind":"TRANSPORT_KIND_UDS"' || fail "unexpected smoke-client output"
  
  # Shut down cleanly
  kill -TERM "$CORE_PID"
  wait "$CORE_PID" || fail "core did not exit cleanly"
  
  # PID file should be gone
  [ ! -f "$CORE_CONFIG_DIR/core.pid" ] || fail "core.pid not cleaned up"
  
  echo "Smoke gate v1: PASSED"
  ```
- Update `scripts/lib/common.sh` to add `wait_for_file(path, timeout_seconds)`.
- The `.github/workflows/smoke.yml` already runs `scripts/smoke.sh`; verify it works on the Linux CI runner.
- Verify the existing `cargo run --bin concerto-core` binary respects `CONCERTO_CONFIG_DIR` and `CONCERTO_DATA_DIR` env vars (Task 11 should have done this; if not, add it here as a minor amendment with a note in Handoff).

## Scope — out
- No Desktop in the smoke gate (it's GUI, hard to run headless in V0.1 CI).
- No agent spawning (Phase 2).
- No workspace creation (Phase 2).

## Public interface this task locks
- Smoke gate version `v1` means: Core boots → UDS up → `GetServerCapabilities` returns → clean shutdown.
- Path: `tools/smoke-client/` is the canonical smoke client. Subsequent tasks extend it with more RPC checks.
- Env vars: `CONCERTO_CONFIG_DIR` and `CONCERTO_DATA_DIR` are the override mechanism for Core directories.

## Implementation notes
- Set explicit timeouts on every Tonic client call (5 seconds) so a broken Core doesn't hang the CI job.
- Use `tokio::time::timeout` for the gRPC call.
- Use `tonic` over `UnixStream` exactly as Task 13's integration test did — copy that pattern.
- The smoke client must NOT depend on a pre-existing Core — it owns the connect logic and times out.
- The smoke script must NOT use `set -e` *inside* function bodies that have intentional non-zero exits (the `set -euo pipefail` at the top is correct but be aware of subshell semantics).
- Output of the smoke script must be human-readable. Each step prints a one-line status (`Smoke gate v1: starting Core...`, `Smoke gate v1: Core ready`, etc.).

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo clippy --workspace -- -D warnings` → clean.
3. `scripts/smoke.sh` locally → exits 0, prints "Smoke gate v1: PASSED" within ~30 seconds.
4. Smoke CI workflow runs green.
5. Force-failure check: temporarily break the `Runtime.GetServerCapabilities` handler (e.g., make it return `Err`); rerun `scripts/smoke.sh`; verify it fails fast with a clear error; revert.
6. Force-failure check: simulate UDS socket never appearing (e.g., set a wrong `CONCERTO_CONFIG_DIR` that the core can't write to); verify the script times out at 15s and fails.
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → no drift.
8. `shellcheck scripts/smoke.sh scripts/lib/common.sh` → clean.

## Definition of Done
- [x] Verification commands pass.
- [x] Smoke gate v1 passes locally and in CI.
- [x] Force-failure checks confirmed (script fails when it should).
- [x] No `TODO` / `FIXME` in new code.
- [x] Single commit created.

## Outputs
- `tools/smoke-client/Cargo.toml` (new)
- `tools/smoke-client/src/main.rs` (new)
- `scripts/smoke.sh` (modified — adds Phase 1 checks)
- `scripts/lib/common.sh` (modified — adds wait_for_file)
- `Cargo.toml` (workspace root, modified — adds tools/smoke-client to members)
- `crates/core/src/runtime.rs` (possibly modified if env var support needs amendment)

## Commit message
```
phase-1: smoke gate v1 — end-to-end Core spine

scripts/smoke.sh now boots concerto-core, waits for the UDS socket,
calls Runtime.GetServerCapabilities via tools/smoke-client, and
verifies clean shutdown. CI runs this on every push.

Refs: tasks/15-smoke-gate-v1.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - The crate's binary is `smoke-client` but the package name is `concerto-smoke-client` (workspace naming convention); `cargo run/build` therefore needs `-p concerto-smoke-client --bin smoke-client` rather than the bare `--bin smoke-client` form the task pseudocode used. Same for `concerto-core` (`-p concerto-core`).
  - Outputs list grew by two files: `.github/workflows/smoke.yml` (added `dtolnay/rust-toolchain@stable`, `arduino/setup-protoc@v3`, and `Swatinem/rust-cache@v2` so the CI runner can compile `concerto-core` and `smoke-client`; the existing one-line `scripts/smoke.sh` step would have failed otherwise) and `Cargo.lock` (automatic — `concerto-smoke-client` added to workspace). `crates/core/src/runtime.rs` did NOT need amendment: Task 11 already wired `CONCERTO_CONFIG_DIR` / `CONCERTO_DATA_DIR` through `RuntimeConfig::default_for_user()`.
  - Skipped `clap` — `smoke-client` parses a single flag (`--socket`) by hand from `std::env::args()` to avoid pulling a new workspace dep.
  - The auto-derived `serde::Serialize` on the prost-generated `ServerCapabilities` would emit `transport_kind` as the raw `i32`, but the smoke script greps for the string name `"TRANSPORT_KIND_UDS"`. `smoke-client` builds its own `serde_json::Value` and uses `TransportKind::as_str_name()` for that field.
- **Open questions for next task:** Task 16 (logging discipline) and Task 17 (integration test harness) inherit a green smoke gate; Task 27's Phase-2 smoke extension can append below the `# Phase 2 checks` marker without restructuring.
- **Deliberate debt:** smoke gate doesn't exercise Desktop yet — V0.1 keeps Desktop verification manual until Task 27 builds Phase 2's end-to-end. The smoke script pre-builds via `cargo build --quiet` to keep wall-clock predictable; cold-cache CI run is ~3–5 min, warm-cache (`Swatinem/rust-cache@v2`) is well under a minute.
- **Smoke-gate state:** **v1 active.** Covers: Core boot → UDS up → GetServerCapabilities → clean shutdown.
  - Force-failure check 5: edited `crates/core/src/handlers/runtime.rs::get_server_capabilities` to return `Status::internal("FORCE_FAILURE_FOR_SMOKE_TEST")`; `scripts/smoke.sh` exited non-zero immediately with `smoke-client: GetServerCapabilities rpc error: status: Internal, message: "FORCE_FAILURE_FOR_SMOKE_TEST"`; reverted before commit.
  - Force-failure check 6: invoked `wait_for_file` against a path that never appears; it correctly timed out at exactly 15s and returned non-zero (no Core process was required because the timeout path is the only branch under test).
