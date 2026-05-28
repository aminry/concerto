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
- [ ] Verification commands pass.
- [ ] gRPC client successfully calls `GetServerCapabilities` over UDS.
- [ ] Socket cleanup verified (no stale `core.sock` after clean shutdown).
- [ ] Socket permissions verified at `0600`.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green (smoke gate's first check arrives in Task 15).
- [ ] Single commit created.

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
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** no auth middleware (V0.1 trusts UDS peer); no reflection endpoint.
- **Smoke-gate state:** infrastructure exists; first smoke check arrives in Task 15.
