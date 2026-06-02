# Task 209 — `Devices` Service: `ListDevices` / `RevokeDevice` / `GetCoreInfo` + Revoke-Mid-Stream

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 207 |
| Touches subsystem(s) | 12 (Security & Identity), 09 (Persistence — devices read/revoke) |
| Smoke gate | unchanged |

## Goal
Complete the **device-management** half of the `Devices` service that Task 207 opened. Task 207 created `devices.proto` with the two pairing RPCs (`StartPairing`/`CompletePairing`) and INSERTs a `devices` row on successful pairing. This task **extends the same proto** with `ListDevices`, `RevokeDevice`, and `GetCoreInfo`, and wires the **read + revoke** side of the already-existing `devices` table (`crates/persist/migrations/0001_initial_schema.sql` lines 242–253 — **no migration is added**). `RevokeDevice` is the security-critical path: it sets `revoked_at` immediately, inserts the `device_id` into the **shared revoked set** that Task 206's `LocalCoreIssuer::validate` reads (so future connects fail at auth), and **actively closes any open streams from the revoked device** via the transport handle so a stolen device is severed in **< 1 s** — well inside the PRD §22.4 < 60 s time-to-revoke. After this task an operator can list paired devices, revoke one from any admin client, and a revoked device is both rejected on reconnect and torn off mid-stream; `GetCoreInfo` returns the `core_pubkey` clients carry from pairing.

## Inputs to read before starting
- `design/12_Security_Identity.md` §5.2 — the gRPC surface for the management RPCs you ADD: `ListDevices(google.protobuf.Empty) returns (ListDevicesResponse)`, `RevokeDevice(RevokeDeviceRequest) returns (google.protobuf.Empty)`, `GetCoreInfo(google.protobuf.Empty) returns (CoreInfo)` (the comment notes `core_pubkey for client carry`). Reproduce the names faithfully.
- `design/12_Security_Identity.md` §3.11 — **revocation**: setting `devices.revoked_at` is immediate; the audit-log entry is the source of truth; the Core **actively closes any open streams from the revoked device**; future connects fail at auth. A user revokes from any other paired device or the tray; recovery is the user's other paired devices.
- `design/12_Security_Identity.md` §7.3 — the **revocation sequence** to implement against (reproduce the order): `revoke_device(id)` → DB `set revoked_at` → `revoked_cache.insert(id)` → transport `terminate sessions for device id` → connection closed → a later reconnect's `validate cert` returns `REVOKED` → `UNAUTHENTICATED`; the Desktop sees the `device.revoked` event.
- `design/12_Security_Identity.md` §5.3 — emitted events: `device.revoked` (broadcast, on revocation persisted) and `device.paired` (broadcast, on pairing completed — 207 owns emitting the latter; this task emits `device.revoked`). Also `device.seen` (low-rate, first daily contact) — **out of scope** here (note it; `last_seen_at` population is a later/auth-path concern).
- `design/12_Security_Identity.md` §3.7 — the `DeviceRevoked` audit event (and `AuditKind` taxonomy) emitted on revoke; §10 — the **integration test bar** "Revoke mid-stream — connection closed within 1s" (latency-budgeted), which is this task's Tier-1 unit-test target via an in-process stream stub.
- `design/10_Local_API_Protocol.md` §8 — the revoked-device error mapping the *auth* path will use (`PERMISSION_DENIED` + `auth.revoked`) — Task 210 owns that mapping; this task only persists the revoked state + populates the revoked set the validator reads.
- `tasks/v1.0/207-pairing-noise-xx.md` (full task + its **Handoff Notes**) — the **FROZEN `devices.proto` pairing surface** + assigned field numbers (you append with NEW numbers, never reorder), the `crates/core/src/security/pairing.rs` coordinator + where the `devices` row is INSERTed (mirror its column conventions: `id` = device-id fingerprint/hex, `public_key` = raw Ed25519 bytes, `paired_at` = unix seconds), how the `Devices` handler reaches persistence/security state, and how the handler is registered in `handlers/mod.rs` + `api_server.rs`.
- `tasks/v1.0/206-device-cert-issuer.md` (+ **Handoff Notes**) — the **shared revoked-set handle type** (`Arc<RwLock<HashSet<[u8;32]>>>` or arc-swap — 206 froze the constructor so this task wires the SAME handle into the revoke path that the issuer reads), the `LocalCoreIssuer::new(core_priv, core_pub, revoked_set_handle)` wiring site, and `core_public_key()` for `GetCoreInfo`.
- `crates/persist/migrations/0001_initial_schema.sql` lines 242–253 — the **`devices` table already exists** (`id`, `name`, `public_key`, `paired_at`, `last_seen_at`, `revoked_at`, `push_token`, `push_platform`) + `idx_devices_active ON devices(revoked_at) WHERE revoked_at IS NULL`. **Do not add a migration.** `ListDevices` reads it; `RevokeDevice` UPDATEs `revoked_at`.
- `crates/core/src/handlers/devices.rs` (created by 207) + `crates/core/src/handlers/sessions.rs` — the `#[async_trait]` thin-handler pattern, how a handler reaches persistence/security state, and `crate::error_map` for Status mapping.
- `crates/core/src/api_server.rs` — how the `Devices` service is constructed + added to the builder (207 wired the pairing coordinator in; you thread the revoked-set handle + the transport handle through the same constructor). Note the `#[cfg(unix)]` gating pattern other services use.
- `tasks/v1.0/README.md` §5.3 (`rust` command set) + §6 row 209.

## Scope — in
- **Extend `devices.proto`** (`crates/proto/proto/concerto/v1/devices.proto`, created by 207) — APPEND only, NEW field/RPC numbers, never reorder 207's pairing surface:
  - `rpc ListDevices(google.protobuf.Empty) returns (ListDevicesResponse);`
  - `rpc RevokeDevice(RevokeDeviceRequest) returns (google.protobuf.Empty);`
  - `rpc GetCoreInfo(google.protobuf.Empty) returns (CoreInfo);`
  - Messages: `DeviceEntry { string device_id; string name; bytes public_key; int64 paired_at; int64 last_seen_at; int64 revoked_at; }` (the nullable `last_seen_at`/`revoked_at` columns map to `0`/unset — document the sentinel), `ListDevicesResponse { repeated DeviceEntry devices; }`, `RevokeDeviceRequest { string device_id; }`, `CoreInfo { bytes core_pubkey; string core_version; string core_host_os; string core_hostname; }` (reuse the host/version values `runtime.rs` already computes via `std::env::consts::OS` / `hostname::get()` — see Task 201).
  - **Reserve a proto comment** noting `UpdateDevicePushToken` is **DEFERRED to P5** (per Decision D1; the `push_token`/`push_platform` columns already exist but no RPC writes them in V1.0).
- **`RevokeDevice` coordinator logic** (extend `crates/core/src/security/pairing.rs` or a sibling `crates/core/src/security/devices.rs` — match where 207 put the coordinator): a `revoke_device(id, by)` that performs the `§7.3` sequence **in order**: (1) `UPDATE devices SET revoked_at = <now> WHERE id = ?`; (2) `revoked_set.insert(device_id)` into the **same shared handle** 206's issuer reads; (3) call the transport handle's `close_sessions_for_device(device_id)` to sever open streams; (4) emit the `DeviceRevoked` audit event + broadcast the `device.revoked` stream event. Idempotent (revoking an already-revoked device is a no-op success, or `NOT_FOUND` for an unknown id — pick + document).
- **`ListDevices` / `GetCoreInfo`** read paths: `ListDevices` SELECTs all rows (active + revoked) into `DeviceEntry`s; `GetCoreInfo` returns `core_public_key()` (from the 206 identity wiring) + the host/version fields.
- **`Devices` gRPC handler** (extend `crates/core/src/handlers/devices.rs`, `#[async_trait]`): implement the three new RPCs by delegating to the coordinator; map failures via `crate::error_map`. The service is already registered (207); thread the new state (revoked-set handle + transport handle) through its constructor.
- **The transport seam.** The active stream-close calls `TransportHandle::close_sessions_for_device(device_id)` (the handle defined by **Task 217**, atop the Iroh transport of **Task 212**). Until 212/217 exist, define a **narrow local trait** (e.g. `SessionCloser { fn close_sessions_for_device(&self, id: [u8;32]); }`) that the coordinator depends on, wire the real `TransportHandle` to it in 217, and inject a **stub** in tests. Name this seam explicitly in Implementation notes + Handoff so 217 connects it.
- **Tier-1 tests**: `ListDevices` returns inserted rows (active + revoked distinguishable via `revoked_at`); `RevokeDevice` sets `revoked_at`, inserts into the shared revoked set (assert 206's `validate` now rejects that `device_id`), and **calls the `SessionCloser` stub** — assert the close happens and measure the **revoke→close latency with an in-process stream stub < 1 s** (use an injected clock / instant capture, not a real `sleep`); `RevokeDevice` on unknown/already-revoked id behaves per the documented choice; `GetCoreInfo` returns the wired `core_pubkey`; `DeviceRevoked` audited + `device.revoked` broadcast.

## Scope — out
- The `StartPairing`/`CompletePairing` pairing RPCs + Noise XX + token store + the `devices` row INSERT — **Task 207** (this task only reads/revokes; 207 froze the pairing surface).
- The **real** `TransportHandle::close_sessions_for_device` implementation + the live Iroh stream teardown — **Tasks 212 / 217**. This task defines + tests the seam against a stub; the real cross-device mid-stream teardown is the **Phase-2 Tier-3 checklist** line ("revoke a device and confirm < 60 s stream teardown").
- The **auth-path** rejection of a revoked cert on reconnect (`PERMISSION_DENIED` / `auth.revoked`) — **Task 210** (it calls 206's `validate`, which already reads the revoked set this task populates).
- Populating `last_seen_at` / the `device.seen` event — later (an auth-path concern, not management).
- `UpdateDevicePushToken` + any push-token write — **DEFERRED to P5** (proto comment only).
- Cert auto-renewal / re-pair flows — later.
- The Desktop "Connected Cores" / revoke UI — **Task 219**.

## Public interface this task locks
- **The extended `devices.proto` management surface** — FROZEN: `ListDevices(Empty) → ListDevicesResponse`, `RevokeDevice(RevokeDeviceRequest) → Empty`, `GetCoreInfo(Empty) → CoreInfo`, and the `DeviceEntry` / `ListDevicesResponse` / `RevokeDeviceRequest` / `CoreInfo` message shapes. Field numbers, once assigned, are frozen and append-only; 207's pairing RPCs/messages keep their numbers untouched.
- **`CoreInfo`** field set (`core_pubkey: bytes`, `core_version`, `core_host_os`, `core_hostname`) — FROZEN; clients carry `core_pubkey` from pairing/connect.
- **The `SessionCloser` / `close_sessions_for_device(device_id)` seam name + signature** — the contract Task 217's `TransportHandle` satisfies. FROZEN at the trait level so 217 implements it without renaming.
- **The revocation sequence ordering** (`§7.3`): persist `revoked_at` → insert revoked set → close sessions → audit/broadcast. FROZEN — auth correctness depends on the revoked-set insert happening before any reconnect can race it.

## Implementation notes
- **The revoked set is the live link to 206.** Do **not** create a second set. Task 206 froze `LocalCoreIssuer::new(.., revoked_set_handle)`; this task receives the **same** `Arc<RwLock<HashSet<[u8;32]>>>` (or arc-swap) handle and inserts into it on revoke. The Tier-1 test that proves "validate rejects after revoke" must share one handle between the issuer and the coordinator — that shared-handle wiring is the whole point.
- **Close-before-audit vs audit-before-close.** Follow `§7.3`: the DB write and revoked-set insert come first (they make future connects fail), then the active close, then audit/broadcast. The < 1 s budget is the *close* latency, not the DB write — the in-process stub measures from `revoke_device` entry to the stub's `close_sessions_for_device` being invoked.
- **The transport seam must not block on 212/217.** Define the `SessionCloser` trait in `crates/core` (near the coordinator), inject an `Arc<dyn SessionCloser>`; the production wiring is a one-line construction in 217. In tests, a stub records the closed `device_id`s and a captured `Instant`. Mirror the "wire against the interface, note the seam" pattern Task 207 used for its loopback channel.
- **Nullable columns → proto sentinels.** `last_seen_at` / `revoked_at` are nullable `INTEGER`s; map `NULL → 0` in `DeviceEntry` and document that `revoked_at == 0` means "active". (Or use `optional int64` — pick one and freeze it; the simpler `0`-sentinel matches the integer-seconds convention 207 used for `paired_at`.)
- **`async-trait` for the handler** (workspace convention; see `handlers/sessions.rs:27`). The coordinator's `revoke_device`/`list_devices` are async (they hit persistence); the revoked-set insert + `SessionCloser` call are sync under the lock / behind the trait.
- **`GetCoreInfo` host fields.** Reuse exactly what `runtime.rs` already produces (`std::env::consts::OS`, `hostname::get()`) so there's one source of truth — do not re-derive differently.
- **Cross-platform.** No `std::os::unix`-only types in the coordinator/handler/seam signatures; gate any UDS-specific glue under `#[cfg(unix)]` as `api_server.rs` already does. The `SessionCloser` trait + `[u8;32]` device ids are portable — keep the Windows CI lane green.
- **Determinism.** The < 1 s latency assertion must be hermetic: drive the stub in-process, capture the close `Instant`, assert the delta — never a real timed `sleep`.

## Verification
Tier 1 — the test double is an **in-process `SessionCloser` stub** that records closed `device_id`s + a captured close `Instant`. It proves: the `§7.3` revocation ordering, the shared-revoked-set insert (asserted via 206's `validate` rejecting the id), the `DeviceRevoked` audit + `device.revoked` broadcast, the `ListDevices`/`GetCoreInfo` reads, and the **revoke→close latency < 1 s** against the stub. It does **NOT** cover: a **real** open Iroh stream from a real second device being torn down over the wire — that needs Tasks 212/217's live `TransportHandle` and is the **Phase-2 Tier-3 checklist** line ("revoke a device and confirm < 60 s stream teardown").
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core` (devices/revocation) → `ListDevices`, `RevokeDevice` (revoked_at + shared-set insert + stub close + < 1 s latency), unknown/already-revoked behavior, `GetCoreInfo`, and audit/broadcast tests pass.
4. `cargo test --workspace --no-fail-fast` → all pass (including 206's `validate`-rejects-after-revoke assertion if it lives there).
5. `cargo deny check` → green (no new external deps expected; confirm).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (the extended `Devices` surface appears in the generated proto interface doc; the `devices` columns are unchanged so `schema.md` is untouched — confirm).

## Definition of Done
- [ ] `devices.proto` extended with `ListDevices`/`RevokeDevice`/`GetCoreInfo` + their messages (NEW field numbers, 207's surface untouched); `UpdateDevicePushToken` reserved-DEFERRED-to-P5 comment
- [ ] `RevokeDevice` performs the `§7.3` sequence (persist `revoked_at` → insert shared revoked set → `SessionCloser::close_sessions_for_device` → `DeviceRevoked` audit + `device.revoked` broadcast); idempotency documented
- [ ] `ListDevices` / `GetCoreInfo` read paths implemented; `CoreInfo` returns the wired `core_pubkey` + host/version
- [ ] `SessionCloser` seam defined + injected; real wiring deferred to Task 217 and noted in Handoff
- [ ] `Devices` handler implements the three new RPCs (already registered by 207); state threaded through its constructor
- [ ] Tier-1 tests incl. revoke→close latency < 1 s against the stub + validate-rejects-after-revoke pass; the Tier-3 uncovered part stated in Verification
- [ ] Existing `devices` table wired with NO migration added
- [ ] Verification commands pass; interfaces regenerated + committed
- [ ] No `TODO`/`unimplemented!()`/`todo!()` in new code (deliberate ones in Handoff)
- [ ] Single commit with the message below

## Outputs
- `crates/proto/proto/concerto/v1/devices.proto` (modified — append `ListDevices`/`RevokeDevice`/`GetCoreInfo` + messages)
- `crates/core/src/security/pairing.rs` *(or `crates/core/src/security/devices.rs` — match 207)* (modified/new — `revoke_device`/`list_devices` + the `SessionCloser` trait) + `crates/core/src/security/mod.rs` (modified)
- `crates/core/src/handlers/devices.rs` (modified — three new RPCs)
- `crates/core/src/api_server.rs` (modified — thread the revoked-set + `SessionCloser` handles into the `Devices` constructor)
- `crates/core/tests/device_revocation.rs` (new — Tier-1 revoke/list/core-info + latency tests)
- `docs/interfaces/proto.md` (regenerated)

## Commit message
```
phase-2: Devices service — ListDevices/RevokeDevice/GetCoreInfo + revoke teardown

Extends the Task 207 devices.proto with the management RPCs and wires
the existing devices table (read/revoke, no migration). RevokeDevice
runs the design/12 §7.3 sequence: persist revoked_at, insert the shared
revoked set the 206 issuer reads, close open sessions via a SessionCloser
seam (Task 217's TransportHandle wires the real one), then audit +
broadcast device.revoked. Revoke→close latency unit-tested < 1 s against
an in-process stream stub. UpdateDevicePushToken reserved for P5.

Refs: tasks/v1.0/209-devices-service.md
```

## Handoff Notes (fill in when finishing)
- devices.proto field numbers assigned (append range) / revoked-set handle shared with 206 issuer / `SessionCloser` seam signature for Task 217 to satisfy / revoke idempotency choice / last_seen_at + device.seen still deferred / Open questions / Deliberate debt / Smoke-gate state
