# Task 509 — `ConcertoIroh` native module: a hand-rolled `uniffi` cdylib over `concerto-transport`

| Field | Value |
|---|---|
| Phase | 5 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 212 (transport), 207/208 (identity Noise), 217.5 (boot Iroh seam) |
| Touches subsystem(s) | 16 (Mobile), 11 (Transport) |
| Smoke gate | unchanged |

## Goal
Ship the `ConcertoIroh` React Native (iOS/Android) native module as a **hand-rolled `uniffi` cdylib** over the EXISTING, spike-validated `concerto-transport` stack. This is the **D12 fallback**: `iroh-ffi` is unusable for Concerto — it is git-only (no published `0.98.x`), and pulling it drags in a SECOND, colliding `iroh` with different crypto pins, which would break the validated `iroh = 0.98.2` / `iroh-relay = 0.98.0` trio (Task 212). So instead of binding iroh-ffi, this crate is a thin uniffi facade over the SAME seam `tools/pair-dial` already proves end to end: `connect_channel` (Noise IK + channel-tag-0x01 API channel as a tonic `Channel`), the `0x03` Noise-XX pairing flow, and the client-side `classify_path` NAT classification. 509 stays a **pure byte passthrough** (510 assembles the typed gRPC paths + messages; 511 persists keys + drives pairing).

## The reuse seam (REUSE — do not reinvent)
- `concerto_transport::connect_channel(client, server_addr, local_static, core_noise_pub) -> tonic::transport::Channel` — the openSession core.
- `concerto_transport::{IrohDuplex, write_channel_tag, ChannelTag, ALPN, MAX_MESSAGE_SIZE, classify_path, ConnectionPath}` — the frozen transport surface.
- `tools/pair-dial`'s `pair_over_iroh` — the exact `0x03` Noise-XX pairing flow (0x03 tag, XX over the token, the frozen `device_pubkey(32)||nonce(32)||signature(64)||device_name` request with `signature = sign(token||nonce||device_pubkey)`, 4-byte-BE framing) and the `concerto-device-cert` = STANDARD-base64(signed_cert) auth header.
- `concerto_identity::{KeyPair, NoiseStatic, device_id, generate_seed}` — Ed25519 + Noise primitives.
- `tools/split-host-loopback` + `crates/core::boot` — the in-process Iroh-Core boot for the Tier-2 loopback test.

## Scope — in
- **New crate `crates/concerto-iroh-ffi`** — `[lib] crate-type = ["cdylib", "staticlib", "lib"]` (the FIRST dynamic-lib crate in the workspace). Added to the root `Cargo.toml` `members` but NOT `default-members` (the mobile FFI link toolchain must never gate default `cargo build` / CI). MINIMAL deps mirroring pair-dial (transport/identity/proto + iroh + tonic + prost, NO core/keychain/sqlx); `concerto-core` + `concerto-test-harness` are DEV-deps only.
- **uniffi pinned at `0.28`** (`default-features = false`, proc-macro mode: `setup_scaffolding!()` + `#[uniffi::export]`). 0.28 is the documented MSRV-1.74 line — comfortably under the workspace MSRV of 1.82. 0.29/0.30/0.31 progressively raised the floor; we pin the older compatible line and document why. `default-features = false` drops `cargo-metadata`/`clap`/`camino` (cdylib runtime scaffolding needs only `uniffi_core` + `uniffi_macros`). uniffi is MPL-2.0, already on the `deny.toml` allow-list.
- **The frozen FFI surface (design/16 §3.2 + §4.6):**
  - `generateDeviceKeypair() -> { seed, public_key, device_id }` (Ed25519 via OS randomness; caller persists the seed).
  - `pair(PairingInputs, device_seed) -> SignedDeviceCert bytes` — `pair_over_iroh` ported exactly.
  - `openSession(ConnectBlob, signed_cert) -> handle: u64` — bind endpoint, reconstruct addr, gen per-session Noise static, `connect_channel`, classify path, register an opaque numeric handle in a `Mutex<HashMap>` registry. The cert rides `concerto-device-cert` on every call.
  - `rpcUnary(handle, method, payload) -> bytes` — `Grpc::unary` with an IDENTITY codec (raw `Bytes` through, NO prost decode of the caller's bytes); 64 MiB ceiling.
  - `rpcStream(handle, method, payload, onEvent) -> subscription_id` — server-streaming via the identity codec; raw bytes to a uniffi callback interface; a bounded select-on-cancel task; `cancelSubscription` drops it.
  - `closeSession(handle)` — drop channel + endpoint, deregister (live subscriptions cancel on drop).
  - `natStats() -> { path, direct, relayed, lan }` — CLIENT-side `ConnectionPath` classification of this device's own session(s) (NOT a Core RPC).
- **Tests** — ALWAYS-RUN host unit tests: (a) identity codec round-trips arbitrary bytes incl. > 4 MiB; (b) handle-registry id/lookup/remove; (c) natStats classify mapping (loopback→Lan, public v4→Direct, relay→Relayed); (d) PairingRequest byte layout + signature-input ordering vs hand-built expected bytes. Plus a **LOOPBACK Tier-2 test** (`tests/loopback.rs`) that boots an Iroh-enabled in-process Core, drives `pair → openSession → rpcUnary(GetServerCapabilities, raw→IROH) → rpcStream(workspace.events, ≥1 raw event) → natStats==Lan → closeSession`, and SKIPs cleanly (returns Ok, logs a skip) when the Core has no Iroh (keychain-less CI) — the belt-and-suspenders pattern.

## Scope — out
- **Typed gRPC path assembly** (the per-method `/concerto.v1.Service/Method` strings + the prost message types) → **510**; 509 is a generic byte passthrough.
- **Key persistence (expo-secure-store) + the RN/JS pairing UI** → **511** (consumes `generateDeviceKeypair` + `pair`).
- **The generated Swift/Kotlin bindings + the Xcode/Gradle build wiring** → the mobile app tasks; this crate only produces the cdylib + the uniffi scaffolding.
- **`iroh-ffi`** — NOT added (D12). The `iroh = 0.98.2` / `iroh-relay = 0.98.0` pins are NOT bumped.

## Public interface this task locks
- The `#[uniffi::export]` free functions above + the `ConnectBlob` / `PairingInputs` / `DeviceKeypair` / `NatPath` / `NatStats` records/enums + the `StreamEventCallback` callback interface + the `IrohFfiError` error enum. (510/511 build against these.)
- The `IdentityCodec` opacity contract: bytes in == bytes out, both directions `bytes::Bytes`, no prost decode.

## Implementation notes
- **The load-bearing rule: this crate adds NO transport logic.** Every primitive delegates to `concerto-transport` (`connect_channel` / `classify_path`) or ports `pair-dial` verbatim (`pair_over_iroh` + the framing). If you find yourself re-implementing the Noise/QUIC plumbing, you are duplicating Task 212.
- **The FFI is sync, the transport is async.** Each exported fn blocks on a process-global multi-thread tokio runtime; the loopback test therefore calls them via `spawn_blocking` (a `block_on` inside a tokio async context panics).
- **SessionHandle is an opaque `u64`** backed by a `Mutex<HashMap>` registry — the simplest representation across uniffi.
- **natStats is client-side, NOT a Core RPC.** It mirrors `classify_path`: relay→Relayed, loopback/private/link-local→Lan, other IP→Direct.
- **Loopback skip is belt-and-suspenders.** The test COMPILES + RUNS on every lane but skips cleanly when `core.iroh()` is `None` (keychain-less CI) or the Core can't boot in the sandbox.

## Verification
**Tier 2.** A loopback double of the Iroh transport (two endpoints on one host, relays disabled) stands in for real cross-machine mobile↔Core; real device NAT diversity / relay fallback / on-device Swift/Kotlin binding execution stay Tier-3 (the phase gate).

1. `cargo fmt -p concerto-iroh-ffi` then `cargo fmt --all -- --check` — clean.
2. `cargo clippy -p concerto-iroh-ffi --all-targets -- -D warnings` — clean.
3. `cargo test -p concerto-iroh-ffi` — the 12 host unit tests pass; the loopback test runs (macOS + keychain) or skips cleanly (elsewhere).
4. `cargo test -p concerto-iroh-ffi --no-run` — the loopback test COMPILES on every lane.
5. `cargo deny check` — stays `advisories ok, bans ok, licenses ok, sources ok` (uniffi MPL-2.0 already ratified; no new SPDX/advisory).
6. `cargo build` (or `cargo check`) at the workspace root — unaffected (the crate is out of `default-members`).

## Definition of Done
- [x] New `crates/concerto-iroh-ffi` cdylib/staticlib/lib over `concerto-transport`, in `members` not `default-members`
- [x] uniffi 0.28 proc-macro mode (`setup_scaffolding!` + `#[uniffi::export]`), `default-features = false`, MSRV-1.82-safe + documented
- [x] All six+ frozen primitives implemented (generateDeviceKeypair / pair / openSession / rpcUnary / rpcStream / cancelSubscription / closeSession / natStats)
- [x] Identity passthrough codec (no prost decode of the caller's bytes), 64 MiB ceiling
- [x] 12 host unit tests (codec >4 MiB round-trip, registry, natStats mapping, pairing byte layout + signature ordering) pass
- [x] Loopback Tier-2 test boots an in-process Iroh Core, drives the full flow, skips cleanly with no iroh
- [x] No iroh-ffi added; iroh/iroh-relay pins unchanged; frozen proto untouched
- [x] fmt + clippy(-D warnings) + cargo deny all clean; default `cargo build` unaffected

## Outputs
- `crates/concerto-iroh-ffi/Cargo.toml` (new)
- `crates/concerto-iroh-ffi/src/lib.rs` (new — the FFI surface + `setup_scaffolding!`)
- `crates/concerto-iroh-ffi/src/codec.rs` (new — the identity passthrough tonic codec)
- `crates/concerto-iroh-ffi/src/registry.rs` (new — the opaque-handle session registry)
- `crates/concerto-iroh-ffi/src/nat.rs` (new — client-side path classification)
- `crates/concerto-iroh-ffi/src/pairing.rs` (new — `pair_over_iroh` port + framing)
- `crates/concerto-iroh-ffi/src/error.rs` (new — the uniffi error enum)
- `crates/concerto-iroh-ffi/tests/loopback.rs` (new — the Tier-2 loopback integration test)
- `Cargo.toml` (modified — `members` += the new crate, with a justifying comment; NOT `default-members`)

## Commit message
```
phase-5: ConcertoIroh native module — hand-rolled uniffi cdylib over concerto-transport (509)
```
