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
- [x] `DeviceCertIssuer` trait (verbatim `§3.10` signature) + `LocalCoreIssuer` implemented in `crates/identity`, composing 205's primitives
- [x] `PairingRequest` (issuance input) + `DeviceContext` declared in `crates/identity/src/api.rs` and FROZEN
- [x] `validate` implements the 4 steps of `§3.2` with `±5 min` skew, in-memory revoked-set, no DB hit; hot-path budget documented
- [x] Core Ed25519 identity established: `load_or_create_core_identity` generates+stores+mirrors on first launch, reloads thereafter, emits `CoreIdentityCreated`, and is wired into the Core boot/runtime path
- [x] V2.0 BSL issuer names reserved as comments for Task 707's registry check
- [x] Issue/validate/expiry/wrong-core/revoked/garbage + keychain round-trip tests pass
- [x] `cargo deny check` green; verification commands pass; interfaces regenerated + committed
- [x] No `TODO`/`unimplemented!()`/`todo!()` in new code (deliberate ones in Handoff)
- [x] Single commit with the message below

## Outputs
- `crates/identity/src/api.rs` (modified — `DeviceCertIssuer`, `PairingRequest`, `DeviceContext`, `LocalCoreIssuer` decls + `generate_seed`)
- `crates/identity/src/issuer.rs` (new — `LocalCoreIssuer` impl + the reserved-names comment) + `crates/identity/src/lib.rs` (modified — `mod issuer;` + re-exports)
- `crates/identity/src/keys.rs` (modified — `generate_seed` persistence path) + `crates/identity/src/error.rs` (modified — `Result` alias + `Expired`/`Revoked`/`WrongCore` variants) — added to Outputs (in-crate, additive; see Handoff)
- `crates/identity/Cargo.toml` (modified — `async-trait` + `tokio` dev-dep)
- `crates/identity/tests/issuer.rs` (new — issue/validate/wrong-core/revoked/garbage over the public trait)
- `crates/core/src/security/identity.rs` (new — `load_or_create_core_identity` + FROZEN hex seed encoding) + `crates/core/src/security/mod.rs` (modified — `pub mod identity;`)
- `crates/core/src/boot.rs` (modified — the **exact boot file**: calls `load_or_create_core_identity` + constructs `LocalCoreIssuer` after the audit writer, best-effort per Handoff)
- `crates/core/src/audit/event.rs` (modified — `AuditKind::CoreIdentityCreated` variant + `as_str` arm) — added to Outputs (design-mandated additive enum entry; see Handoff)
- `crates/core/Cargo.toml` (modified — `concerto-identity`/`concerto-keychain`/`hex` deps)
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

## Handoff Notes (filled in when finishing)

- **Where things live:**
  - `DeviceCertIssuer` trait + `PairingRequest` + `DeviceContext` + the
    `LocalCoreIssuer` struct/constructor are declared in
    `crates/identity/src/api.rs` (the regen-indexed public surface). The
    `LocalCoreIssuer` impl (`issue`/`validate`/`supported_capabilities` + the
    reserved V2.0 BSL issuer names) lives in `crates/identity/src/issuer.rs`.
  - Core identity loader: `crates/core/src/security/identity.rs`
    (`load_or_create_core_identity`).
  - **Core-identity wiring site (exact boot file): `crates/core/src/boot.rs`**,
    in `boot::start`, immediately AFTER the AuditWriter is spawned (~line 147).
    It calls `load_or_create_core_identity(&Secrets::new(), &home_dir,
    &audit_writer)`, then constructs the `LocalCoreIssuer` with a fresh
    `new_revoked_set()`. The issuer is bound as `_core_issuer` (not yet consumed
    — Task 210 injects it into the auth middleware; Task 209 shares the
    `revoked_set` handle with the revoke path). There is no `runtime.rs`
    injection point for an identity/issuer yet; `boot.rs` is the assembly site
    and the natural place for 209/210 to thread the handles into the gRPC
    factory closure.

- **Revoked-set handle type for 209 (FROZEN):**
  `RevokedSet = Arc<RwLock<HashSet<[u8; 32]>>>` (`std::sync`, not `tokio` — the
  `validate` hot path is sync and must not `.await`). Build one with
  `concerto_identity::new_revoked_set()`. The validator takes a *read* lock per
  call; **Task 209's `RevokeDevice` path takes a *write* lock to `insert` the
  revoked `device_id`** into the SAME `Arc` it must clone from the issuer-
  construction site in `boot.rs`. Boot currently builds an empty set; 209 wires
  it to the `devices` table (mirror revoked rows in at boot + insert on revoke).

- **`PairingRequest` split with 207 (FROZEN):** `PairingRequest { device_pubkey:
  [u8;32], device_name: String }` is the *issuance-input* slice only. **207 owns
  the full pairing message** (32-byte token, nonce, and the device's signature
  proving possession of `device_pubkey`); 207 verifies those pairing-channel
  concerns, then constructs THIS exact struct and calls `issue`. `device_id` is
  derived inside `issue` (via `device_id(&device_pubkey)`) — the caller does not
  (and cannot) supply it. The issuer crate stays a leaf (207 depends on 206, not
  vice-versa).

- **Audit wiring state:** `CoreIdentityCreated` is emitted on first generation
  via the existing `AuditWriter::append` path (`AuditActor::System`, subject
  `Secret:"core_identity_private_key"`, details carry the hex `core_pubkey`).
  The variant did **not** exist in the live `AuditKind` enum (V0.1 froze a
  subset; `design/12 §3.7` lists it) — I added `AuditKind::CoreIdentityCreated`
  (+ its `"core_identity_created"` `as_str` arm) to
  `crates/core/src/audit/event.rs`. This is an additive enum change (the enum
  doc says "additions are additive"); see Drift below.

- **`validate` latency:** release-build `#[cfg(test)]` timing
  (`validate_hot_path_timing_informational`, **informational, not a gate**):
  **~29 µs/call**, comfortably inside the `design/12 §6.1` **< 200 µs** budget
  (Ed25519 verify dominates; the revoked-set read lock + skew check are
  negligible). Debug build measures ~210 µs/call (unoptimized — not
  representative; the budget is for the release hot path).

- **FROZEN keychain seed encoding:** the Core's Ed25519 private key is stored as
  the **lowercase-hex of the 32-byte seed** (32 bytes → 64 hex chars), under
  `SecretKind::CoreIdentityPrivateKey` (account `identity.core_private_key`). A
  re-encode would orphan an existing Core's identity, so this is frozen. The
  public key is mirrored to `~/.concerto/identity.pub` as `hex(pubkey) + "\n"`,
  self-healed on every boot.

- **Drift from plan:**
  - The task body said to **base64** the seed; the same task's License note
    forbids new third-party crates beyond `async-trait`, and no base64 crate is
    in the workspace. I froze **hex** instead — already in the tree (agent-host
    + the unix cookie path), so the dependency graph is unchanged and `cargo
    deny` stays green. The *encoding* (bare 32-byte seed, minimal +
    schema-free-decodable) is what the task froze; hex satisfies that intent
    exactly. Documented inline in `security/identity.rs`.
  - Added `crates/core/src/audit/event.rs` (the `CoreIdentityCreated` variant +
    `as_str` arm) and `crates/core/Cargo.toml` (deps) and
    `crates/identity/src/{keys.rs,error.rs,lib.rs}` to the touched set beyond the
    literal `Outputs` list. All are in-crate, additive, and necessary:
    `event.rs` for the design-mandated audit kind; `keys.rs`/`error.rs`/`lib.rs`
    for `generate_seed` (the keychain persistence path that keeps `KeyPair` from
    ever re-exposing its private bytes) and the issuer `Result` alias/variants.
  - The FROZEN trait renders in `docs/interfaces/rust-api.md` with the result
    alias spelled `IdentityResult<…>` (design/12 §3.10 writes bare
    `Result<…>`). The alias is `crate::error::Result`; I imported it as
    `IdentityResult` in `api.rs` to avoid shadowing the two-arg
    `Result<T, IdentityError>` form used by 205's already-frozen signatures in
    the same file. The trait *semantics* are verbatim; only the alias spelling
    differs.

- **Open questions for next task (207 constructs `PairingRequest` to call
  `issue`):**
  - On-wire cert form is `cert_bytes || signature` (from 205's handoff):
    `validate(raw)` and `verify_cert` expect exactly that framing. When 207/209
    move the cert over the wire / into gRPC metadata, concatenate
    `SignedDeviceCert.cert_bytes` with `.signature`.
  - `issue` derives `issued_at` from `SystemTime::now()` and sets `expires_at =
    issued_at + 365d`. 207 does not pass a clock.
  - 207 must enforce the pairing token/nonce/sig BEFORE calling `issue` — the
    issuer trusts that `device_pubkey` was proven to belong to the pairing peer
    (the issuer does no possession check; that's the pairing channel's job).
  - To consume the issuer from a gRPC service, 209/210 should thread the
    `LocalCoreIssuer` (and the shared `revoked_set`) from `boot.rs` into the
    `ApiServerActor::with_managers` factory closure (where every other handle is
    injected). The construction site is already there as `_core_issuer`.

- **Deliberate debt:** — (none; no `TODO`/`unimplemented!()`/`todo!()` in new
  code).

- **Smoke-gate state:** **unchanged.** No smoke check added; establishing the
  Core identity at boot is a fast keychain read (+ a one-time generate on first
  launch) and does not alter the existing boot smoke path. `scripts/smoke.sh`
  unaffected.
