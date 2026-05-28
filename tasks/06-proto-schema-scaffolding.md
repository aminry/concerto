# Task 06 — Proto Schema Scaffolding

| Field | Value |
|---|---|
| Phase | 1 |
| Size | small (≤4h) |
| Depends on | 01, 02, 04, 05 |
| Touches subsystem(s) | 10 (Local API) |
| Smoke gate | unchanged |

## Goal
Set up the `crates/proto` crate to compile `.proto` files into Rust types and gRPC server/client stubs via `tonic-build`. After this task, every later proto-touching task simply drops a `.proto` file into `crates/proto/proto/concerto/v1/` and gets generated Rust code on the next build. No actual proto messages or services are added in this task — that's Task 07.

## Inputs to read before starting
- `design/10_Local_API_Protocol.md` §3.1 (one service per domain), §3.6 (code generation pipeline), §4.1 (proto file layout under `crates/proto/proto/concerto/v1/`).
- `design/00_Architecture_Overview.md` §6.5 (gRPC + Tonic locked).
- `tasks/05-error-and-logging-baseline.md` → "Handoff Notes".

## Scope — in
- Add `prost`, `prost-types`, `tonic`, `tonic-build` (dev-dep) to `crates/proto/Cargo.toml`. Pin to a recent stable (e.g., `tonic = "0.12"`).
- Create `crates/proto/build.rs` that:
  - Walks `crates/proto/proto/**/*.proto`.
  - Compiles them via `tonic_build::configure()` with `.build_server(true).build_client(true)`.
  - Outputs to `OUT_DIR`; emits `cargo:rerun-if-changed=proto/`.
  - Adds `serde::Serialize, serde::Deserialize` derives on every message via the `type_attribute("." , "#[derive(serde::Serialize, serde::Deserialize)]")` configurator. (We do not use these in gRPC traffic but they are useful for snapshots and audit logging.)
- Create `crates/proto/src/lib.rs` that re-exports the generated modules:
  ```rust
  pub mod concerto {
      pub mod v1 {
          tonic::include_proto!("concerto.v1");
      }
  }
  pub use concerto::v1 as v1;
  ```
- Add a placeholder `crates/proto/proto/concerto/v1/_placeholder.proto` containing only `syntax = "proto3"; package concerto.v1;` so the build has at least one input. Will be removed in Task 07 when real protos arrive.
- Add `prost-build`/`protoc` setup notes to `crates/proto/README.md` (one paragraph: requires `protoc` on PATH; CI installs it).
- Add `protoc` install step to `.github/workflows/ci.yml` jobs that build the workspace (use `arduino/setup-protoc@v3`).
- Run `cargo build -p concerto-proto`; commit the build artifacts only inasmuch as they are needed for `tonic::include_proto!` (which reads from `OUT_DIR`, so nothing to commit beyond source).

## Scope — out
- No actual proto messages — Task 07 adds the first ones.
- No TypeScript / Swift / Kotlin client generation in V0.1.
- No reflection endpoint (V1.0).

## Public interface this task locks
- File layout: `crates/proto/proto/concerto/v1/<file>.proto`. Package always `concerto.v1`.
- Rust import path: `concerto_proto::v1::<file_basename>::*` for generated messages and `concerto_proto::v1::<file_basename>::<service>_server` / `_client` for services.
- Generated types carry `serde::Serialize` and `serde::Deserialize`.

## Implementation notes
- Use `protoc` from the system PATH; do not bundle a binary. Document the version baseline (`protoc --version >= 25`) in `crates/proto/README.md`.
- Configure `tonic_build` to use `compile_well_known_types(true)` so `google.protobuf.Timestamp`, `Empty`, `Struct` map to `prost-types` types automatically.
- Set `out_dir(env::var("OUT_DIR")?)` and let cargo handle it.
- Ensure the build.rs is deterministic: same `.proto` inputs → byte-identical generated code (this depends on tonic-build/prost-build version).
- Don't commit anything in `target/`. The placeholder `_placeholder.proto` ensures `tonic::include_proto!` doesn't fail when proto/ would otherwise be empty.

## Verification
1. `cargo build -p concerto-proto` → succeeds.
2. `cargo check --workspace` → clean.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. `cargo test -p concerto-proto` → passes (no tests yet, exits 0).
5. `cargo deny check` → still clean.
6. On a clean checkout, deleting `target/` and rebuilding produces the same Rust API surface (`cargo expand -p concerto-proto` is reproducible).
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → may or may not show changes; commit any new state of `docs/interfaces/proto.md` (placeholder content for now).

## Definition of Done
- [ ] Verification commands pass.
- [ ] `protoc` install step added to CI; CI build is green.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] No files outside Outputs modified.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/proto/Cargo.toml` (modified)
- `crates/proto/build.rs` (new)
- `crates/proto/src/lib.rs` (modified)
- `crates/proto/proto/concerto/v1/_placeholder.proto` (new)
- `crates/proto/README.md` (new)
- `.github/workflows/ci.yml` (modified — `protoc` install)
- `docs/interfaces/proto.md` (regenerated, possibly unchanged)

## Commit message
```
phase-1: proto crate scaffolding (tonic-build)

Sets up crates/proto to compile .proto files into Rust gRPC types
via tonic-build per design/10 §3.6. Placeholder proto until Task 07.
CI installs protoc.

Refs: tasks/06-proto-schema-scaffolding.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** —
- **Smoke-gate state:** unchanged.
