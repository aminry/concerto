# concerto-proto

The Concerto gRPC schema. Single source of truth for every wire message and
service Concerto speaks: `.proto` files under `proto/concerto/v1/` →
`tonic-build` → Rust server stubs + client stubs at build time.

Per `design/10_Local_API_Protocol.md` §3.6 and §4.1.

## Layout

```
crates/proto/
├── build.rs                    # tonic-build invocation
├── Cargo.toml
├── README.md                   # this file
├── proto/
│   └── concerto/
│       └── v1/                 # package concerto.v1 — the v1 schema
│           └── *.proto         # one file per service domain
└── src/
    └── lib.rs                  # re-exports the generated module tree
```

Generated code lands in `OUT_DIR` (cargo-managed) and is pulled in by
`tonic::include_proto!("concerto.v1")` from `src/lib.rs`. Nothing
generated is checked into the repo.

## Conventions

- Every `.proto` file declares `package concerto.v1;`.
- Generated types derive `serde::Serialize` and `serde::Deserialize` in
  addition to the prost-default `prost::Message`. Serde derives are not
  used on the wire — proto encoding handles that — but they make snapshot
  tests and audit-log JSON serialization straightforward.
- Well-known types (`google.protobuf.Timestamp`, `Empty`, `Struct`, etc.)
  map to `prost-types` automatically via
  `tonic_build::configure().compile_well_known_types(true)`.
- The Rust import path mirrors the proto package:
  `concerto_proto::v1::<message-or-service>`.

## Build requirements

`protoc` (the Protocol Buffers compiler) must be on `PATH`. The crate does
not bundle a `protoc` binary; the build script shells out to whatever
version is installed locally.

Minimum supported version: **`protoc >= 25`** (any recent stable; we
develop and CI against 25+).

### Installing locally

```sh
# macOS
brew install protobuf

# Debian/Ubuntu
apt-get install protobuf-compiler

# verify
protoc --version
```

### CI

`.github/workflows/ci.yml` installs `protoc` via `arduino/setup-protoc@v3`
before any `cargo` step that builds the workspace.

## Determinism

Same `.proto` inputs → byte-identical generated Rust for a given
`tonic-build` / `prost-build` pair. We pin those versions in `Cargo.toml`
(`tonic-build = "0.12"`) so the only source of drift is the proto sources
themselves. CI rebuilds from scratch on every PR.

## Status

Task 06 added the scaffolding. The placeholder file
`proto/concerto/v1/_placeholder.proto` exists only so `compile_protos`
has at least one input; Task 07 replaces it with the first real messages
(`Workspace`, `Workarea`, `Session`, `ServerCapabilities`) and the first
service (`ServerService.GetCapabilities`).
