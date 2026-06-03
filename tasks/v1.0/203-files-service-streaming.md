# Task 203 — `Files` Service: Chunked Streaming Upload/Download, BLAKE2b Checksum, Allow-List Enforced

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 201 |
| Touches subsystem(s) | 10 (Client API Protocol), 12 (Security & Identity) |
| Smoke gate | new:files-transfer |

## Goal
Add the new **`Files`** gRPC service so a split-host client (Desktop in remote mode, mobile) can transfer files to/from a Core it does not share a filesystem with. Today there is no `files.proto` and no `Files` handler — co-located clients read/write the same disk, but the V1.0 split-host configuration (`design/10 §2`) needs an explicit transfer path. This task creates `crates/proto/proto/concerto/v1/files.proto` (faithfully reproducing the `design/10 §5.1` surface) and the `crates/core/src/handlers/files.rs` handler implementing `Upload` (client→Core streaming, chunked ≤256 KiB, finalize with a BLAKE2b checksum the Core verifies), `Download` (Core→client streaming with optional offset/length), `Stat`, and `List`. Every write and read is **scoped to a `(workarea, repository)` (or the workarea's `.context/`) and enforced against the filesystem allow-list + hard deny-list** (`design/12 §3.5`) *before* any byte touches disk — reusing the existing `crates/core/src/security/path_policy.rs` machinery (Task 41), not a new policy. After this task the Core can safely receive and serve files for remote clients; the real over-Iroh transfer is exercised split-host by Task 220 (Tier 3).

## Inputs to read before starting
- `design/10_Local_API_Protocol.md` §5.1 — the **full `Files` proto is spelled out** and must be reproduced faithfully: `service Files { rpc Upload(stream UploadChunk) returns (UploadResult); rpc Download(DownloadRequest) returns (stream DownloadChunk); rpc Stat(StatRequest) returns (StatResult); rpc List(ListFilesRequest) returns (ListFilesResponse); }`; `UploadChunk { oneof body { UploadHeader header = 1; bytes data = 2; UploadFinalize finalize = 3; } }`; `UploadHeader { string workarea_id = 1; optional string repository_id = 2; string relative_path = 3; uint64 expected_size = 4; string content_type = 5; }`; `UploadFinalize { bytes blake2b = 1; }`; `UploadResult { string stored_path = 1; uint64 size = 2; }`; `DownloadRequest { string workarea_id = 1; optional string repository_id = 2; string relative_path = 3; optional uint64 offset = 4; optional uint64 length = 5; }`; `DownloadChunk { bytes data = 1; }`. The doc comment on the service: *"Scoped to a (workarea, repo) or a workarea's `.context/`; Core enforces the permission-mode + allow-list checks."* (`repository_id` unset ⇒ the `.context/` root.) `Stat`/`List` request+result messages are NOT spelled out in the doc — design them minimally (see Implementation notes) and FREEZE them.
- `design/10_Local_API_Protocol.md` §2 (split-host file transfer is V1.0; co-located clients don't need it — same FS), §3.4 (the two auth paths land in the **same** handlers — `Files` is not transport-special; auth/peer-uid gating is Task 210's job, not this task), §4.1 (proto-file layout: `files.proto` joins the `concerto/v1/` set).
- `design/12_Security_Identity.md` §3.5 (filesystem allow-list: **always allowed** = worktree path + `.context/` + the project's declared writable paths; approval required elsewhere; **hard deny** = `~/.ssh`, `~/.aws`, etc. — never bypassed), §3.7 (the deny-list is the only floor that never bypasses). `Files` writes/reads MUST resolve the target path under the `(workarea, repo)` scope and reject anything that lands `Outside`/`Denied`.
- `crates/core/src/security/path_policy.rs` — the **live** allow/deny machinery (Task 41) to **reuse**: `AllowList::for_workarea(...)`, `pub async fn for_workarea_from_db(persistence, workarea_id, home) -> Result<(AllowList, DenyList)>`, `classify(path, &allow, &deny) -> PathDecision` returning `Allowed | Outside | Denied`. For `Files`, **only `Allowed` proceeds** — `Outside` and `Denied` both reject (there is no interactive approval ceremony on a streamed RPC; the strict policy floor is the contract). Note the symlink-escape canonicalization the module already does.
- `crates/core/src/api_server.rs` — `run_uds` is where services are conditionally registered (`builder.add_service(...)`). Add the `Files` service here behind the handles it needs (an `Arc<Persistence>` to resolve the workarea→worktree_root→repo paths via `for_workarea_from_db`, plus a `home` dir). Mirror the existing `if let Some(...)` registration pattern.
- `crates/core/src/handlers/mod.rs` — register the new `pub mod files;` (gate `#[cfg(unix)]` only if it ends up depending on unix-only handles; the handler itself should be cross-platform — see Implementation notes).
- `tasks/v1.0/205-identity-crypto-primitives.md` → "Implementation notes" + "Outputs" — Task 205 introduces the `blake2` crate to `[workspace.dependencies]` for `device_id`. **Decision to resolve in-task:** if 205 has merged first, reuse the workspace `blake2` pin for the BLAKE2b checksum here; if this task runs first, **declare** the `blake2` workspace pin yourself (MIT/Apache-2.0, clears `deny.toml`) and note in Handoff that 205 reuses it. Either way `blake2` ends up a single workspace pin.
- `tasks/v1.0/201-capability-negotiation.md` → "Handoff Notes" — 201 is the dependency (current Streams/handler surface); the `ConnTransport` seam is not consumed here, but confirms the handler-registration surface you extend.

## Scope — in
- New proto `crates/proto/proto/concerto/v1/files.proto` — `package concerto.v1;`, the **exact** `§5.1` surface above. Add minimal, FROZEN `Stat`/`List` messages (see Implementation notes). `crates/proto/build.rs` auto-globs `proto/**/*.proto`, so the file is compiled with **no build.rs edit** — UNLESS a field needs the explicit `serde` `with` mapping (the build.rs lists those per `MessageName.field_name`, ~line 45); the `bytes` fields here (`blake2b`, `data`) are plain `Vec<u8>`/`bytes` and should not need one — verify against how `streams.proto`'s `bytes` fields are handled and only touch build.rs if the build complains.
- New handler `crates/core/src/handlers/files.rs` implementing the generated `Files` trait:
  - **Upload**: first frame MUST be `UploadHeader`; resolve `(workarea_id, repository_id, relative_path)` → an absolute target path; `classify` it against `for_workarea_from_db(...)` → reject non-`Allowed` with `PERMISSION_DENIED` + a typed `files.*` code **before** opening the file; reject `relative_path` containing `..`/absolute components (path-escape) with `INVALID_ARGUMENT`; stream `data` frames (each ≤256 KiB — reject larger) to a temp file while updating a running BLAKE2b hasher; on `UploadFinalize`, compare the computed digest to the supplied `blake2b` (reject mismatch with `DATA_LOSS`/typed code) and the byte count to `expected_size`; atomically rename temp→target; return `UploadResult { stored_path, size }`.
  - **Download**: resolve + `classify` (must be `Allowed`); honor optional `offset`/`length` (seek + bounded read); stream `DownloadChunk { data }` in ≤256 KiB frames.
  - **Stat**: resolve + `classify`; return existence/size/is_dir/content-type.
  - **List**: resolve a directory under scope + `classify`; return entries (name, size, is_dir) — non-recursive.
- Reuse `path_policy` for ALL four RPCs; the `(workarea, repo)` scope root is the allow-list root for that workarea (`worktree_root` for a repo path, the repo's `local_path` when `repository_id` is set, `.context/` when unset).
- Tests (Tier 1, co-located): round-trip Upload→Download of a multi-chunk file with matching checksum; a tampered checksum on finalize → reject; a `relative_path` with `..` → reject; an Upload targeting a deny-list/outside path → `PERMISSION_DENIED`; `expected_size` mismatch → reject; a `data` frame >256 KiB → reject; `Download` with offset/length returns the right slice; `Stat`/`List` on an in-scope path.

## Scope — out
- The real **split-host transfer over Iroh** — exercised end-to-end by Task 220 (Tier 3); this task proves chunking/checksum/scoping co-located in CI.
- Auth / peer-uid gating / device-cert validation on the `Files` RPCs — Task 210 (auth middleware) applies uniformly across all services; do not build per-RPC auth here.
- Interactive approval for `Outside` writes — `Files` enforces the strict floor (`Allowed` only); there is no per-write approval ceremony over a streamed RPC. (The agent-host tool path has that ceremony; `Files` does not.)
- Resumable/ranged **uploads**, compression, content-type sniffing beyond echoing the header — V1.5+.
- Desktop drag-drop → `Files.Upload` wiring (Task 602) and any client.
- A `transport.proto` (decision D1: NO `transport.proto`).

## Public interface this task locks
- The `files.proto` surface: `service Files { Upload / Download / Stat / List }`, `UploadChunk` oneof (`header=1`/`data=2`/`finalize=3`), `UploadHeader` (fields 1–5 exactly), `UploadFinalize.blake2b`, `UploadResult` (`stored_path`/`size`), `DownloadRequest` (fields 1–5 exactly), `DownloadChunk.data`, plus the new `StatRequest`/`StatResult`/`ListFilesRequest`/`ListFilesResponse` messages — FROZEN field numbers.
- The checksum algorithm: **BLAKE2b** over the full uploaded byte stream; the digest width (pick BLAKE2b-256 unless the design implies otherwise — FREEZE it in the proto comment).
- The scope contract: `(workarea_id, repository_id?, relative_path)` resolves under the workarea allow-list; **only `PathDecision::Allowed` proceeds**; `repository_id` unset ⇒ `.context/` root.

## Implementation notes
- **`Stat`/`List` messages aren't in the design** — keep them minimal and additive-friendly: `StatRequest { string workarea_id = 1; optional string repository_id = 2; string relative_path = 3; }`, `StatResult { bool exists = 1; uint64 size = 2; bool is_dir = 3; string content_type = 4; }`; `ListFilesRequest { string workarea_id = 1; optional string repository_id = 2; string relative_path = 3; }`, `ListFilesResponse { repeated FileEntry entries = 1; }`, `FileEntry { string name = 1; uint64 size = 2; bool is_dir = 3; }`. Freeze these numbers; mirror the doc's optional-`repository_id` convention.
- **Path resolution is the security-critical step.** Build the absolute target as `scope_root.join(relative_path)`, then `classify` the **canonical** result. Reject `relative_path` that is absolute or contains `..` components up front (cheap defense), but rely on `classify`'s canonicalization (which already defends symlink-escape) as the authoritative check. Never open/create the file before `classify` returns `Allowed`.
- **Upload atomicity**: write to a temp file in the *same scope directory* (so the rename is same-filesystem and atomic), hash as you go, verify checksum + size, then `rename`. On any error, remove the temp file. Reuse `tokio::fs`; keep the hasher fed incrementally (don't buffer the whole file in memory — that defeats chunking and blows the 16 MiB payload budget).
- **BLAKE2b**: use the `blake2` crate's `Blake2b` with a 256-bit output (`Blake2b<U32>` or the digest-API equivalent). Coordinate the workspace pin with Task 205 per the input note above.
- **Cross-platform**: the handler must build on the Windows CI lane (Task 113). Use `std::path`/`tokio::fs` only — no `std::os::unix`. `path_policy` is already cross-platform. If `api_server.rs` registration forces a `#[cfg(unix)]` handle, gate only that registration line, not the handler module.
- **Handler thinness** (`§6.1`): the handler resolves scope + enforces policy + does the byte plumbing; there is no business sub-system to delegate to (file IO *is* the operation). That's fine — `path_policy` is the "sub-system" it leans on.
- Regen: new proto ⇒ `./scripts/regen-interfaces.sh` updates `docs/interfaces/proto.md`; commit it.

## Verification
Tier 1.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core files` → round-trip, checksum-mismatch, path-escape, deny/outside-reject, size-mismatch, oversize-chunk, offset/length download, stat/list tests pass.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (the `blake2` pin clears MIT/Apache-2.0; confirm — and only if this task introduces it).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`proto.md` gains the `Files` service).
7. `scripts/smoke.sh` → add a new `files-transfer` capability (`scripts/smoke.d/<NN>-files-transfer.sh` defining `check_files_transfer`, appended to `scripts/smoke.manifest` after `streams-subscribe`): over the live UDS Core, upload a small file into a workarea scope, download it back, assert byte-identical + checksum, and assert an out-of-scope path is rejected. Exits 0.

## Definition of Done
- [x] `crates/proto/proto/concerto/v1/files.proto` reproduces the §5.1 surface faithfully + frozen `Stat`/`List` messages; added to the proto build list
- [x] `crates/core/src/handlers/files.rs` implements Upload/Download/Stat/List with chunking (≤256 KiB) + incremental BLAKE2b + atomic rename
- [x] Every RPC resolves `(workarea, repo)` scope and enforces `path_policy` (Allowed-only); `..`/absolute paths rejected
- [x] `Files` service registered in `api_server.rs`; `pub mod files` in `handlers/mod.rs`
- [x] `blake2` is a single workspace pin (introduced here + noted in Handoff); `cargo deny check` green
- [x] Tests cover round-trip, checksum/size mismatch, path-escape, deny/outside reject, oversize chunk, ranged download
- [x] Builds on the Windows CI lane (no `std::os::unix` in the handler)
- [x] Verification commands pass; new `files-transfer` smoke green; interfaces regenerated
- [x] Single commit with the message below

## Outputs
- `crates/proto/proto/concerto/v1/files.proto` (new — auto-globbed by `build.rs`; edit `build.rs` only if a field needs an explicit serde `with` mapping)
- `crates/core/src/handlers/files.rs` (new)
- `crates/core/src/handlers/mod.rs` (modified — `pub mod files`)
- `crates/core/src/api_server.rs` (modified — register `FilesServer`)
- `crates/core/Cargo.toml` / root `Cargo.toml` (modified — `blake2` dep, if introduced here)
- `crates/core/tests/files_service.rs` (new)
- `scripts/smoke.d/45-files-transfer.sh` (new) + `scripts/smoke.manifest` (modified)
- `tools/smoke-client/src/cmd/files_transfer_probe.rs` (new) + `tools/smoke-client/src/cmd/mod.rs` / `tools/smoke-client/src/main.rs` / `tools/smoke-client/Cargo.toml` (modified) — the `files-transfer-probe` subcommand the smoke check drives (added beyond the original Outputs list, mirroring Task 202's `streams_replay_probe`; see Handoff)
- `docs/interfaces/proto.md` (regenerated)

## Commit message
```
phase-2: Files service — chunked streaming upload/download

New Files gRPC service (Upload/Download/Stat/List) for split-host file
transfer: ≤256 KiB chunks, incremental BLAKE2b checksum verified on
finalize, atomic rename. Every path is resolved under the (workarea,
repo) scope and enforced against the path_policy allow/deny floor
(Allowed-only). Real over-Iroh transfer is exercised by Task 220.

Refs: tasks/v1.0/203-files-service-streaming.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan:** Two additions beyond the literal `Outputs` list, both required to satisfy the `new:files-transfer` smoke gate and flagged here per the rules of engagement. (1) `tools/smoke-client/src/cmd/files_transfer_probe.rs` (+ registration in `cmd/mod.rs`, `main.rs`, and a `blake2` dep in `tools/smoke-client/Cargo.toml`): the smoke gate drives RPCs exclusively through `smoke-client`, exactly as Task 202 added `streams_replay_probe.rs` for its smoke capability — this is the established pattern, not new surface. (2) One extra line in `crates/core/src/handlers/files.rs` beyond pure byte-plumbing: `resolve_allowed` canonicalizes the DB-stored scope root before joining `relative_path`. Without it, on hosts where the data root sits behind a symlink (macOS `/var`→`/private/var`), a target whose leaf doesn't yet exist falls back to lexical cleaning and keeps the un-canonical prefix, so `classify` wrongly returns `Outside`. Canonicalization is on the existing scope dir only; `classify` still does the authoritative symlink-resolving check on the full target (the deny-list symlink-escape test confirms this).
- **Open questions for next task:** None blocking. Note for Task 210 (auth middleware): the `Files` handler does NOT do peer-uid / device-cert gating — it assumes 210 applies auth uniformly across all services (per Scope — out). Note for Task 220 (split-host loopback smoke): the over-Iroh `Files` transfer is unexercised here; 220 owns that Tier-3 reality. `content_type` is echoed in `UploadHeader` but `Stat`/`List` return `""` for files (no sniffing — V1.5+ per Scope — out); a future content-type task can populate it without a proto change.
- **Deliberate debt:** None. No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code.
- **Smoke-gate state:** Added `scripts/smoke.d/45-files-transfer.sh` (`check_files_transfer`) and registered `files-transfer` in `scripts/smoke.manifest` immediately after `streams-subscribe` (so `WA_ID` + the `.context/` root exist). The probe uploads a ~450 KiB multi-chunk file into the workarea's `.context/` (repository_id unset, always allow-listed), downloads it back asserting byte-identical + BLAKE2b-256 match, stats it, and asserts an out-of-scope `../escape.txt` upload is rejected. `scripts/smoke.sh` (full + `--only files-transfer`) is GREEN; `shellcheck -x` on the new check is clean. **blake2 pin ownership:** introduced here as the single `[workspace.dependencies] blake2 = "0.10"` pin (BLAKE2b-256, `Blake2b<U32>`); Task 205 should REUSE this pin for its `device_id` hash rather than re-declare it. **Digest width FROZEN:** BLAKE2b-256 (32-byte output), documented in the `files.proto` `UploadFinalize.blake2b` comment.
