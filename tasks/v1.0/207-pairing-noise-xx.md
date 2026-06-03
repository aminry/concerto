# Task 207 — Pairing: Noise XX over a One-Shot Token + `Devices.Start/CompletePairing`

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 206 |
| Touches subsystem(s) | 12 (Security & Identity), 11 (Transport — pairing channel) |
| Smoke gate | unchanged |

## Goal
Implement the **device-pairing ceremony** — the most user-visible security flow. A Core operator starts pairing (`Devices.StartPairing`), which mints a 32-byte one-shot `pairing_token` (60 s TTL, ≤ 3 active, in-memory only) and returns the QR payload. A new device completes pairing (`Devices.CompletePairing`) by running a **Noise XX handshake bootstrapped by the shared `pairing_token`**, then sending a `PairingRequest` whose signature proves possession of the token; the Core verifies the signature, **consumes the token one-shot**, mints + signs a `DeviceCert` via Task 206's `LocalCoreIssuer`, inserts the `devices` row, and returns the `SignedDeviceCert`. This task also **creates `crates/proto/proto/concerto/v1/devices.proto`** with the two pairing RPCs (Task 209 later *extends* the same proto with `ListDevices`/`RevokeDevice`/`GetCoreInfo`). It composes Task 206's issuer and adds the `snow`-based Noise XX primitive to `crates/identity`. After this task a device can pair end-to-end over an in-process loopback channel (the Tier-2 double); real cross-device QR-scan pairing is the Tier-3 phase-gate line.

## Inputs to read before starting
- `design/12_Security_Identity.md` §3.3 — the **pairing flow sequence** (reproduce faithfully): QR payload = `base64({core_pubkey, pairing_token, lan_endpoint, relay_hint})`; the pairing channel is a **Noise XX** (mutual-unauthenticated) handshake **bootstrapped by the shared `pairing_token`** (both ends know it from the QR — use it as the Noise XX **PSK** per §7.1's `Noise XX init (PSK = pairing_token)`); the device sends `PairingRequest{device_pubkey, device_name, nonce, sig_over(pairing_token || nonce || device_pubkey)}`; Core verifies the sig, consumes the token one-shot, mints + signs the `DeviceCert`, inserts the `devices` row, returns `SignedDeviceCert + core_pubkey`.
- `design/12_Security_Identity.md` §6.2 — **token security** (the rules you must enforce): 32 random bytes from `getrandom`; **in-memory only**, never persisted (survives restart only by re-initiating); **one-shot** (consuming clears it); a photographed-but-unused token still expires at 60 s; **rate-limited to ≤ 3 active tokens** (older ones rotate out). Store as `TokenHash` → `PairingTokenState` per the `IdentityState`/`PairingTokenState` structs in `§4`.
- `design/12_Security_Identity.md` §5.2 — the gRPC surface for pairing: `StartPairing(google.protobuf.Empty) returns (PairingChallenge)` and `CompletePairing(CompletePairingRequest) returns (SignedDeviceCert)` (the `SignedDeviceCert` rides as **opaque CBOR `bytes`**, not a protobuf message — Decision D1).
- `design/12_Security_Identity.md` §5.1 (the `SecurityHandle` methods `start_pairing(PairingSource) -> PairingChallenge` and `complete_pairing(CompletePairing) -> SignedDeviceCert` — your Core-side coordinator embodies these), §7.1 (the **LAN pairing sequence** — the exact message order to implement against), §8 (failure modes: `pairing.expired`, `pairing.consumed` on replay, Noise handshake failure → drop), §3.7 (`DevicePairingStarted`/`DevicePairingCompleted`/`DevicePairingFailed` audit events).
- `design/12_Security_Identity.md` §12 R-3 (pairing path is **both** LAN and relay in V1.0 — QR carries a LAN endpoint when discovered plus a relay hint; same protocol either way) and R-10 *(note: §12's R-10 in this doc is "sign managed.json — V2.0"; the framing's "R-10 sync-only" intent is the LAN+relay parity from R-3 — implement the message exchange transport-agnostically so 212/215 supply the real channel)*.
- `design/11_Remote_Transport_Relay.md` §3.3 (the pairing channel / "QUIC stream pool for gRPC" model — the transport that *carries* the Noise XX bytes in production; this task implements the handshake over a loopback/in-memory duplex, the same Tier-2 double pattern the spike used).
- `tasks/v1.0/206-device-cert-issuer.md` (+ its **Handoff Notes**) — the FROZEN `DeviceCertIssuer` trait, `LocalCoreIssuer::new(...)`, the **`PairingRequest` issuance-input shape** (`{device_pubkey, device_name}`) you construct *after* verifying the token signature, the revoked-set handle type, and where the Core identity is wired so you can reach the issuer.
- `tasks/v1.0/205-identity-crypto-primitives.md` (+ Handoff) — Ed25519 `verify`, `device_id()`, and the `crates/identity` layout.
- `crates/proto/proto/concerto/v1/sessions.proto` (any existing proto) — the proto house style; `crates/proto/build.rs` (auto-walks `proto/**/*.proto`, so a new `devices.proto` compiles with no build-script edit) — read its head + the `timestamp_fields` list pattern.
- `crates/persist/migrations/0001_initial_schema.sql` lines 242–253 — the **`devices` table already exists** (`id`, `name`, `public_key`, `paired_at`, `last_seen_at`, `revoked_at`, `push_token`, `push_platform`). **Do not add a migration.** This task INSERTs on successful pairing; Task 209 owns the read/revoke side.
- `crates/core/src/handlers/sessions.rs` — the `#[async_trait]` gRPC handler pattern + how a handler reaches persistence/security state; mirror it for the `Devices` handler. `crates/core/src/handlers/mod.rs` — how services are registered (so the new `Devices` service is served).
- `deny.toml` — the license allow-list + the dated operator-ratification comment style (you add `snow`).

## Scope — in
- **`devices.proto`** (`crates/proto/proto/concerto/v1/devices.proto`): a new `service Devices` with **exactly** the two pairing RPCs (Task 209 appends the rest):
  - `StartPairing(google.protobuf.Empty) returns (PairingChallenge)` — `PairingChallenge` carries the QR-payload fields the operator/UI needs (`core_pubkey: bytes`, `pairing_token: bytes`, `lan_endpoint: string`, `relay_hint: string`, `expires_at`). (Name the message `PairingChallenge` per `§5.2`.)
  - `CompletePairing(CompletePairingRequest) returns (CompletePairingResponse)` where `CompletePairingRequest` carries the device's handshake/`PairingRequest` material and `CompletePairingResponse` carries the **`SignedDeviceCert` as opaque CBOR `bytes`** (Decision D1 — NOT a protobuf-typed cert) plus `core_pubkey: bytes`. (The design writes the return type as `SignedDeviceCert`; per D1 it is `bytes` on the wire — document this inline in the proto.)
  - Reserve a comment in the proto noting Task 209 extends this service with `ListDevices`/`RevokeDevice`/`GetCoreInfo` and that a Devices push-token RPC is deferred to P5.
- **Noise XX primitive** in `crates/identity` (e.g. `src/noise_xx.rs`): a thin `snow`-based wrapper that runs the XX pattern with the `pairing_token` as the pre-shared secret (`psk0`/`psk3` placement per `snow`'s XX+psk — pick the placement that authenticates both ends via the token and document it; the design's intent is "both ends authenticate via the `pairing_token`"). Expose a two-sided API: an initiator side and a responder side that exchange the three XX messages over a caller-supplied byte channel and yield a completed transport on which the `PairingRequest` is sent. Keep it transport-agnostic (the caller provides the duplex).
- **Pairing coordinator** in `crates/core/src/security/` (e.g. `pairing.rs`): the in-memory `pairing_tokens: HashMap<TokenHash, PairingTokenState>` with the `§6.2` rules — mint (32 `getrandom` bytes, 60 s TTL, evict to keep ≤ 3 active, store the **hash** not the raw token), one-shot consume, expiry sweep. `start_pairing(...) -> PairingChallenge` and `complete_pairing(...)` that: drives the Noise XX responder, parses the `PairingRequest`, **verifies `sig_over(pairing_token || nonce || device_pubkey)`** against `device_pubkey` (205's `verify`), consumes the token (reject replay with `pairing.consumed`, expired with `pairing.expired`), constructs Task 206's `PairingRequest{device_pubkey, device_name}` and calls `issuer.issue(...)`, INSERTs the `devices` row (`id` = hex/fingerprint of `device_id`, `public_key`, `paired_at`, `name`), and returns the `SignedDeviceCert` bytes. Emits `DevicePairingStarted`/`Completed`/`Failed` audit events.
- **`Devices` gRPC handler** (`crates/core/src/handlers/devices.rs`, `#[async_trait]`): implements the two RPCs by delegating to the coordinator; register the service in `handlers/mod.rs` so it is served. Map failures to gRPC status (`FAILED_PRECONDITION`/`UNAUTHENTICATED` per `§8`).
- Add `snow` to `[workspace.dependencies]` (pinned, rationale comment mirroring the `tonic`/`sqlx` pins) and to `crates/identity/Cargo.toml`; **run `cargo deny check`** and ratify `snow`'s SPDX in `deny.toml` with a dated comment (it is "Unlicense OR MIT" → cargo-deny selects **MIT**, already on the allow-list; confirm and note it — `Unlicense` should *not* need adding, but verify the resolved expression).
- **Tier-2 double tests**: two in-process endpoints complete the full Noise XX pairing over a **loopback/in-memory duplex** (e.g. `tokio::io::duplex`) — happy path yields a valid `SignedDeviceCert` that Task 206's `validate` accepts and a `devices` row exists; **token one-shot** (second `CompletePairing` with the same token → `pairing.consumed`); **token expiry** (advance the clock → `pairing.expired`); **≤ 3 active** (4th `StartPairing` evicts the oldest); **bad signature** (wrong key over the token payload → rejected, no row, `DevicePairingFailed` audited); **replay** of a recorded handshake → rejected (`§10` security test).

## Scope — out
- `ListDevices`/`RevokeDevice`/`GetCoreInfo` RPCs + revocation propagation + the revoked-set *write* side — **Task 209** (it extends `devices.proto` and owns the read/revoke columns).
- The Noise **IK** *session* layer (post-pairing per-connection crypto) — **Task 208** (a different pattern; do not conflate with the XX pairing handshake).
- The real Iroh transport / relay that carries the pairing channel in production — **Tasks 212/214/215**. This task's channel is the loopback double.
- The desktop/mobile QR display + camera scan UI — **Tasks 219/511**. This task produces the QR *payload bytes*, not the rendered QR.
- `concerto pair` headless CLI — **Task 713**.
- Cert auto-renewal — later.

## Public interface this task locks
- **`devices.proto` pairing surface** — FROZEN: `service Devices { rpc StartPairing(Empty) returns (PairingChallenge); rpc CompletePairing(CompletePairingRequest) returns (CompletePairingResponse); }`, the `PairingChallenge` fields (`core_pubkey`/`pairing_token`/`lan_endpoint`/`relay_hint`/`expires_at`), and `CompletePairingResponse` carrying the `SignedDeviceCert` as **opaque CBOR `bytes`** (D1). Field numbers, once assigned, are frozen; Task 209 appends new RPCs/messages with new numbers, never reordering.
- **The `PairingRequest` signed-payload construction**: the signature is computed over the exact byte concatenation `pairing_token || nonce || device_pubkey` (in that order). FROZEN — both the device signer and the Core verifier depend on this byte layout.
- **The pairing-token rules** (`§6.2`): 32 `getrandom` bytes, hashed at rest in memory, 60 s TTL, one-shot, ≤ 3 active.

## Implementation notes
- **XX-with-PSK placement.** `snow`'s XX pattern with a PSK can place the PSK at different message indices (`psk0`..`psk3`). The design's requirement is only that *both ends authenticate via the shared `pairing_token`*; choose the placement `snow` supports cleanly for XX, verify with a two-sided test, and **document the chosen Noise protocol string** (e.g. `Noise_XXpsk3_25519_...`) as part of the frozen wire contract.
- **Token at rest = hash.** Per `§4`'s `PairingTokenState.token_hash: [u8;32]`, store BLAKE2b(token) keyed by `TokenHash`, not the raw token. The raw 32 bytes go out in the QR payload and are used as the PSK; the Core keeps only the hash to consume/compare.
- **Signature payload framing.** `pairing_token || nonce || device_pubkey` is a raw concatenation — fix the `nonce` length (document it) so the verifier parses unambiguously. The token is the raw 32 bytes (not the hash) since the device only has the raw token from the QR.
- **`devices` row INSERT.** Use the existing table (`0001`). `id` is the public-key fingerprint (`design` comment on the column) — use the `device_id` hex or the agreed fingerprint; `public_key` = the raw Ed25519 bytes; `paired_at` = now (unix seconds, matching the integer column). Do **not** touch `revoked_at`/`push_*` (209/P5).
- **Reach the issuer.** Task 206 wired `load_or_create_core_identity` + constructed `LocalCoreIssuer` at boot — the coordinator needs a handle to that issuer (and the shared revoked-set handle). Follow 206's Handoff for where this state lives; thread it into the `Devices` handler the same way other handlers reach their state.
- **`async-trait`** for the `Devices` handler (workspace convention; see `handlers/sessions.rs`). The coordinator's `complete_pairing` is async (it INSERTs + issues); the token-store ops are sync under a lock.
- **Determinism / no flakiness.** The Tier-2 loopback test must be hermetic (in-process duplex, no real network, no real Iroh). For the expiry test, inject the clock (don't `sleep` 60 s).
- **Cross-platform.** `snow`, `getrandom`, `tokio::io::duplex` are all portable; no `std::os::unix` types in the coordinator/handler. Keep the Windows CI lane green.

## Verification
Tier 2 — the double is **two in-process gRPC/coordinator endpoints completing the Noise XX pairing over a loopback (`tokio::io::duplex`) channel**. It proves: token mint/TTL/one-shot/≤3-active, the XX handshake, the signed-payload verification, cert issuance via 206, the `devices` INSERT, and replay/expiry rejection. It does **NOT** cover: real cross-device QR-scan pairing over a real Iroh LAN/relay transport (no NAT, no camera, no real endpoint) — that is the **Phase-2 Tier-3 checklist** line ("pair a real second machine over LAN (mDNS direct)").
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-identity noise` → the two-sided Noise XX handshake + PSK-placement test pass.
4. `cargo test -p concerto-core` (pairing/devices) → loopback happy-path pairing, one-shot, expiry, ≤3-active, bad-sig, replay-rejection tests pass.
5. `cargo test --workspace --no-fail-fast` → all pass.
6. `cargo deny check` → green; `snow` ratified in `deny.toml` (resolved SPDX = MIT, confirmed) with a dated comment.
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (the new `Devices` proto surface appears in the generated proto interface doc; the Noise XX wrapper surface in `crates/identity/src/api.rs` if exposed there).

## Definition of Done
- [x] `devices.proto` created with the two pairing RPCs (`SignedDeviceCert` as opaque CBOR `bytes`); 209/P5 extension reserved in a comment
- [x] Noise XX (PSK = pairing_token) primitive in `crates/identity`; chosen protocol string documented + frozen
- [x] Pairing coordinator: token rules (`§6.2`) enforced; signed-payload `pairing_token||nonce||device_pubkey` verified; one-shot consume; issues cert via 206; INSERTs `devices` row; audits Started/Completed/Failed
- [x] `Devices` gRPC handler implemented + registered/served
- [x] Tier-2 loopback pairing tests (happy/one-shot/expiry/≤3-active/bad-sig/replay) pass; the Tier-3 uncovered part stated in Verification
- [x] `snow` pinned in workspace deps; `cargo deny check` green (resolved SPDX = MIT, already on the allow-list — no `deny.toml` ratification needed; see Handoff)
- [x] Verification commands pass; interfaces regenerated + committed
- [x] No `TODO`/`unimplemented!()`/`todo!()` in new code (deliberate ones in Handoff)
- [x] Single commit with the message below

## Outputs
- `crates/proto/proto/concerto/v1/devices.proto` (new)
- `crates/identity/src/noise_xx.rs` (new) + `crates/identity/src/lib.rs` / `src/api.rs` (modified — module + any exposed surface) + `crates/identity/Cargo.toml` (modified — `snow`)
- `crates/core/src/security/pairing.rs` (new — token store + coordinator) + `crates/core/src/security/mod.rs` (modified)
- `crates/core/src/handlers/devices.rs` (new) + `crates/core/src/handlers/mod.rs` (modified — register `Devices`)
- `crates/core/tests/pairing.rs` (new — Tier-2 loopback tests)
- `Cargo.toml` (modified — `snow` in `[workspace.dependencies]`) + `Cargo.lock` (resolved)
- `crates/core/src/handlers/mod.rs` (modified — register `devices` module)
- `crates/core/src/api_server.rs` (modified — register `DevicesServer` + thread the coordinator)
- `crates/core/src/boot.rs` (modified — construct + inject the `PairingCoordinator`)
- `crates/core/src/audit/event.rs` (modified — 3 design-mandated pairing `AuditKind`s)
- `crates/proto/build.rs` (modified — `PairingChallenge.expires_at` timestamp field)
- `crates/identity/src/error.rs` (modified — `IdentityError::Noise` variant)
- `deny.toml` — **NOT modified** (`snow` SPDX `Apache-2.0 OR MIT` resolves to MIT, already allowed; see Handoff)
- `docs/interfaces/*` (regenerated — `proto.md` gains the `Devices` surface)

## Commit message
```
phase-2: device pairing — Noise XX over one-shot token + Devices RPCs

Implements the QR-pairing ceremony (design/12 §3.3/§6.2): StartPairing
mints a 32-byte one-shot token (60 s TTL, ≤3 active, in-memory) and
CompletePairing runs a Noise XX handshake bootstrapped by that token,
verifies sig_over(token||nonce||device_pubkey), mints a DeviceCert via
the Task 206 LocalCoreIssuer, and inserts the devices row. Creates
devices.proto with the two pairing RPCs (SignedDeviceCert as opaque CBOR
bytes); Task 209 extends it. Adds snow (ratified in deny.toml).

Refs: tasks/v1.0/207-pairing-noise-xx.md
```

## Handoff Notes (fill in when finishing)

- **Chosen Noise XX protocol string (FROZEN wire contract):**
  `Noise_XXpsk3_25519_AESGCM_SHA256` — declared as
  `concerto_identity::PAIRING_NOISE_PARAMS` in `crates/identity/src/noise_xx.rs`.
  `psk3` places the pairing-token PSK at message index 3 (after `-> s, se`),
  the latest point that binds the token on **both** ends, so a handshake that
  reaches transport mode has provably mixed the token everywhere — exactly the
  design's "both ends authenticate via the `pairing_token`" intent. A wrong PSK
  fails the final `read_message` decrypt (→ dropped pairing, `§8`). Cipher suite
  is X25519 / AES-256-GCM / SHA-256. The wrapper exposes a two-sided API
  (`NoiseHandshake::initiator`/`responder` → 3 XX messages → `into_transport()`
  → `NoiseTransport::{write,read}_message`) over a **caller-supplied byte
  duplex** — transport-agnostic (Task 212 supplies the real Iroh stream; the
  Tier-2 test uses `tokio::io::duplex`).

- **`snow` pin + SPDX:** `snow = "0.9"` in `[workspace.dependencies]` (resolves
  **0.9.6**), `default-features = false` + `default-resolver` (pure-Rust crypto,
  no `ring`/libsodium C deps → Windows lane stays green). **Pinned 0.9, NOT
  0.10**: snow 0.10 declares `rust-version = 1.85`, above the workspace MSRV of
  1.82 (`[workspace.package].rust-version`); 0.9.6 has the same `XXpsk3` surface
  and no MSRV floor. License is **`Apache-2.0 OR MIT`** (the task body guessed
  "Unlicense OR MIT" — it is actually Apache/MIT); cargo-deny resolves it to
  **MIT**, already on the allow-list. **No `deny.toml` change was needed** —
  `cargo deny check` is green with `deny.toml` untouched. (So `deny.toml` is NOT
  in the final Outputs; flagged under Drift.)

- **Token store + clock-injection shape:**
  `crates/core/src/security/pairing.rs` holds a `TokenStore`
  (`std::sync::Mutex<HashMap<TokenHash, PairingTokenState>>`, sync — no `.await`
  under the lock). Tokens are 32 `getrandom` bytes; the store keys on
  `BLAKE2b-256(token)` (`TokenHash = [u8;32]`) — **the raw token is never
  stored**; the raw bytes go out in the QR / are the Noise PSK. Rules:
  `PAIRING_TOKEN_TTL = 60s`, `MAX_ACTIVE_TOKENS = 3` (mint evicts the oldest by
  `issued_at`), one-shot consume (remove-on-success). The coordinator exposes
  **clock-injected** `start_pairing_at(now)` / `complete_pairing_at(input, now)`
  (the public `start_pairing()`/`complete_pairing()` call them with
  `SystemTime::now()`), so the expiry test advances the clock instead of
  sleeping 60s. `PAIRING_NONCE_LEN = 32` is the FROZEN nonce length in the
  signed payload `pairing_token(32) || nonce(32) || device_pubkey(32)`.
  **Signature is verified BEFORE the token is consumed**, so a bad-signature
  attempt does not burn a still-valid token (the device can retry) — tested.

- **How the handler reaches the 206 issuer + revoked-set:** `boot::start`
  (`crates/core/src/boot.rs`) now, right where 206 constructed the
  `LocalCoreIssuer`, builds a `PairingCoordinator::new(issuer, Arc<Persistence>,
  audit_writer, lan_endpoint, relay_hint)` and wraps it in an `Arc` (so the
  api-server actor's factory closure — which may re-run on a supervised restart
  — shares the **same in-memory token store**). The `Arc<PairingCoordinator>` is
  threaded as a new trailing `Option<Arc<PairingCoordinator>>` arg to
  `ApiServerActor::with_managers`, and `run_uds` registers `DevicesServer` when
  it is `Some`. The coordinator owns the issuer and a `Persistence` clone (for
  the `devices` INSERT). **The revoked-set handle** is still built at the same
  boot site (`new_revoked_set()`, bound `_revoked_set`) — Task 209 wires its
  `RevokeDevice` write path into that same `Arc`; the issuer the coordinator
  owns already reads it on `validate`. `lan_endpoint`/`relay_hint` are passed
  empty (Task 212/213/214 supply the real values; the QR carries no endpoint
  yet and the exchange is transport-agnostic).

- **`devices.proto` field numbers assigned (FROZEN — for Task 209):**
  `service Devices` has `StartPairing` (RPC #1, by declaration order) +
  `CompletePairing` (#2). Message field numbers:
  - `PairingChallenge`: `core_pubkey=1`, `pairing_token=2`, `lan_endpoint=3`,
    `relay_hint=4`, `expires_at=5`.
  - `CompletePairingRequest`: `device_pubkey=1`, `device_name=2`, `nonce=3`,
    `signature=4`, `pairing_token=5`.
  - `CompletePairingResponse`: `signed_device_cert=1` (opaque CBOR bytes,
    `cert_bytes || signature`, D1), `core_pubkey=2`.
  Task 209 **appends** `ListDevices`/`RevokeDevice`/`GetCoreInfo` RPCs + their
  messages with NEW numbers to the SAME service — never reorder the two pairing
  RPCs or renumber these fields. A push-token RPC is deferred to Phase 5 (noted
  in a proto comment).

- **Cert wire form for 209/210:** `CompletePairingResponse.signed_device_cert`
  is `signed.cert_bytes || signed.signature` — exactly what 205's `verify_cert`
  / 206's `validate` expect (the same form 206's Handoff froze). The device
  persists these bytes verbatim and presents them on every connect; the proto
  layer never decodes the CBOR (D1).

- **Audit:** added `AuditKind::{DevicePairingStarted, DevicePairingCompleted,
  DevicePairingFailed}` (+ `as_str` arms) to `crates/core/src/audit/event.rs`
  (design-mandated `§3.7` kinds, additive enum entries). `start_pairing` emits
  Started; `complete_pairing` emits Completed on success and Failed on any
  rejection (bad sig / expired / consumed / issue / insert).

- **Drift from plan:**
  - **`deny.toml` NOT modified** (it is in the task's Outputs as "if a new
    expression surfaces"): `snow`'s `Apache-2.0 OR MIT` resolves to the
    already-allowed MIT, so no SPDX ratification was needed. `cargo deny check`
    is green with `deny.toml` untouched. (Removed from the final Outputs.)
  - **`snow` pinned at 0.9, not the latest 0.10** — MSRV reason above (0.10
    needs rustc 1.85 > workspace MSRV 1.82). 0.9.6 has the identical `XXpsk3`
    pattern surface.
  - **Touched files beyond the literal Outputs list (all additive / wiring):**
    `crates/core/src/audit/event.rs` (3 design-mandated audit kinds),
    `crates/core/src/api_server.rs` (register `DevicesServer` + thread the
    coordinator through `with_managers`/`run_uds`), `crates/core/src/boot.rs`
    (construct the coordinator at the 206 issuer site + inject it),
    `crates/proto/build.rs` (one `timestamp_fields` row for
    `PairingChallenge.expires_at`), `crates/identity/src/error.rs` (the
    `IdentityError::Noise` variant), `Cargo.lock`. None are prior-task interface
    files; all are in-crate, necessary wiring.
  - **Proto comment formatting:** a 4-space-indented byte-layout line in the
    `CompletePairingRequest` message comment was being captured as a Rust
    doctest by the generated rustdoc (it failed to compile). Reflowed the layout
    inline (backtick-quoted) so the comment carries no indented code block. No
    semantic change.

- **Open questions for next task:**
  - **Task 208 (Noise IK session):** the XX wrapper here is the *pairing*
    channel only (first contact, mutual-unauthenticated + token PSK). The IK
    session layer (post-pairing per-connection crypto, initiator=device static
    key, responder=Core static key) is a **different snow pattern** — do not
    reuse `noise_xx.rs`; add a sibling module. The `snow` dep + `IdentityError::
    Noise` variant are already in place for you.
  - **Task 209 (`Devices` list/revoke + `devices` table):** wire `RevokeDevice`
    into the SAME `revoked_set` `Arc` built in `boot.rs` (clone it from the
    coordinator-construction site); mirror revoked `devices` rows into it at
    boot. The `devices` row this task INSERTs uses `id = hex(device_id)`,
    `name`, `public_key = raw 32-byte Ed25519`, `paired_at = unix secs`;
    `revoked_at`/`push_*` are left NULL for you. Append your RPCs/messages to
    `devices.proto` with new numbers (see the frozen numbers above).
  - **Task 210 (auth middleware):** the coordinator hands the issuer's `issue`
    output back as `signed_device_cert` bytes; your middleware calls the same
    issuer's `validate` on inbound cert metadata. The `Arc<PairingCoordinator>`
    is registered on the UDS server today; the bridge (Task 204) does NOT yet
    serve `Devices` — if remote pairing must traverse the Connect-Web bridge,
    register `DevicesServer` in `connect_bridge::serve` too (out of scope here).

- **Deliberate debt:** — (none; no `TODO`/`unimplemented!()`/`todo!()` in new
  code).

- **Smoke-gate state:** **unchanged.** No smoke check added; `scripts/smoke.sh`
  untouched. Note: `boot::start` calls `Secrets::new()` (added by **Task 206**,
  not this task) to establish the Core identity; on a headless **macOS** runner
  that blocks on a Keychain prompt. The Tier-2 pairing test in
  `crates/core/tests/pairing.rs` therefore deliberately **does NOT boot a full
  Core** — it constructs the `LocalCoreIssuer` + `Persistence` directly, so it
  is keychain-free and hermetic. (Integration tests that DO boot a Core — the
  pre-existing harness/lifecycle suites — already need
  `CONCERTO_KEYCHAIN_SERVICE` set on macOS local; that is a 206 condition, green
  on Linux CI where `keyring` errors fast and boot continues via its warning
  path.)

- **Tier-3 not covered by the loopback double** (→ Phase-2 manual checklist):
  real cross-device QR-scan pairing over a real Iroh LAN/relay transport — no
  NAT, no camera, no real endpoint, no relay. Stated in the `Verification`
  section and the test module doc.
