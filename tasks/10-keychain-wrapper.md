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
- [ ] Verification commands pass.
- [ ] `docs/interfaces/rust-api.md` reflects new public types.
- [ ] No `TODO` / `FIXME` / `unimplemented!()` in new code.
- [ ] `SecretValue::expose` is the only way to extract the inner string; verified.
- [ ] Smoke gate still green.
- [ ] Single commit created.

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
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** audit-log emission uses tracing only; structured AuditWriter (Task 44) replaces this.
- **Smoke-gate state:** unchanged.
