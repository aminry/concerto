# Task 10 — Keychain Wrapper

| Field | Value |
|---|---|
| Phase | 1 |
| Size | small (≤4h) |
| Depends on | 05 |
| Touches subsystem(s) | 09 (Persistence), 12 (Security) |
| Smoke gate | unchanged |

## Goal
Wrap `keyring-rs` in a typed Concerto-specific API that namespaces entries, returns typed values, and logs access (without the secret). After this task, every later subsystem that needs a secret calls `Secrets::get(SecretKind::...)` instead of touching `keyring` directly. Provider tokens, GitHub PATs, and the Core's Ed25519 identity all flow through this single API.

## Inputs to read before starting
- `design/09_Persistence.md` §3.7 (typed keychain wrapper), §5.2 (Secrets interface).
- `design/00_Architecture_Overview.md` §6.2 (`keyring-rs` v4 is the locked choice) and §6.7 (crypto — Ed25519 identity).
- `tasks/05-error-and-logging-baseline.md` → "Handoff Notes".

## Scope — in
- Add `keyring = "3"` (or current stable v3+; the doc says v4, but the actual published version may differ — pin to what's published) to `crates/keychain/Cargo.toml`.
- Add `zeroize = "1"` and `secrecy = "0.10"` for in-memory secret hygiene.
- Implement `crates/keychain/src/lib.rs`:
  ```rust
  pub enum Provider { Anthropic, OpenAI, Gemini, Bedrock, Vertex }
  
  pub enum SecretKind {
      ProviderToken(Provider),
      GithubPat,
      DevicePairingKey,
      CoreIdentityPrivateKey,
      PushExpoApiKey,
  }
  
  pub struct SecretValue(secrecy::SecretString);
  
  impl SecretValue {
      pub fn new(s: String) -> Self;
      pub fn expose(&self) -> &str;   // explicit, audit-loggable in callers
  }
  
  pub struct Secrets { /* ... */ }
  
  impl Secrets {
      pub fn new() -> Self;
      pub async fn get(&self, kind: SecretKind) -> Result<Option<SecretValue>>;
      pub async fn set(&self, kind: SecretKind, value: SecretValue) -> Result<()>;
      pub async fn delete(&self, kind: SecretKind) -> Result<()>;
  }
  ```
- Namespacing: entries are stored under service `"concerto"` and account `kind.to_account_string()`. The account string scheme:
  - `ProviderToken(Anthropic)` → `"provider_token.anthropic"`
  - `GithubPat` → `"vcs.github.pat"`
  - `DevicePairingKey` → `"device.pairing_key"`
  - `CoreIdentityPrivateKey` → `"identity.core_private_key"`
  - `PushExpoApiKey` → `"push.expo_api_key"`
- Errors: wrap `keyring::Error` into `crates/keychain/src/error.rs::SecretsError` with variants `NotFound`, `AccessDenied`, `PlatformError`. Convert to the top-level `Error` via `From`.
- Audit-log integration: emit a `tracing::info!` event on every successful access (`"secret accessed"`, kind, no value). The structured audit-log writer arrives in Task 44; for V0.1 Phase 1, a `tracing` event is sufficient.
- Add `crates/keychain/src/api.rs` re-exporting the public types.
- Tests:
  - Round-trip: `set` → `get` returns the same value.
  - Missing key returns `Ok(None)`.
  - Delete-then-get returns `Ok(None)`.
  - Tests should be gated behind `#[cfg(target_os = "macos")]` initially; Linux requires Secret Service to be running which CI may not have. Document this gate in the test module's top comment.

## Scope — out
- No identity-key generation (Task 41 or the security subsystem task).
- No multi-device key store (V1.0).
- No actual audit-log writer to JSONL (Task 44 builds that).
- No keychain enrollment / unlock UI (out of scope; relies on OS prompts).

## Public interface this task locks
- Rust: `crates/keychain/src/api.rs` — `pub enum SecretKind`, `pub enum Provider`, `pub struct SecretValue`, `pub struct Secrets`, `Secrets::get/set/delete` async methods.
- Account-string namespacing scheme above. Changing any account string is a hard break (would orphan existing keychain entries).
- Service name `"concerto"` for all keychain entries.

## Implementation notes
- `keyring::Entry::new("concerto", "<account>")` is the right shape.
- On macOS, the first access may prompt the user. Tests should set a non-default service name (e.g., `"concerto-test-<uuid>"`) to avoid colliding with real installations.
- `secrecy::SecretString` zeroes its memory on drop; do not store raw `String` in `SecretValue`.
- The `expose()` method is named to make callers think about whether they're crossing a trust boundary. Don't use it casually.
- Document in the lib.rs top comment: "the wire codes for these secrets are referenced from `design/09 §3.7`."

## Verification
1. `cargo build -p concerto-keychain` → succeeds.
2. `cargo test -p concerto-keychain` on macOS → all tests pass. On Linux/Windows CI, tests are skipped via `cfg`; the build still passes.
3. `cargo clippy -p concerto-keychain -- -D warnings` → clean.
4. Manual test on a Mac:
   ```
   cargo run --example secrets_demo  # see Outputs
   ```
   Set, get, delete a `ProviderToken(Anthropic)` from the command line and confirm it appears in Keychain Access.
5. `./scripts/regen-interfaces.sh && git diff docs/interfaces/rust-api.md` → updated; commit.
6. `cargo deny check` → clean.

## Definition of Done
- [x] Verification commands pass.
- [x] `docs/interfaces/rust-api.md` reflects new public types.
- [x] No `TODO` / `FIXME` / `unimplemented!()` in new code.
- [x] `SecretValue::expose` is the only way to extract the inner string; verified.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `crates/keychain/Cargo.toml` (modified — adds keyring, secrecy, zeroize)
- `crates/keychain/src/lib.rs` (modified)
- `crates/keychain/src/error.rs` (new)
- `crates/keychain/src/api.rs` (new)
- `crates/keychain/examples/secrets_demo.rs` (new — small CLI for manual smoke)
- `crates/keychain/tests/round_trip.rs` (new)
- `crates/error/Cargo.toml` (modified — depends on concerto-keychain for `From<SecretsError>`)
- `crates/error/src/error.rs` (modified — `Secrets(#[from] SecretsError)` variant)
- `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-1: typed keychain wrapper

crates/keychain wraps keyring-rs with namespaced typed accounts per
design/09 §3.7. SecretKind enum covers provider tokens, GitHub PAT,
device pairing keys, Core identity, push API keys. SecretValue uses
secrecy::SecretString.

Refs: tasks/10-keychain-wrapper.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **Workspace `keyring` pin bumped from `"2"` to `{ version = "3", default-features = false }`.** Task spec said pin to "current stable v3+". `keyring 4.0.1` is the absolute latest but the design doc named v4 before v3 stabilized and the task body explicitly wrote `keyring = "3"`; sticking to v3 keeps the API surface the task was written against. v3.6.3 is the resolved version. `default-features = false` at the workspace level matches the posture for `sqlx`/`tonic` (Task 05 drift). The `Cargo.toml` was modified — not in Outputs, adding here in retrospect.
  - **Workspace `Cargo.toml` also adds `secrecy = "0.10"` and `zeroize = "1"` as workspace deps.** Outputs listed these under `crates/keychain/Cargo.toml`, but to keep version pins consistent across crates that may later hold secrets (Task 12 Security, Task 22 agent spawn), they live at the workspace root and `crates/keychain/Cargo.toml` references them with `workspace = true`.
  - **`concerto-keychain` is acyclic with `concerto-error`.** The orchestrator prompt called this out. Implementation: `concerto-keychain` carries its own `SecretsError` + crate-local `Result` alias (no `concerto-error` dep). `concerto-error` adds `concerto-keychain` as a dep and the new `Error::Secrets(#[from] SecretsError)` variant bridges at the boundary. `cargo check --workspace` confirms no cycle. The `From<SecretsError> for Error` impl is the `thiserror`-generated `#[from]`.
  - **`Error::wire_code()` adds the `"secrets"` arm.** Authorized by Outputs (`crates/error/src/error.rs`). Added the matching unit test in `crates/error/tests/wire_codes.rs` (also in Outputs by extension — the test file was created in Task 05 and exists; we only appended one test).
  - **`SecretValue` has a `pub(crate)` field, not `pub`.** Spec said `pub struct SecretValue(secrecy::SecretString)` which would expose the inner field publicly. That would defeat the whole "`expose()` is the only escape hatch" invariant. Field is `pub(crate)` so external callers cannot tuple-destructure their way around `expose()`. `regen-interfaces.sh` still captures the enum shape verbatim.
  - **`Secrets::with_service_for_test` added** (not in spec). Tests need to override the service name to avoid colliding with real entries; the spec said "set a non-default service name (e.g., `"concerto-test-<uuid>"`)" but didn't give the API. Marked `#[doc(hidden)]` so it's not part of the public surface even though it has to be `pub` for integration tests in `tests/` to reach it. Production code uses `Secrets::new()` which binds to `"concerto"`.
  - **`SecretsError::AccessDenied` is currently unreachable.** keyring 3 on macOS surfaces user-cancelled prompts as `PlatformFailure` with an OS error code; we can't reliably distinguish that from other backend failures via the public API, so all backend errors except `NoEntry` map to `PlatformError`. The variant stays in the enum because Linux's Secret Service backend (V1.0) surfaces a distinct `Locked` state that will map to `AccessDenied`. Documented in the error module's doc comment.
  - **Test uniqueness uses `pid+nanos+seq+tag` instead of `uuid`.** Spec said "e.g., `concerto-test-<uuid>`". Adding the `uuid` crate just for test-name uniqueness was disproportionate — used a `pid + SystemTime + AtomicU64` combo that's monotonic per-process and unique across processes for all practical purposes. Each test cleans up its own entries.
  - **`cargo deny check` is clean — no new licenses added.** keyring 3 + secrecy + zeroize + security-framework + core-foundation all ship under MIT/Apache-2.0, which are already in `deny.toml`. No allow-list extension required.
- **Open questions for next task:**
  - When Task 11 (runtime skeleton) and Task 22 (agent spawn) start pulling secrets through `Secrets::get`, they'll need a single `Arc<Secrets>` or fresh `Secrets::new()` per call. The struct is `Clone + Default + Debug`; cloning is essentially free (one `Cow::Borrowed("concerto")`). No reason to wrap in `Arc` unless future state is added.
  - The `tracing::info!` audit event uses `target: "concerto::keychain"` so Task 16's logging discipline can filter / route it. Task 44 (audit log writer JSONL) should ingest the `kind` and `account` fields verbatim; no extra translation layer needed.
  - keyring 4.0.1 exists; if a future task wants the v4 API (notably the new `Entry` builder pattern and credential-attribute extensions), bumping is a one-line workspace change but will need re-verification of `apple-native` semantics. v3 is fine for V0.1.
  - The `keyring` workspace dep is `default-features = false`. When Linux support lands (V1.0), per-crate feature toggles will need `linux-native` or `sync-secret-service`; the `apple-native` feature is already opted in by `crates/keychain/Cargo.toml`.
  - The example binary uses the real `"concerto"` service. After running `cargo run --example secrets_demo -- delete`, the Keychain Access entry under service=`concerto`, account=`provider_token.anthropic` is gone — no developer-machine cleanup needed.
- **Deliberate debt:** audit-log emission uses `tracing::info!` only; structured `AuditWriter` (Task 44) will subscribe to this target and persist JSONL. No `TODO`/`FIXME` markers in code.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still prints "Smoke gate: PASSED (no checks active yet — Phase 0)". The keychain wrapper is not yet exercised by smoke; Task 15+ will add the first real assertion.
