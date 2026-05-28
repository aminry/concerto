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
- [x] Verification commands pass.
- [x] `protoc` install step added to CI; CI build is green.
- [x] No `TODO` / `FIXME` in new code.
- [x] No files outside Outputs modified. *(Workspace `Cargo.toml` added to Outputs in Handoff Notes drift.)*
- [x] Smoke gate still green.
- [x] Single commit created.

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
- **Drift from plan:**
  - **Workspace `Cargo.toml` modified** (not listed in Outputs as written). Bumped `tonic` from `"0.11"` to `"0.12"` per the task's explicit pin and the operator's pre-flight note. Also bumped `prost` from `"0.12"` to `"0.13"` because tonic 0.12 is version-coupled to prost 0.13 — they share the same major series and tonic 0.12's generated code emits `prost = 0.13` paths. Both stay `default-features = false` at the workspace level; `crates/proto/Cargo.toml` re-enables `tonic` features `["transport", "codegen", "prost"]` locally. The existing consumer (`crates/error`, which pulls `tonic::Status` via `tonic.workspace = true`) compiles unchanged — the `Status` type is stable across the 0.11 → 0.12 bump. Adding `Cargo.toml` to Outputs in retrospect: yes.
  - **Placeholder proto contains one trivial empty message `_Placeholder`** instead of the spec's literal "only `syntax = ...; package ...;`". prost-build / tonic-build skip emitting a Rust file for a package that contains zero messages and zero services, which makes `tonic::include_proto!("concerto.v1")` fail at compile time ("couldn't read .../out/concerto.v1.rs: No such file or directory"). The trivial message is the carrier so the generated file actually lands on disk. It is not referenced anywhere and Task 07 removes the whole `_placeholder.proto` file when real messages arrive. A comment in the proto file documents this. (Generated symbol is `concerto_proto::v1::Placeholder` — prost strips the leading underscore in Rust identifiers. Confirmed not used by any other crate.)
  - **`build.rs` uses `std::io::Error::other(...)`**, not `Error::new(ErrorKind::Other, ...)`. Clippy's `io_other_error` lint (new in 1.95) fires on `Error::new(ErrorKind::Other, _)` under `-D warnings`. The `::other` shortcut is the modern idiomatic form; behavior is identical. Mentioned because the task didn't specify the error-construction form.
  - **`prost-types = "0.13"`** added as a direct dep on `crates/proto/Cargo.toml` rather than going through the workspace table. Workspace currently doesn't list `prost-types`; rather than touch the workspace dep table a second time, I pinned the version locally. If a future task adds another crate that needs `prost-types`, hoist it then.
  - **`tonic-build` is `default-features = false` with `["prost", "transport"]`** rather than the default. The defaults include `cleanup-markdown` (pulls `pulldown-cmark` — large) and `prettyplease` (formats generated code — needs nightly-only formatting on some builds). Neither is needed for our pipeline. The pin is `tonic-build = "0.12"` matching `tonic`.
- **Open questions for next task:**
  - **`scripts/regen-interfaces.sh` looks at `$ROOT/proto`, not `crates/proto/proto/`.** The proto files this task created live at `crates/proto/proto/concerto/v1/*.proto`, so `docs/interfaces/proto.md` still says "_No `.proto` files yet._" after regeneration. This is a Task 04 parser limitation (it was written before the proto layout was locked) and the task spec for Task 06 anticipated it ("may or may not show changes"). Task 07 (which adds the first real messages) will hit the same blind spot — the interface drift backstop won't catch proto changes until the script is patched. Recommend the next polish/revision task fix the search path in `gen_proto()`: change `"$ROOT/proto"` to `"$ROOT/crates/proto/proto"`. Single-line change; deterministic; doesn't affect any other generator.
  - **`tonic` is now `0.12`, `prost` is now `0.13`.** Any future crate adding `tonic.workspace = true` will need feature flags appropriate to tonic 0.12's split (`["transport", "codegen", "prost"]` is the typical server/client set; `["codegen", "prost"]` alone works for crates that only need the generated stubs without `transport::Server`). The 0.11 → 0.12 hyper bump is internal: tonic 0.12 uses `hyper = 1.x` whereas 0.11 used `hyper = 0.14`. No other crate in the workspace currently uses hyper directly, so no spillover.
  - **`cargo deny check` emits warnings (not errors)** about license entries that aren't currently used and about duplicated transitive deps (two getrandom versions, two wit-bindgen versions, both pulled by prost-build → tempfile). `deny.toml` is configured to fail on advisories/bans/licenses/sources only; duplicates are advisory. If a future task wants the duplicate noise gone, dedup will require pinning at a deeper level (probably not worth it for two transitively-different getrandom releases).
  - **First proto build downloads ~50 transitive deps** (axum, hyper-1, h2, prost-build, prost-derive, tonic-build, tonic). Cold workspace build is now ~3 min longer than after Task 05. Subsequent builds are incremental and unaffected.
- **Deliberate debt:**
  - The `_Placeholder` empty message in `_placeholder.proto` is the only TODO-shaped object I'm carrying. Removed by Task 07 when the first real messages land. Not a `TODO` literal; just a temporary file. Documented inline in the proto file's header comment.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still exits 0 with "Smoke gate: PASSED (no checks active yet — Phase 0)". Task 15 will add the first real assertion.
