# Task 07 — First Proto Messages and Runtime Service

| Field | Value |
|---|---|
| Phase | 1 |
| Size | small (≤4h) |
| Depends on | 06 |
| Touches subsystem(s) | 01 (Runtime), 10 (Local API) |
| Smoke gate | unchanged |

## Goal
Define the smallest set of `.proto` messages and services needed for the V0.1 smoke gate v1: `Runtime.GetServerCapabilities`, and the entity messages `Workspace`, `Workarea`, `Session` (no service methods on these yet — just the data shapes that Task 09's DB schema and later RPCs will share). This task is the canonical source of the V0.1 wire schema; subsequent tasks ADD service methods, never modify these messages.

## Inputs to read before starting
- `design/10_Local_API_Protocol.md` §4.2 (`ServerCapabilities`, `TransportKind`, `ResourceLimits`), §5.1 (full service catalog — extract V0.1 subset only).
- `design/09_Persistence.md` §4.1 (entity tables — extract column-to-field mapping).
- `tasks/06-proto-schema-scaffolding.md` → "Handoff Notes".

## Scope — in
Add the following proto files under `crates/proto/proto/concerto/v1/`:

**`common.proto`:**
```proto
syntax = "proto3";
package concerto.v1;
import "google/protobuf/timestamp.proto";

message ConcertoError {
  string code = 1;
  string message = 2;
  google.protobuf.Struct fields = 3;
  string transaction_id = 4;
}

message Identifier { string value = 1; }   // newtype for UUIDv7 IDs

enum PermissionMode {
  PERMISSION_MODE_UNSPECIFIED = 0;
  PERMISSION_MODE_STRICT = 1;
  PERMISSION_MODE_NORMAL = 2;
  PERMISSION_MODE_AUTO = 3;
  PERMISSION_MODE_YOLO = 4;
}
```

**`runtime.proto`:**
```proto
syntax = "proto3";
package concerto.v1;
import "google/protobuf/empty.proto";
import "google/protobuf/timestamp.proto";

enum TransportKind {
  TRANSPORT_KIND_UNSPECIFIED = 0;
  TRANSPORT_KIND_UDS = 1;
  TRANSPORT_KIND_IROH = 2;
  TRANSPORT_KIND_WSS_BRIDGE = 3;
}

message ResourceLimits {
  uint32 max_concurrent_streams = 1;
  uint64 max_payload_bytes = 2;
}

message ServerCapabilities {
  string server_version = 1;
  string schema_version = 2;
  repeated string optional_services = 3;
  ResourceLimits limits = 4;
  TransportKind transport_kind = 5;
  string core_host_os = 6;
  string core_hostname = 7;
}

message RuntimeStatus {
  string version = 1;
  google.protobuf.Timestamp started_at = 2;
  uint64 uptime_seconds = 3;
}

service Runtime {
  rpc GetServerCapabilities(google.protobuf.Empty) returns (ServerCapabilities);
  rpc GetStatus(google.protobuf.Empty) returns (RuntimeStatus);
}
```

**`workspaces.proto`:**
```proto
syntax = "proto3";
package concerto.v1;
import "google/protobuf/timestamp.proto";
import "concerto/v1/common.proto";

message Workspace {
  string id = 1;
  string project_id = 2;
  string name = 3;
  string slug = 4;
  optional string description = 5;
  optional PermissionMode permission_mode = 6;
  google.protobuf.Timestamp created_at = 7;
  optional google.protobuf.Timestamp archived_at = 8;
}
```

**`workareas.proto`:**
```proto
syntax = "proto3";
package concerto.v1;
import "google/protobuf/timestamp.proto";
import "concerto/v1/common.proto";

message Workarea {
  string id = 1;
  string workspace_id = 2;
  string composer_name = 3;
  string branch_name = 4;
  string worktree_root = 5;
  string status = 6;                   // created | active | running | awaiting | paused | archived | crashed
  optional PermissionMode permission_mode = 7;
  google.protobuf.Timestamp created_at = 8;
  optional google.protobuf.Timestamp last_activity_at = 9;
  optional google.protobuf.Timestamp archived_at = 10;
}
```

**`sessions.proto`:**
```proto
syntax = "proto3";
package concerto.v1;
import "google/protobuf/timestamp.proto";
import "concerto/v1/common.proto";

message Session {
  string id = 1;
  string workarea_id = 2;
  string chat_id = 3;
  string agent_kind = 4;               // claude | codex | gemini
  optional string agent_version = 5;
  optional string model = 6;
  string status = 7;                   // starting | running | awaiting | finished | crashed
  PermissionMode permission_mode = 8;
  google.protobuf.Timestamp started_at = 9;
  optional google.protobuf.Timestamp ended_at = 10;
}
```

- Delete `_placeholder.proto` from Task 06.
- Update `crates/proto/src/lib.rs` to re-export specific modules: `pub mod runtime { tonic::include_proto!("concerto.v1.runtime"); }` etc., OR a single `tonic::include_proto!("concerto.v1")` for all (whichever generates correctly).
- Update `docs/interfaces/proto.md` by running `./scripts/regen-interfaces.sh`.

## Scope — out
- No service implementations (Task 13 wires up the Runtime service handler).
- No service methods for Workspaces, Workareas, Sessions — those messages exist as data types only; service methods land in Tasks 19, 20, 23.
- No `Streams` service (Task 24 or later).
- No `Devices` / `Files` / `Maestro` / `Notifications` / `Vcs` services.
- No `Skills` / `Suggestions` / `Schedules` services (Phase 3).

## Public interface this task locks
- Proto package `concerto.v1` is canonical.
- Message field numbers in this task are FROZEN. Adding new fields is fine (higher numbers); renumbering or repurposing is forbidden.
- The `Runtime` service's two methods (`GetServerCapabilities`, `GetStatus`) are the V0.1 minimum surface.

## Implementation notes
- Use `Timestamp` from `google.protobuf` rather than int64 — it's the schema convention in `design/10`.
- `optional` on proto3 scalar fields requires `prost` ≥ `0.12`; verify.
- Don't add `Streams` service yet — its inclusion needs reconnect-offset thinking that V1.0 covers.
- The `agent_kind` and `status` fields use string rather than enum for V0.1 simplicity. We can promote to enums in a later revision task if needed.
- The "wire codes" defined in Task 05 (`crates/error/src/error.rs`) should map onto `ConcertoError.code` strings; the mapping happens in the gRPC server middleware (Task 13).

## Verification
1. `cargo build -p concerto-proto` → succeeds.
2. `cargo check --workspace` → clean.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. `cargo expand -p concerto-proto | grep -c 'pub struct ServerCapabilities'` → exactly 1.
5. `cargo expand -p concerto-proto | grep -c 'pub struct Workspace'` → exactly 1.
6. `cargo expand -p concerto-proto | grep -c 'fn get_server_capabilities'` → at least 1 (server + client + trait).
7. `./scripts/regen-interfaces.sh && git diff docs/interfaces/proto.md` → shows the four new services/messages; commit the result.
8. `cargo deny check` → clean.

## Definition of Done
- [x] Verification commands pass.
- [x] `docs/interfaces/proto.md` regenerated and committed.
- [x] No `TODO` / `FIXME` in proto files.
- [x] `_placeholder.proto` deleted.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `crates/proto/proto/concerto/v1/common.proto` (new)
- `crates/proto/proto/concerto/v1/runtime.proto` (new)
- `crates/proto/proto/concerto/v1/workspaces.proto` (new)
- `crates/proto/proto/concerto/v1/workareas.proto` (new)
- `crates/proto/proto/concerto/v1/sessions.proto` (new)
- `crates/proto/proto/concerto/v1/_placeholder.proto` (deleted)
- `crates/proto/src/lib.rs` (modified)
- `crates/proto/build.rs` (modified — see Handoff Notes / Drift)
- `crates/proto/Cargo.toml` (modified — see Handoff Notes / Drift)
- `Cargo.lock` (auto-updated)
- `scripts/regen-interfaces.sh` (modified — proto-scan path fix, see Handoff Notes / Drift)
- `docs/interfaces/proto.md` (regenerated)

## Commit message
```
phase-1: first proto messages — Runtime service + entity shapes

Adds common.proto, runtime.proto, workspaces.proto, workareas.proto,
sessions.proto. Field numbers locked. Runtime service exposes
GetServerCapabilities + GetStatus — the minimum smoke gate needs.

Refs: tasks/07-first-proto-messages.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **`scripts/regen-interfaces.sh` patched** to scan `crates/proto/proto/` instead of `$ROOT/proto/`. The bug was flagged in Task 06's Handoff Notes; without the fix, Verification step #7 (and the silent-drift backstop for proto in general) would be useless. Single-line change in `gen_proto()`. Added `scripts/regen-interfaces.sh` to Outputs.
  - **`crates/proto/build.rs` modified** beyond what the task said it would touch:
    1. Dropped `compile_well_known_types(true)` from the tonic-build config (was set by Task 06). With it on, tonic-build emits references to a sibling `super::super::google::protobuf::*` module that doesn't exist in our setup. With it off (the default), tonic-build auto-maps `.google.protobuf.*` to `::prost_types::*`, which is what we want — downstream crates use `prost_types::Timestamp` etc. directly. This is the cleaner of the two paths and matches design/10's intent.
    2. Added `field_attribute(...)` calls that inject `#[serde(with = "crate::serde_compat::option_timestamp")]` on every `Option<prost_types::Timestamp>` field, and `#[serde(with = "crate::serde_compat::option_struct")]` on `ConcertoError.fields`. Reason: Task 06's blanket `type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")` doesn't compile once Timestamp / Struct fields exist, because `prost_types::Timestamp` / `prost_types::Struct` themselves don't implement serde and there's no `serde` feature on `prost-types` to enable. The `with` shims do JSON-friendly serialization without a new dependency on `prost-wkt-types`. Added a comment in `build.rs` listing every Timestamp field; future proto tasks must extend that list when they add Timestamp-typed fields.
  - **`crates/proto/src/lib.rs` adds a `serde_compat` module** with two submodules — `option_timestamp` (encodes `Option<Timestamp>` as `Option<(i64, i32)>` for seconds + nanos) and `option_struct` (recursively converts `Option<Struct>` to/from a `serde_json::Value::Object`). The doc-comment-style usage hints are inline. Only the build.rs-injected `#[serde(with = ...)]` attributes reference these modules; no other crate uses them directly yet.
  - **`crates/proto/Cargo.toml` adds `serde_json = { workspace = true }`** for the `option_struct` shim. Workspace already had `serde_json = "1"` so no new dep at the workspace level.
- **Open questions for next task:**
  - The `optional` proto3 keyword expands the V1.0 wire shape: when an unset `optional PermissionMode` is sent, the receiver sees `None`, distinct from `PERMISSION_MODE_UNSPECIFIED`. Persistence schemas (Task 09) should treat `None` and `STRICT` (the typical implied default) differently — `None` means "inherit from parent" per design/03 §3.2, not "strict".
  - The proto `Workspace`, `Workarea`, `Session` messages list `string status` rather than enums. Task 09's SQLite migration will need a corresponding text-with-CHECK-constraint, or a separate enum table. Either is fine, but the column type must match the validators the gRPC server middleware (Task 13) will write — the canonical list of allowed values is in the proto comments.
  - The `agent_kind = "claude"|"codex"|"gemini"` list in `sessions.proto` includes gemini for forward compatibility, but per the V0.1 scope in `tasks/README.md §2` only claude + codex are wired in V0.1. The agent-supervisor work (Tasks 21–22, 33–37) does not need to handle gemini; future polish task will.
  - If Task 08+ (persist + migration) reads `docs/interfaces/proto.md`, note the file now reflects the real protos. If you see `_No `.proto` files yet._` you're on a stale checkout.
- **Deliberate debt:** — (no `TODO`/`FIXME` left in any new file)
- **Smoke-gate state:** unchanged — still v1 (i.e., still "no checks active yet — Phase 0"). The gRPC server skeleton (Task 13) and the Desktop UDS round-trip (Task 14) are what flips smoke to its first real assertion in Task 15.
