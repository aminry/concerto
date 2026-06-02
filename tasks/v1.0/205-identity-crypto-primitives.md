# Task 205 — Crypto Primitives: Ed25519 Identity, BLAKE2b `device_id`, `DeviceCert` (`crates/identity`)

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | — |
| Touches subsystem(s) | 12 (Security & Identity) |
| Smoke gate | unchanged |

## Goal
Create the new **`crates/identity`** crate holding the pure, side-effect-free cryptographic primitives the rest of Phase 2's security spine is built on: Ed25519 keypair generation / sign / verify, BLAKE2b `device_id` derivation, and the `DeviceCert` / `SignedDeviceCert` **deterministic-CBOR** serialization with sign + verify. No actor, no DB, no keychain, no async, no gRPC — just primitives, exhaustively unit-tested and shaped so Task 208 can drop a `cargo-fuzz` target on `verify_cert`. This is the trust anchor: everything in 206–211 composes these functions. Per the Phase-2 planning decision (C1), the primitives live in their own crate (mirroring the already-standalone `crates/transport` / `crates/relay`) while the `SecurityActor` + wiring stay in `crates/core`.

## Inputs to read before starting
- `design/12_Security_Identity.md` §3.1 (Ed25519 core identity; one per machine), §3.2 (the **exact** `DeviceCert` / `SignedDeviceCert` struct layout; deterministic CBOR with canonical ordering — "no JSON"; the 4 validation steps; R-1 rationale: standalone so recovery tools decode without a proto schema), §6.1 (cert-validation hot path target <200 µs — informs the allocation shape).
- `design/00_Architecture_Overview.md` §6.7 (locked crypto: Ed25519 device identity, Noise IK + AES-256-GCM — 205 does the Ed25519/cert half; Noise is 207/208).
- `crates/keychain/Cargo.toml` + `crates/keychain/src/api.rs` — small-crate conventions: `version.workspace = true` etc., the `[lib] name = "concerto_keychain"` form, the `src/api.rs` public-surface file that `regen-interfaces.sh` indexes, and the `zeroize` / `secrecy` secret-hygiene pattern to mirror for the private key.
- `deny.toml` — the `[licenses] allow` list + the dated **operator-ratification comment style**; the new crypto deps must clear `cargo deny check` (see Implementation notes).
- `tasks/v1.0/112-audit-log-subscribers.md` → "Handoff Notes" — the regen-interfaces depth-3 `api.rs` indexing rule, and the async-trait-vs-native convention (205 is sync, but the wiring tasks aren't).

## Scope — in
- New crate `crates/identity` (lib `concerto_identity`), added to `[workspace.members]` in the root `Cargo.toml`.
- **Keys** (`src/keys.rs`): wrappers over `ed25519-dalek` `SigningKey`/`VerifyingKey` — `generate()` (via `getrandom`/`rand_core`), `sign(msg)`, `verify(msg, sig)`. The private key zeroizes on drop and is never `Debug`/`Display` (mirror keychain's `secrecy`/`zeroize` use).
- **`device_id`**: `device_id(pubkey: &[u8; 32]) -> [u8; 32]` = BLAKE2b-256 of the Ed25519 public key. Deterministic; documented as the canonical derivation `12 §3.2` names.
- **Cert** (`src/cert.rs`): `DeviceCert` + `SignedDeviceCert` structs with the **exact** fields from `12 §3.2` (`version`, `device_id`, `device_pubkey`, `device_name`, `core_pubkey`, `issued_at`, `expires_at`, `capabilities`, `revocation_check_required`). Canonical CBOR encode/decode (see Implementation notes for the canonicalization decision) producing **byte-identical** output across runs/platforms. `sign_cert(core_priv, &DeviceCert) -> SignedDeviceCert`; `verify_cert(raw: &[u8], core_pub) -> Result<DeviceCert>` (signature + structural validity only); pure helper `DeviceCert::is_expired(now_unix) -> bool` (± skew left to the caller/206).
- `src/api.rs` **declaring the public surface directly** (the keychain convention: `regen-interfaces.sh` reads literal `pub struct`/`enum`/`fn` decls in `api.rs`, *not* `pub use` re-exports — so the canonical `DeviceCert`/`SignedDeviceCert`/key types live in `api.rs`; their `impl` blocks live in `cert.rs`/`keys.rs`).
- Tests: CBOR round-trip + **byte-stability**; sign→verify happy path; single-bit tamper → verify fails; wrong `core_pubkey` → fails; truncated/garbage input → `Err`, never panic; a **committed known-answer vector** (fixed 32-byte seed → fixed cert bytes → fixed 64-byte signature) that freezes the encoding across versions.

## Scope — out
- `DeviceCertIssuer` trait + `LocalCoreIssuer` (Task 206 — composes these primitives with the keychain + a clock + the issuance policy).
- Noise IK / XX and `snow` (Tasks 207/208 — the crate gains the Noise wrapper later; 205 adds **no** `snow` dependency).
- Storing the Core private key in the OS keychain, or loading identity at boot (that is `crates/core` actor wiring; 205 takes keys as plain inputs).
- Any gRPC/proto (`devices.proto` is Task 209) and revocation/expiry *enforcement* (206/209 own the policy; 205 only exposes `is_expired` as a pure helper).

## Public interface this task locks
- Crate `concerto-identity` / lib `concerto_identity`.
- The `DeviceCert` / `SignedDeviceCert` **field layout + canonical-CBOR encoding** — FROZEN. The signature is computed over these exact bytes and recovery tooling decodes them without the proto schema; the byte layout is a wire contract. New fields append-only with a `version` bump; never reorder.
- Function signatures: key `generate`/`sign`/`verify`, `device_id`, `sign_cert`/`verify_cert`, `DeviceCert::is_expired`.
- The committed known-answer test vector (the cross-version encoding freeze).

## Implementation notes
- **Canonical CBOR is the load-bearing detail.** `ciborium` (already a workspace dep — used by the agent-host frame codec) does not guarantee canonical map-key ordering. To make the bytes unambiguous, encode the cert as a **fixed-order CBOR structure**: a `serde`-derived struct serializes fields in declaration order, so the struct *is* the wire order — but verify this with the byte-stability test, and prefer no maps/sets inside the cert (use ordered `Vec` for `capabilities`). Document loudly that field order = wire order and is frozen. (If `ciborium`'s struct encoding proves non-canonical under test, fall back to an explicit positional CBOR array; decide in-task and freeze whichever passes byte-stability.)
- `capabilities` is `["admin"]` for all V1.0 certs (per `12 §3.2`; missing/empty ⇒ "admin" per `10` R-7). Keep the field; V1.0 always emits `["admin"]`.
- **License clearance is part of this task.** New deps — `ed25519-dalek` + `curve25519-dalek` (BSD-3-Clause), `blake2` (MIT/Apache-2.0), `rand_core` (MIT/Apache-2.0). All *should* already satisfy `deny.toml`'s allow-list; **run `cargo deny check` and confirm.** If any new SPDX surfaces, add it to the allow-list with a **dated operator-ratification comment** in the existing house style and flag it in Handoff. A copyleft/SSPL/BSL transitive dep is a **Stop-and-ask**, not a silent allow.
- Pin exact versions in `[workspace.dependencies]` with a rationale comment (mirror the `tonic`/`sqlx` pin comments). `zeroize`/`secrecy`/`getrandom`/`ciborium` are already workspace-pinned — reuse.
- Keep `verify_cert` allocation-light (no needless `Vec`/`String` clones on the verify path) so 206 can hit its <200 µs hot-path budget; a `#[cfg(test)]` timing sanity check is welcome but not a gate.

## Verification
Tier 1.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-identity` → all primitive tests + the known-answer vector pass.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → advisories/bans/licenses/sources all green (new crypto deps cleared; `deny.toml` updated + ratified if needed).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`rust-api.md` gains the `concerto-identity` surface from `src/api.rs`).

## Definition of Done
- [ ] `crates/identity` created, registered in workspace members, builds clean
- [ ] Ed25519 keys (zeroizing private key), BLAKE2b `device_id`, `DeviceCert`/`SignedDeviceCert` canonical CBOR + sign/verify implemented
- [ ] Byte-stability + tamper + wrong-key + garbage-input tests pass; known-answer vector committed
- [ ] `cargo deny check` green; any new license SPDX ratified in `deny.toml` with a dated comment
- [ ] Verification commands pass; interfaces regenerated + committed
- [ ] No `TODO`/`unimplemented!()`/`todo!()` in new code
- [ ] Single commit with the message below

## Outputs
- `Cargo.toml` (modified — `[workspace.members]` += `crates/identity`; `[workspace.dependencies]` += crypto pins)
- `crates/identity/Cargo.toml`, `crates/identity/src/lib.rs`, `crates/identity/src/keys.rs`, `crates/identity/src/cert.rs`, `crates/identity/src/api.rs` (new)
- `crates/identity/tests/cert_vectors.rs` (new — known-answer + tamper tests)
- `deny.toml` (modified only if a new SPDX needs ratification)
- `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-2: crates/identity — Ed25519, BLAKE2b device_id, DeviceCert

New crate holding the pure crypto primitives for the Phase-2 security
spine: Ed25519 key/sign/verify, BLAKE2b device_id, and the
deterministic-CBOR DeviceCert sign/verify with a committed known-answer
vector freezing the wire encoding. Fuzz-ready for Task 208.

Refs: tasks/v1.0/205-identity-crypto-primitives.md
```

## Handoff Notes (fill in when finishing)
- Drift from plan / Open questions for next task / Deliberate debt (e.g. CBOR canonicalization choice) / License ratifications / Smoke-gate state
