# Task 13 — gRPC Server over Unix Domain Socket

| Field | Value |
|---|---|
| Phase | 1 |
| Size | small (≤4h) |
| Depends on | 07, 11, 12 |
| Touches subsystem(s) | 01 (Runtime), 10 (Local API) |
| Smoke gate | unchanged |

## Goal
Stand up the Tonic gRPC server bound to `~/.concerto/core.sock`, hosting the `Runtime` service from Task 07. After this task, a gRPC client (in-process test, or `grpcurl` over `unix:///tmp/...`) can call `Runtime.GetServerCapabilities` and `Runtime.GetStatus` and get real responses backed by the supervisor's actual state.

## Inputs to read before starting
- `design/10_Local_API_Protocol.md` §3.4 (auth: UDS peer-UID is implicit admin — V0.1 doesn't enforce; V1.0 does), §6.3 (UDS via `tokio::net::UnixListener`), §5.1 (Runtime service shape — implemented here).
- `design/01_Core_Daemon_Runtime.md` §5.1 (RuntimeAdmin RPCs from Runtime POV).
- `tasks/12-supervision-tree.md` → "Handoff Notes".

## Scope — in
- Add `tonic`, `tower`, `hyper-util`, `tokio-stream` to `crates/core/Cargo.toml`.
- Implement `crates/core/src/api_server.rs`:
  - `ApiServerActor` implementing the `Actor` trait from Task 12.
  - The actor binds to `<config_dir>/core.sock` (on Windows: named pipe — V0.1 macOS-only so we accept the gap and document it).
  - It registers the generated `RuntimeServer<RuntimeHandler>` from `concerto_proto`.
- Implement `RuntimeHandler` in `crates/core/src/handlers/runtime.rs`:
  - `GetServerCapabilities` returns:
    - `server_version` = `env!("CARGO_PKG_VERSION")`.
    - `schema_version` = `"concerto.v1"`.
    - `optional_services` = `[]` (V0.1 ships exactly what's in the proto, no optional gates).
    - `limits` = `ResourceLimits { max_concurrent_streams: 256, max_payload_bytes: 16 * 1024 * 1024 }`.
    - `transport_kind` = `TRANSPORT_KIND_UDS`.
    - `core_host_os` = result of `std::env::consts::OS`.
    - `core_hostname` = result of `hostname::get()` (add `hostname = "0.4"` dep).
  - `GetStatus` returns:
    - `version` = same as above.
    - `started_at` = Runtime's started-at timestamp.
    - `uptime_seconds` = elapsed since start.
- Wire the `RuntimeHandler` to read live state from the `RootSupervisor` (so future status fields like actor list have a path).
- On socket bind:
  - Remove an existing stale socket file (left over from prior runs) before binding.
  - Set permissions to `0o600` (owner-only).
  - On clean shutdown, remove the socket file (best-effort).
- Convert internal errors to gRPC `Status` via a `From` impl: `concerto_error::Error → tonic::Status` mapping wire codes to status details. Use `tonic::Status::with_details` with a serialized `ConcertoError` proto.
- Update `crates/core/src/main.rs`: after `RootSupervisor` is ready, spawn the `ApiServerActor`.

## Scope — out
- No auth middleware in V0.1 (UDS implicit admin — anyone on the box who can write to the socket is trusted). Auth middleware lands when Iroh arrives.
- No Iroh transport.
- No Connect-Web bridge.
- No `Streams` service (later task).
- No interceptors / instrumentation middleware beyond `tracing::instrument`.
- Windows named-pipe path is documented but not built (macOS-only in V0.1 per `design/00 §6.8`).

## Public interface this task locks
- Socket path: `<config_dir>/core.sock` (default `~/.concerto/core.sock`); permissions `0600`.
- Rust: `crates/core/src/handlers/runtime.rs` — `RuntimeHandler` (the `tonic::server::Runtime` impl).
- gRPC client connection string for callers: `unix://<absolute-path-to-core.sock>`.

## Implementation notes
- Tonic's UDS server: build the service with `tonic::transport::Server::builder().add_service(RuntimeServer::new(handler))`, then `serve_with_incoming(UnixListenerStream::new(listener))`. `UnixListenerStream` is in `tokio-stream::wrappers`.
- `tonic` requires `tower::Service<http::Request<_>>`-compatible plumbing; the UDS variant uses `hyper::server::conn::http2`.
- On socket cleanup: wrap the listener creation in a small helper that:
  ```rust
  if path.exists() {
      // Verify it's a socket; remove if so. Don't remove arbitrary files.
      if path.metadata()?.file_type().is_socket() {
          std::fs::remove_file(&path)?;
      }
  }
  ```
- The `From<concerto_error::Error> for tonic::Status` impl is the canonical conversion. Map wire codes to gRPC `Code::*` per a small table; default to `Code::Internal` for unmapped cases.
- For `started_at` propagation: store it on the `RootSupervisor` or `Runtime` struct, expose via an `Arc`-able snapshot the handler reads.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core api_server` → integration tests pass (see below).
3. Integration test: spawn `concerto-core` in a tempdir; create a Tonic client over `unix://<sock>`; call `GetServerCapabilities`; assert version + transport_kind == `UDS`.
4. Integration test: stale socket file in place before start → bind succeeds (socket replaced).
5. Manual: `cargo run --bin concerto-core &` then:
   ```
   grpcurl -plaintext -unix ~/.concerto/core.sock list
   grpcurl -plaintext -unix ~/.concerto/core.sock concerto.v1.Runtime/GetServerCapabilities
   ```
   (grpcurl requires reflection — note in Handoff Notes that V0.1 doesn't ship reflection; alternative: write a tiny Rust client example.)
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → no unintended drift.
7. `cargo clippy --workspace -- -D warnings` → clean.

## Definition of Done
- [x] Verification commands pass.
- [x] gRPC client successfully calls `GetServerCapabilities` over UDS.
- [x] Socket cleanup verified (no stale `core.sock` after clean shutdown).
- [x] Socket permissions verified at `0600`.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green (smoke gate's first check arrives in Task 15).
- [x] Single commit created.

## Outputs
- `crates/core/Cargo.toml` (modified — tonic, tower, hostname, tokio-stream)
- `crates/core/src/api_server.rs` (new)
- `crates/core/src/handlers/mod.rs` (new)
- `crates/core/src/handlers/runtime.rs` (new)
- `crates/core/src/error_map.rs` (new — `From<Error> for tonic::Status`)
- `crates/core/src/lib.rs` (modified — module declarations)
- `crates/core/src/main.rs` (modified — spawn ApiServerActor)
- `crates/core/tests/grpc_runtime.rs` (new)
- `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-1: gRPC server over UDS

Binds Tonic to ~/.concerto/core.sock (perms 0600), hosts the Runtime
service from Task 07 backed by RootSupervisor state. Error mapping
from concerto_error::Error to tonic::Status via wire codes.

Refs: tasks/13-grpc-uds-server.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **`From<concerto_error::Error> for tonic::Status` replaced by free function `error_to_status`.** The orphan rule forbids that `From` impl outside the crates that own one of the two types. `crates/core/src/error_map.rs` exposes `pub fn error_to_status(Error) -> tonic::Status` instead — handlers call `.map_err(error_to_status)?` to bridge. The mapping table and `ConcertoError` proto details payload behavior are unchanged. Pre-authorized in the orchestrator brief.
  - **`Runtime::started_at()` added** as `pub fn started_at(&self) -> Arc<SystemTime>`. The task scope said "store it on the `RootSupervisor` or `Runtime`"; I picked `Runtime` because the supervisor is consumed first during shutdown and the started-at value is part of the runtime's identity. Stored as `Arc<SystemTime>` so the gRPC handler clones cheaply once at construction. Outputs list updated to include `crates/core/src/runtime.rs` (already in Task 11 outputs; the new method is additive).
  - **`RootSupervisor::view()` + `SupervisorView` type added** in `crates/core/src/supervisor.rs`. `SupervisorView::list()` returns sorted `Vec<ActorStatusSummary>` from a cloneable handle backed by `Arc<StdRwLock<Vec<ActorViewEntry>>>` that `spawn` populates. V0.1's `GetStatus` does NOT return the actor list yet — but the wiring is there so future tasks extending `RuntimeStatus` can read live state without surgery on the handler's construction path. Outputs list grew to include `crates/core/src/supervisor.rs` (Task 12's file; the additions are pure extension).
  - **Socket-file cleanup is a `Drop` guard, not an explicit unlink at the end of `run`.** The supervisor wrapper races `stop.cancelled()` against `run_fut` in a `select!`; on shutdown the `run_fut` is *dropped*, so cleanup code at the bottom of `run` would never execute. `SocketCleanupGuard` on the stack removes the socket regardless of which arm wins. Verified via integration test `get_capabilities_returns_uds_transport` which asserts the file is gone after `runtime.stop().await`.
  - **`tower` is pinned at version 0.5 in the workspace** but tonic 0.12 transitively pulls in `tower 0.4.13` for its own service plumbing. Both versions co-exist; cargo-deny is clean. The integration test uses the 0.5 path (`tower::service_fn`) via the explicit dep.
  - **No `grpcurl` smoke step.** `grpcurl` is not installed in this environment AND tonic reflection is intentionally not built in V0.1 (the task's manual verification §5 notes this). The integration test in `crates/core/tests/grpc_runtime.rs` exercises the same surface with a real in-process Tonic client over `unix://<sock>`.
  - **`crates/core/Cargo.toml` gained `tokio` feature `net`** (in both `[dependencies]` and `[dev-dependencies]`) — required for `tokio::net::UnixListener` / `UnixStream`. Additive only.
  - **Workspace deps added**: `tower = "0.5"`, `hyper-util = "0.1"`, `tokio-stream = "0.1"`, `hostname = "0.4"`. All MIT/Apache-2.0; cargo-deny clean.
  - **`prost-types = "0.13"` added to `crates/core` deps** (matches `concerto-proto`'s existing pin). Needed for `Timestamp` construction in `handlers/runtime.rs` and for the `ConcertoError` details payload in `error_map.rs`.
  - **`docs/interfaces/rust-api.md` NOT updated.** Same pattern as Tasks 11 and 12: the interface generator only scrapes `crates/<crate>/src/api.rs`, and `concerto-core` still does not follow that convention. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` is clean. Locked types (`RuntimeHandler`, `ApiServerActor`, `ApiServerConfig`, `error_to_status`, `SupervisorView`) live in their respective files; a future task that adds `crates/core/src/api.rs` re-exports could surface them.
- **Open questions for next task:**
  - **Task 14 (Tauri Desktop)** connects via `unix://<config_dir>/core.sock`. The client must use `Endpoint::connect_with_connector` with a `tower::service_fn` that returns `hyper_util::rt::TokioIo<UnixStream>` — the pattern is in `crates/core/tests/grpc_runtime.rs::connect_client`. The URI fed to `Endpoint::try_from` is a placeholder (`http://[::1]:50051`); the connector overrides it.
  - **Task 15 (smoke gate v1)** can rely on: socket appears within 5s of `concerto-core` boot at `<CONCERTO_CONFIG_DIR>/core.sock`, has perms `0600`, responds to `Runtime/GetServerCapabilities` with `transport_kind == TRANSPORT_KIND_UDS`, removes the socket on clean shutdown. The smoke script will need to use a Rust binary or `grpcurl --plaintext` (if added). No reflection is shipped, so `grpcurl list` will not work; the manual command in the task file is documented as an aspirational example.
  - **`RuntimeStatus.actor_list` extension path:** the handler already holds a `SupervisorView`; adding a repeated field for the actor table in `runtime.proto` and reading `self.supervisor_view.list()` is the natural follow-on. Field number 4 onward is free.
  - **`error_to_status` is currently only used by the wire-error contract tests.** No handler returns `Err(concerto_error::Error)` yet — `GetServerCapabilities` and `GetStatus` are infallible in V0.1. Future RPCs that touch persistence or the supervisor's stop-actor path will use `.map_err(error_to_status)?`.
- **Deliberate debt:** no auth middleware (V0.1 trusts UDS peer); no reflection endpoint (so `grpcurl list` doesn't work — clients must know the schema out-of-band); Windows path returns `Error::Internal` (V1.0 named-pipe port); no in-process retry on actor restart of the socket bind path — if the underlying tonic transport errors, the supervisor's restart loop will rebuild the listener (covered by the existing Task 12 backoff). No `TODO`/`FIXME`/`todo!()` markers in new code.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still prints "Smoke gate: PASSED (no checks active yet — Phase 0)". Task 15 is where the first real `GetServerCapabilities` round-trip lands; everything that check needs (socket path, perms, transport_kind) is wired and tested in-process by `crates/core/tests/grpc_runtime.rs`.
