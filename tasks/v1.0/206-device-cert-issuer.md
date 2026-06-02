# Task 206 — `DeviceCertIssuer` Trait + `LocalCoreIssuer` (Core identity establishment + <200 µs validation)

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 205 |
| Touches subsystem(s) | 12 (Security & Identity), 18 (Distribution — trait seam) |
| Smoke gate | unchanged |

## Goal
Turn Task 205's pure cert primitives into the **issuance + validation seam** the rest of Phase 2 authenticates against. Define the `DeviceCertIssuer` trait (one of the `design/18 §3.7` extension seams) and its V1.0 MIT implementation `LocalCoreIssuer`, which mints `SignedDeviceCert`s **self-signed by the Core's Ed25519 identity** and validates incoming certs against that identity in **< 200 µs** (in-memory signature + expiry + revoke-set check, no DB hit). Crucially, this task also **establishes the Core's Ed25519 identity for the first time**: the keychain has a `SecretKind::CoreIdentityPrivateKey` *slot* (`crates/keychain/src/api.rs:49`) but **nothing in the codebase currently generates, stores, or loads a Core keypair** (grep-confirmed: zero `ed25519`/`SigningKey` references in `crates/core`). After this task the Core has a persistent identity, an issuer that can mint device certs from it, and a validator with the locked hot-path budget — the foundation Task 207 (pairing) and Task 210 (auth middleware) build on.

## Inputs to read before starting
- `design/12_Security_Identity.md` §3.10 — the **exact** `DeviceCertIssuer` trait (reproduce its signature verbatim; see Public interface below), the `LocalCoreIssuer` description (self-signed, caps `["admin"]`), and the V2.0 BSL impl names to **reserve as comments** (`OrgManagedCaIssuer`/`MdmIssuer`/`OidcBridgeIssuer`) for Task 707's trait-seam registry check.
- `design/12_Security_Identity.md` §3.1 (Core identity: Ed25519, one per machine, private key in OS keychain, public mirrored to `~/.concerto/identity.pub`), §3.2 (the **4 validation steps** the validator implements: ① verify signature against configured `core_pubkey`; ② `expires_at > now`; ③ `device_id` not revoked; ④ extract `capabilities`; plus the default 365-day expiry and `±5 min` clock-skew tolerance from §8), §3.11 (revocation: the in-memory `revoked_cache` is the source the validator consults; mirrored from DB on every revoke — this task owns the cache *read* side, Task 209 owns the *populate-on-revoke* side), §6.1 (the **< 200 µs** hot path: signature + expiry + revoke-set membership all in-memory, NO DB hit).
- `design/12_Security_Identity.md` §4 (the `IdentityState` in-memory struct: `core_priv: SigningKey`, `core_pub: VerifyingKey`, `cert_validator`, `revoked_cache: HashSet<DeviceId>`) — your `LocalCoreIssuer` is the concrete embodiment of the issuer+validator slice of this state.
- `tasks/v1.0/205-identity-crypto-primitives.md` (full task + its **Handoff Notes** once 205 is merged) — the FROZEN `DeviceCert`/`SignedDeviceCert` field layout, the `sign_cert`/`verify_cert`/`device_id`/`is_expired` signatures, the canonical-CBOR encoding decision, and the `crates/identity` crate layout (`src/api.rs` is the public surface `regen-interfaces.sh` reads; impls in `keys.rs`/`cert.rs`). **You compose these — do not reimplement crypto.**
- `crates/keychain/src/api.rs` — the `Secrets` handle (`get`/`set`/`delete`, all async), `SecretKind::CoreIdentityPrivateKey` (the slot you fill), and the `SecretValue::new(String)`/`expose()` secret-hygiene pattern. Note `get` returns `Ok(None)` when absent — that is the "first launch, generate it" branch.
- `crates/core/src/security/mod.rs` — the existing policy-only security module (`destructive`/`managed`/`path_policy`/`permission`/`tool_classes`); this is where the Core-side issuer wiring (keychain-backed signing key construction) lives. There is **no** identity/cert/noise module here yet — you add one.
- `design/00_Architecture_Overview.md` §6.7 (locked crypto: Ed25519 device identity) — confirms the identity primitive.

## Scope — in
- **`DeviceCertIssuer` trait** in `crates/identity` (it is cert-issuance logic, the natural home alongside 205's primitives; the keychain-backed signing key is *injected* by Core, so the crate stays keychain-free). Signature reproduced **verbatim** from `12 §3.10` (see below). Uses `#[async_trait]` (workspace convention — `async-trait` is already a workspace dep; `crates/identity` adds it).
- **`LocalCoreIssuer`** in `crates/identity`: holds the Core's `SigningKey` + `VerifyingKey` (the `core_pubkey`) and a **shared, cheaply-cloneable handle to the revoked set** (e.g. `Arc<RwLock<HashSet<[u8;32]>>>` or `Arc<ArcSwap<…>>` — pick the lowest-latency read; document the choice). `new(core_priv, core_pub, revoked) -> Self`.
  - `issue(req: PairingRequest)`: build a `DeviceCert` from `req` (device_pubkey, device_name; derive `device_id` via 205's `device_id()`; set `core_pubkey` = own; `issued_at = now`, `expires_at = issued_at + 365 days`; `capabilities = ["admin"]`; `revocation_check_required = true`), then `sign_cert(core_priv, &cert)`. **NOTE:** `PairingRequest` is *defined by Task 207* (the pairing flow owns its fields). To avoid a 206→207 dependency cycle, **define a minimal `PairingRequest` shape in `crates/identity` here** carrying exactly what issuance needs (`device_pubkey: [u8;32]`, `device_name: String`) and FREEZE it; 207 constructs it and adds the pairing-token/nonce/signature verification *before* calling `issue` (those are pairing-channel concerns, not issuance concerns). State this split loudly in Handoff.
  - `validate(raw: &[u8]) -> Result<DeviceContext>`: the 4 steps of `§3.2` — (1) `verify_cert(raw, core_pub)` from 205 (signature + structural validity); (2) expiry via `DeviceCert::is_expired(now)` with `±5 min` skew tolerance per `§8`; (3) `revoked_cache` membership on `device_id`; (4) build `DeviceContext { device_id, device_name, capabilities }`. **No DB, no async, no allocation on the happy path beyond what `verify_cert` needs** — this is the < 200 µs hot path.
  - `supported_capabilities()` → `&["admin"]`.
- **`DeviceContext`** type (the validated-identity output the auth middleware in Task 210 consumes): `device_id: [u8;32]`, `device_name: String`, `capabilities: Vec<String>`. Declare in `crates/identity/src/api.rs`. FROZEN.
- **Core identity establishment** in `crates/core/src/security/` (new module, e.g. `identity.rs` under the existing `security/`): a `load_or_create_core_identity(secrets: &Secrets) -> Result<(SigningKey, VerifyingKey)>` that: reads `SecretKind::CoreIdentityPrivateKey`; on `Ok(None)` generates a new key (205's `generate()`), persists the private key bytes to the keychain (encode the 32-byte seed — base64 in a `SecretValue`, documented), mirrors the public key to `~/.concerto/identity.pub` per `§3.1`, and emits the `CoreIdentityCreated` audit event; on `Ok(Some)` decodes and loads it. This is the function the Core boot path calls to get the keypair it injects into `LocalCoreIssuer`. **Wire it into the actual Core boot/runtime startup** so the identity exists at runtime (find the boot path — `crates/core/src/runtime/` / wherever the `SecurityActor`-adjacent state is assembled; if no obvious injection point exists yet, construct the issuer in the security module and expose a constructor the runtime calls, and note the exact wiring site in Handoff).
- Reserve (commented, not implemented) the V2.0 BSL issuer names in the trait doc / a registry comment so Task 707's completeness check passes.
- Tests: issue→validate round-trip (happy path); expired cert → `Err`; cert signed by a *different* Core key → `Err` (wrong `core_pubkey`); revoked `device_id` → `Err` (after inserting into the shared revoked set); garbage bytes → `Err`, never panic; `load_or_create` generates-then-reloads the same key across two calls (keychain round-trip, test-injected service); a `#[cfg(test)]` timing sanity check on `validate` (informational, **not** a gate) documenting the hot-path budget.

## Scope — out
- The pairing wire protocol, Noise XX channel, pairing-token issuance/verification, and `devices.proto` — **Task 207**. (206 defines the minimal `PairingRequest` issuance input and the `issue`/`validate` calls; 207 owns the ceremony around them.)
- Populating the revoked set on revoke, the `RevokeDevice`/`ListDevices` RPCs, and persisting `devices` rows — **Task 209**. (206 only *reads* the shared revoked-set handle; the `devices` table already exists in `migrations/0001` — do not add a migration.)
- The Noise IK session layer — **Task 208**.
- Auth middleware that calls `validate` on inbound gRPC metadata — **Task 210**.
- Any V2.0 issuer impl (`OrgManagedCaIssuer` etc.) — reserve names only.
- Cert auto-renewal at 30-days-before-expiry (`§3.2 R-2`) — V1.0 issues 365-day certs; renewal is a later task, not here.

## Public interface this task locks
- **`DeviceCertIssuer` trait** (reproduced verbatim from `12 §3.10`) — FROZEN published extension seam:
  ```rust
  #[async_trait]
  pub trait DeviceCertIssuer: Send + Sync + 'static {
      async fn issue(&self, req: PairingRequest) -> Result<SignedDeviceCert>;
      fn validate(&self, raw: &[u8]) -> Result<DeviceContext>;
      fn supported_capabilities(&self) -> &'static [&'static str];
  }
  ```
- **`PairingRequest`** (issuance-input shape, defined here, consumed by 207): `{ device_pubkey: [u8;32], device_name: String }` — FROZEN; 207 extends the *pairing message* (token/nonce/sig) but constructs this exact struct to call `issue`.
- **`DeviceContext`** `{ device_id: [u8;32], device_name: String, capabilities: Vec<String> }` — FROZEN; the validated-identity contract Task 210 consumes.
- **`LocalCoreIssuer`** constructor signature `new(core_priv, core_pub, revoked_set_handle)` and the V1.0 issuance policy (365-day expiry, `["admin"]`, `revocation_check_required = true`).
- The keychain encoding of `CoreIdentityPrivateKey` (the 32-byte-seed serialization) — FROZEN, since a re-encode orphans an existing Core's identity.

## Implementation notes
- **The < 200 µs hot path is the load-bearing constraint.** `validate` must be sync and allocation-light: clone the `Arc` revoked-set handle once at construction, take a read lock (or `ArcSwap::load`) per call, and lean on 205's allocation-light `verify_cert`. The `DeviceContext` does allocate (`device_name`/`capabilities`) — that is acceptable (it is the *output*), but do no other heap work on the success path. The `#[cfg(test)]` timing check documents the budget but is not a CI gate (loopback timing is environment-sensitive — see how Task 102's spike treats sub-ms numbers).
- **Revoked-set ownership.** 206 holds a *read* handle; Task 209 owns inserting on revoke. Pick a type that makes the write side trivial for 209 (an `Arc<RwLock<HashSet<…>>>` or `arc-swap`) and document it in the FROZEN constructor so 209 wires the same handle into both the issuer and the revoke path. Keep the read path lock-free or near-lock-free for the hot-path budget.
- **`async-trait`.** The workspace uses `async-trait` (e.g. `crates/core/src/handlers/sessions.rs:27`, `crates/core/Cargo.toml:64`). `issue` is async (matches the trait + future issuers that hit an MDM/CA API); `validate`/`supported_capabilities` are sync per the design signature. Add `async-trait` to `crates/identity/Cargo.toml`.
- **Error type.** Reuse `crates/identity`'s error type from 205 (whatever `verify_cert` returns); add issuer-specific variants (`Expired`, `Revoked`, `WrongCore`, `Malformed`) as needed. Follow 205's Handoff for the exact `Result` alias.
- **Keychain seed encoding.** Store the 32-byte Ed25519 seed, not a DER/PKCS#8 blob, to keep it minimal and recovery-tool-friendly (consistent with 205's "recovery tools decode without a schema" ethos). Base64 the seed into the `SecretString`. Document the format inline; it is FROZEN.
- **Audit event.** `CoreIdentityCreated` is in the `AuditKind` enum (`design/12 §3.7`). Emit it on first generation via the existing audit path (Task 112 shipped the `AuditLogSubscriber` fan-out / `audit()` shortcut — follow `design/12 §5.1`'s `audit(kind, subject_ids)` shape; check the live audit module for the exact call site). If the boot path doesn't yet have an audit handle in reach, note it in Handoff rather than forcing it.
- **License.** No new third-party crates beyond `async-trait` (already a workspace dep) and whatever 205 introduced. Still run `cargo deny check` — it must stay green. No `snow` here (that is 207/208).
- **Cross-platform.** No `std::os::unix`-only types in the issuer or the identity loader; the `~/.concerto/identity.pub` mirror uses `std::fs` + a home-dir crate already in the tree (check `dirs`/`directories` usage). Keep the Windows CI lane (Task 113) green.

## Verification
Tier 1.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-identity` → trait + `LocalCoreIssuer` issue/validate/expiry/wrong-core/revoked/garbage tests pass; informational hot-path timing prints.
4. `cargo test -p concerto-core` (security/identity module) → `load_or_create_core_identity` generate-then-reload round-trip passes against a test-injected keychain service.
5. `cargo test --workspace --no-fail-fast` → all pass.
6. `cargo deny check` → advisories/bans/licenses/sources green (no new external deps requiring ratification; confirm).
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`rust-api.md` gains `DeviceCertIssuer`/`LocalCoreIssuer`/`PairingRequest`/`DeviceContext` from `crates/identity/src/api.rs`; the `crates/core/src/security/identity.rs` surface is internal and at depth 4 → no `core` diff, per Task 112's regen note — confirm).

## Definition of Done
- [ ] `DeviceCertIssuer` trait (verbatim `§3.10` signature) + `LocalCoreIssuer` implemented in `crates/identity`, composing 205's primitives
- [ ] `PairingRequest` (issuance input) + `DeviceContext` declared in `crates/identity/src/api.rs` and FROZEN
- [ ] `validate` implements the 4 steps of `§3.2` with `±5 min` skew, in-memory revoked-set, no DB hit; hot-path budget documented
- [ ] Core Ed25519 identity established: `load_or_create_core_identity` generates+stores+mirrors on first launch, reloads thereafter, emits `CoreIdentityCreated`, and is wired into the Core boot/runtime path
- [ ] V2.0 BSL issuer names reserved as comments for Task 707's registry check
- [ ] Issue/validate/expiry/wrong-core/revoked/garbage + keychain round-trip tests pass
- [ ] `cargo deny check` green; verification commands pass; interfaces regenerated + committed
- [ ] No `TODO`/`unimplemented!()`/`todo!()` in new code (deliberate ones in Handoff)
- [ ] Single commit with the message below

## Outputs
- `crates/identity/src/api.rs` (modified — `DeviceCertIssuer`, `PairingRequest`, `DeviceContext` decls)
- `crates/identity/src/issuer.rs` (new — `LocalCoreIssuer` impl + the reserved-names comment) + `crates/identity/src/lib.rs` (modified — `mod issuer;`)
- `crates/identity/Cargo.toml` (modified — `async-trait`)
- `crates/identity/tests/issuer.rs` (new — issue/validate/expiry/wrong-core/revoked/garbage)
- `crates/core/src/security/identity.rs` (new — `load_or_create_core_identity` + keychain seed encoding) + `crates/core/src/security/mod.rs` (modified — `pub mod identity;`)
- `crates/core/src/<boot/runtime path>` (modified — call `load_or_create_core_identity` and construct the issuer at startup; exact file recorded in Handoff)
- `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-2: DeviceCertIssuer trait + LocalCoreIssuer + Core identity

Adds the DeviceCertIssuer extension seam (design/12 §3.10) and its V1.0
MIT impl LocalCoreIssuer, composing the Task 205 primitives: 365-day
self-signed device certs and a <200 µs in-memory validate (sig + expiry
+ revoke-set, no DB). Establishes the Core's Ed25519 identity for the
first time (generate-or-load via the keychain CoreIdentityPrivateKey
slot, public mirrored to ~/.concerto/identity.pub). Reserves the V2.0
BSL issuers for Task 707.

Refs: tasks/v1.0/206-device-cert-issuer.md
```

## Handoff Notes (fill in when finishing)
- Where the issuer trait lives / Core-identity wiring site / revoked-set handle type for 209 / PairingRequest split with 207 / audit wiring state / Open questions / Deliberate debt / Smoke-gate state
