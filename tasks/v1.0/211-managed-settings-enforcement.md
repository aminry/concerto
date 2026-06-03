# Task 211 — `managed.json` Enforcement: `disable_remote`, Allowed Devices, Max Paired Devices

| Field | Value |
|---|---|
| Phase | 2 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 210 |
| Touches subsystem(s) | 12 (Security & Identity), 11 (Transport — remote-gate seam) |
| Smoke gate | unchanged |

## Goal
Turn the **parsing-only** `managed.json` reader into an **enforcement** layer for the V1.0 security/pairing/remote fields the design adds. `crates/core/src/security/managed.rs` already parses `managed.json` (V0.1: `version`, `max_permission_mode`, `allow_yolo`, `allow_bypass_destructive_guard`, `preamble_template_path`, `max_reasoning_level`) — this task **extends the parser** with the `design/12 §3.8` security fields (`allowedPairingDevices`, `maxPairedDevicesPerUser`, `relayUrl`, `auditForwardEndpoint`, `denyFilesystemPaths`, and the remote toggle `disable_remote` per `design/11 §6.4`) and **provides the policy predicates** other Phase-2 tasks call at their enforcement points: `remote_disabled()`, `is_pairing_allowed(pubkey_fingerprint)`, `max_paired_devices()`, plus the `ManagedSettingsViolation` audit on a malformed/invalid file (with revert-to-default). The enforcement *points* live in other tasks (212 gates relay registration off `disable_remote`; 207 checks the pairing whitelist + max-paired cap); **211 owns the predicates + the violation audit**, and names which consumer task calls each so they wire against a stable surface. After this task, an org's `managed.json` can lock a Core to LAN-only, whitelist exactly which device pubkeys may pair, and cap the paired-device count — and a broken policy file is audited and safely defaulted rather than silently mis-enforced.

## Inputs to read before starting
- `design/12_Security_Identity.md` §3.8 — the **`managed.json` schema (V1.0)**: reproduce the field set you enforce here — `allowedPairingDevices` (`null` = any / array of **device-pubkey fingerprints** = whitelist), `maxPairedDevicesPerUser` (e.g. `4`), `relayUrl`, `auditForwardEndpoint` (e.g. `syslog://…`), `denyFilesystemPaths` (array), plus the existing permission-mode fields. **"Validation runs on load; invalid fields are flagged in the audit log (`ManagedSettingsViolation`) and the field reverts to the default."** Also §3.7 (the `ManagedSettingsViolation` + `ManagedSettingsLoaded` audit kinds), §3.9 (deployment trust modes — the table that motivates `disable_remote` / self-hosted-relay enforcement).
- `design/11_Remote_Transport_Relay.md` §6.4 — **LAN-only mode**: `managed.json` `disable_remote = true` makes the Core (1) **not register with any relay**, (2) **continue to publish mDNS**, (3) **accept only LAN connections**. §3.9 (same binary, two trust models — `disable_remote` eliminates relay involvement). These three behaviors are what `remote_disabled()` gates in Task 212/214's relay registration + remote-accept path.
- `crates/core/src/security/managed.rs` — the **live parser** you extend: `ManagedFile` (the `#[derive(Deserialize)]` on-disk schema), `ManagedPolicy` (the parsed in-memory struct + `Default`), `load_managed_policy` / `parse_managed_policy_at` (the **"malformed JSON → warn + default, never refuse to boot; unknown `version` → hard error"** contract you must preserve), and `ManagedPolicySource` (the hot-reload `watch` broadcaster). Mirror its field-by-field `Option<…>` + `unwrap_or(default)` parsing style and its `tracing::warn!`-on-bad-value pattern.
- `design/12_Security_Identity.md` §5.1 — `managed(&self) -> &ManagedSettings` (the handle accessor the predicates back) + `audit(kind, subject_ids)` (the shortcut to emit `ManagedSettingsViolation`; check the live audit module for the exact call shape Task 112 shipped).
- `tasks/v1.0/210-auth-middleware.md` (+ **Handoff Notes**) — confirms the auth layer is in place (this task's pairing/remote predicates are policy *atop* an authenticated surface) and how the security module's shared state is reached at boot.
- `tasks/v1.0/207-pairing-noise-xx.md` (+ **Handoff Notes**) — the pairing coordinator (`crates/core/src/security/pairing.rs`) is the **consumer** of `is_pairing_allowed(pubkey_fingerprint)` (checked before minting a cert) and `max_paired_devices()` (checked against the live `devices` count before issuance). You PROVIDE the predicates; note that 207's coordinator (or a follow-up) calls them — do not rewire 207's flow here beyond exposing the predicate surface.
- `tasks/v1.0/112-audit-log-subscribers.md` → **Handoff Notes** — the deferred "where does the opt-in audit-forwarder config live?" question (Task 112 left Stdout/Syslog/Https subscribers implemented but registered nowhere, awaiting a `managed.json` audit-forwarders source). Read it to decide the `auditForwardEndpoint` scope below.
- `crates/persist/migrations/0001_initial_schema.sql` lines 242–253 — the `devices` table (`max_paired_devices()` enforcement counts active rows: `revoked_at IS NULL`). No migration.
- `tasks/v1.0/README.md` §5.3 (`rust` command set) + §6 row 211.

## Scope — in
- **Extend the parser** in `crates/core/src/security/managed.rs`: add to `ManagedFile` + `ManagedPolicy` the V1.0 security fields — `disable_remote: bool` (default `false`), `allowed_pairing_devices: Option<Vec<String>>` (`None`/JSON `null` = any; a `Vec` = the fingerprint whitelist), `max_paired_devices_per_user: Option<u32>` (`None` = unlimited), `relay_url: Option<String>`, `audit_forward_endpoint: Option<String>`, `deny_filesystem_paths: Vec<String>` (default `[]`). Map the JSON camelCase keys (`disable_remote` is snake per `design/11`; `allowedPairingDevices`/`maxPairedDevicesPerUser`/`relayUrl`/`auditForwardEndpoint`/`denyFilesystemPaths` are camelCase per `design/12 §3.8`) — use `#[serde(rename = "…")]` to match the design's exact spelling and FREEZE the on-disk keys. Preserve the **"malformed → warn + default, never refuse to boot; unknown `version` → hard error"** contract.
- **Policy predicates** (methods on `ManagedPolicy`, or a thin `ManagedEnforcement` wrapper the security handle exposes):
  - `remote_disabled(&self) -> bool` → `disable_remote`.
  - `is_pairing_allowed(&self, fingerprint: &str) -> bool` → `true` when `allowed_pairing_devices` is `None`; else membership in the whitelist (document the fingerprint format = the device-id/pubkey fingerprint 207 already computes, so caller + policy agree).
  - `max_paired_devices(&self) -> Option<u32>` → the cap (consumer compares against the live active `devices` count).
  - `relay_url(&self)` / `deny_filesystem_paths(&self)` accessors (consumed by 214 / the allow-list policy respectively — expose them now; their enforcement points are out, see below).
- **`ManagedSettingsViolation` audit on invalid policy**: when a field fails validation (e.g. `maxPairedDevicesPerUser` non-numeric, `allowedPairingDevices` a non-array/non-null, an unparseable `relayUrl`), emit `ManagedSettingsViolation` (per §3.7) and **revert that field to its default** — matching the existing per-field `warn`-and-default behavior, now also audited. Emit `ManagedSettingsLoaded` on a clean load. (Whole-file malformed JSON keeps the existing warn+full-default path; add the violation audit there too.)
- **Name the consumer seam for each predicate** (Implementation notes + Handoff, not code here): `remote_disabled()` → **Task 212/214** gate relay registration + remote-accept (still publish mDNS); `is_pairing_allowed()` → **Task 207** pairing coordinator (pre-issuance check; reject with a pairing-denied audit); `max_paired_devices()` → **Task 207/209** at issuance (count active `devices`, reject when at cap); `deny_filesystem_paths()` → the `design/12 §3.5` allow-list policy (later); `relay_url()` → **Task 214** relay config.
- **Resolve Task 112's deferred audit-forwarder config question** — IN scope as a **decision + the parsed field only**: `audit_forward_endpoint: Option<String>` is parsed + exposed here (this is the `managed.json` home Task 112 anticipated). **Out of scope:** actually registering the `SyslogSubscriber`/`HttpsForwarderSubscriber` from it (that's the audit-pipeline wiring — leave the one-line `boot.rs` `vec!` extension to a Phase-5/ops task and **explicitly note** that 211 provides the config field but does not wire the subscriber). State this split loudly so the boundary is unambiguous.
- **Tier-1 tests**: whitelist allow (fingerprint in list → `true`) / deny (absent → `false`) / `null` → any (`true`); `max_paired_devices()` returns the cap and `None` when unset; `disable_remote` read (`true`/`false`/absent→`false`); malformed `managed.json` → `ManagedSettingsViolation` audited + full default (Core still "boots"); a single invalid field (e.g. bad `maxPairedDevicesPerUser` type) → that field defaults + violation audited while valid sibling fields parse; the existing V0.1 parser tests stay green.

## Scope — out
- The **actual relay registration / remote-accept gating** off `disable_remote` — **Tasks 212/214** call `remote_disabled()`; not built here. (211 provides the predicate; 212/214 own the relay code.)
- The **pairing-time whitelist check** + **max-paired rejection** wiring inside the pairing flow — **Task 207** (its coordinator calls `is_pairing_allowed` / `max_paired_devices`); 211 only exposes the predicates and names the seam.
- **Registering audit forwarders** from `auditForwardEndpoint` (the `SyslogSubscriber`/`HttpsForwarderSubscriber` `boot.rs` `vec!` wiring) — out (Task 112 left this seam; 211 supplies the config field, not the registration).
- `denyFilesystemPaths` **enforcement** in the agent allow-list — later (`design/12 §3.5`); 211 only parses + exposes it.
- The permission-mode / yolo / bypass fields' enforcement — already shipped in V0.1 (`resolve_effective_mode` etc.); untouched here beyond keeping their tests green.
- Signed `managed.json` verification (`org_root_pubkey` / `.sig`) — **V2.0** (`§12 R-10`).
- Any UI surfacing of locked settings / Diagnostics panel — Desktop tasks (`design/12 §3.8` "Surface in Tray + Settings → Diagnostics").

## Public interface this task locks
- **The extended `managed.json` on-disk keys** (`disable_remote`, `allowedPairingDevices`, `maxPairedDevicesPerUser`, `relayUrl`, `auditForwardEndpoint`, `denyFilesystemPaths`) with their exact spelling + `null`/default semantics — FROZEN (an org's policy file is a stable contract; renames orphan deployed files).
- **The policy-predicate surface** on `ManagedPolicy`/`ManagedEnforcement`: `remote_disabled() -> bool`, `is_pairing_allowed(&str) -> bool`, `max_paired_devices() -> Option<u32>` (+ `relay_url`/`deny_filesystem_paths` accessors) — FROZEN; Tasks 207/212/214 call these by name.
- **The fingerprint format** `is_pairing_allowed` matches against = the same device-pubkey fingerprint Task 207/206 derives — FROZEN so whitelist entries are comparable.
- **The validation-violation behavior**: an invalid field → `ManagedSettingsViolation` audit + revert-to-default; whole-file malformed → warn + full default + violation, never refuse to boot; unknown `version` → hard error (preserved from V0.1).

## Implementation notes
- **Extend, don't rewrite.** Add the new fields to the existing `ManagedFile`/`ManagedPolicy`/`Default`/`parse_managed_policy_at` rather than introducing a parallel parser. Keep the hot-reload `ManagedPolicySource` working — the new fields flow through the same `watch` channel automatically once they're on `ManagedPolicy`.
- **`null` vs absent for `allowedPairingDevices`.** Both `"allowedPairingDevices": null` and an omitted key mean "any device may pair" → `None` → `is_pairing_allowed` returns `true`. An empty array `[]` means "no device may pair" (a hard lockdown) → `Some(vec![])` → always `false`. Make this distinction explicit in the type (`Option<Vec<String>>`) and test all three.
- **Predicates are pure reads; enforcement is elsewhere.** 211 must not itself call into the relay or the pairing flow — it exposes the predicates so the owning tasks stay the single enforcement site. This keeps `disable_remote` enforcement testable in 212 and avoids a 211→212 cycle.
- **Audit shape.** `ManagedSettingsViolation` and `ManagedSettingsLoaded` are in the `AuditKind` enum (`design/12 §3.7`). Emit via the audit path Task 112 shipped (the `audit(kind, subject_ids)` shortcut / `AuditLogSubscriber` fan-out). If the parser (a free function today) lacks an audit handle in reach, thread one in at the call site that loads the policy at boot, or return a structured violation the caller audits — **decide + document**; do not silently drop the violation.
- **`disable_remote` still publishes mDNS.** When you document the `remote_disabled()` seam for 212/214, restate the `design/11 §6.4` three-part behavior so the consumer doesn't accidentally also kill mDNS — LAN-only ≠ discovery-off.
- **Cross-platform.** Pure parsing + predicates; no `std::os::unix` types. `denyFilesystemPaths` strings stay opaque here (no path canonicalization — that's the allow-list task). Keep the Windows CI lane green.

## Verification
Tier 1 — pure enforcement unit tests against constructed `managed.json` files (the existing `tempfile`-based pattern in `managed.rs`'s test module). No external double needed. It proves: whitelist allow/deny/`null`/empty-array, the max-paired cap read, `disable_remote` read, and malformed/invalid-field → `ManagedSettingsViolation` audited + default. It does **NOT** cover: the **actual** relay-registration suppression (Task 212/214), the pairing-time whitelist/cap rejection (Task 207), or audit-forwarder registration from `auditForwardEndpoint` (the deferred Task-112 `boot.rs` wiring) — those enforcement points are exercised in their owning tasks; confirming `disable_remote` truly disables remote is the **Phase-2 Tier-3 checklist** line.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core` (managed / security) → whitelist allow/deny/null/empty, max-paired cap, disable_remote read, malformed + invalid-field violation tests pass; the V0.1 `managed.rs` tests stay green.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (no new external deps expected; confirm).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → the `managed.rs` surface is internal to `crates/core` (depth-4 / no `core` `api.rs`) → expect **no** `docs/interfaces/` diff (confirm, cf. Task 112's regen note); commit if any surfaces.

## Definition of Done
- [x] Parser extended with `disable_remote` / `allowedPairingDevices` / `maxPairedDevicesPerUser` / `relayUrl` / `auditForwardEndpoint` / `denyFilesystemPaths` (exact keys, `null`/default semantics); V0.1 parse contract preserved
- [x] Predicates `remote_disabled()` / `is_pairing_allowed(fingerprint)` / `max_paired_devices()` (+ `relay_url`/`deny_filesystem_paths` accessors) exposed + FROZEN
- [x] Invalid field → `ManagedSettingsViolation` audit + revert-to-default; clean load → `ManagedSettingsLoaded`; malformed file still boots on full default
- [x] Each predicate's consumer task named (212/214 disable_remote, 207 whitelist, 207/209 max-paired, allow-list deny-paths) in Implementation notes + Handoff
- [x] Task-112 audit-forwarder question resolved: `auditForwardEndpoint` parsed + exposed here; subscriber registration explicitly deferred + noted
- [x] Tier-1 enforcement tests pass; the Tier-3 uncovered part stated in Verification
- [x] Verification commands pass; interfaces clean (or regenerated); no migration added
- [x] No `TODO`/`unimplemented!()`/`todo!()` in new code (deliberate ones in Handoff)
- [x] Single commit with the message below

## Outputs
- `crates/core/src/security/managed.rs` (modified — new fields + predicates + violation audit) + `crates/core/src/security/mod.rs` (modified only if a `ManagedEnforcement` wrapper is added)
- `crates/core/tests/managed_enforcement.rs` (new — Tier-1 enforcement tests) *(or extend `managed.rs`'s `#[cfg(test)]` module)*
- `docs/interfaces/*` (regenerated only if a surface appears)

## Commit message
```
phase-2: managed.json enforcement — disable_remote, pairing whitelist, max devices

Extends the managed.json parser (design/12 §3.8 + design/11 §6.4) with the
V1.0 security fields and provides the policy predicates the spine consumes:
remote_disabled() (Task 212/214 gate relay registration, mDNS still on),
is_pairing_allowed(fingerprint) (Task 207 pairing whitelist), and
max_paired_devices() (Task 207/209 issuance cap). Invalid fields emit
ManagedSettingsViolation and revert to default; malformed files still boot.
Parses auditForwardEndpoint (resolving Task 112's config-home question);
subscriber registration stays deferred.

Refs: tasks/v1.0/211-managed-settings-enforcement.md
```

## Handoff Notes (fill in when finishing)

**Frozen `managed.json` keys + semantics** (in `ManagedFile`, `crates/core/src/security/managed.rs`, via `#[serde(rename)]`):
- `disable_remote` (snake_case, `design/11 §6.4`) — bool, default `false`.
- `allowedPairingDevices` (camelCase) — `null`/absent → `None` (**any device may pair**); `["fp", …]` → `Some(vec)` (whitelist); `[]` → `Some(vec![])` (**hard lockdown, deny all**). The `null`-vs-`[]` distinction is load-bearing and tested.
- `maxPairedDevicesPerUser` (camelCase) — `null`/absent → `None` (unlimited); else `Some(u32)`.
- `relayUrl` / `auditForwardEndpoint` (camelCase) — `Option<String>`.
- `denyFilesystemPaths` (camelCase) — `Vec<String>`, default `[]`, opaque strings.

**Predicate surface (FROZEN, on `ManagedPolicy`)** — methods, all pure reads:
- `remote_disabled(&self) -> bool` — **Task 212/214** gate relay registration + remote-accept off this. RESTATE for the consumer: LAN-only (`design/11 §6.4`) = (1) don't register with any relay, (2) **keep publishing mDNS**, (3) accept LAN only. Do NOT also kill mDNS — LAN-only ≠ discovery-off.
- `is_pairing_allowed(&self, fingerprint: &str) -> bool` — **Task 207** pairing coordinator (pre-issuance; reject + pairing-denied audit on `false`).
- `max_paired_devices(&self) -> Option<u32>` — **Task 207/209** compare against the live active (`revoked_at IS NULL`) `devices` count at issuance.
- `relay_url(&self) -> Option<&str>` — **Task 214** relay config.
- `deny_filesystem_paths(&self) -> &[String]` — the `design/12 §3.5` allow-list policy (later); opaque here, no canonicalization.
- `audit_forward_endpoint(&self) -> Option<&str>` — see boundary below.

**Fingerprint format shared with 207/206**: the whitelist entry format is the **hex-encoded `BLAKE2b-256(device_pubkey)`** device id (`concerto_identity::device_id`) — the exact string stored in `devices.id` and used as the `EntityKind::Device` pairing audit subject (`pairing.rs` uses `hex::encode(signed.cert.device_id)`). 207's coordinator must compare `is_pairing_allowed(&hex::encode(device_id(&device_pubkey)))`. Verified by a unit test against a real derived id.

**How the violation audit handle is reached from the parser**: the free parser (`parse_managed_policy_load_at`) has no `AuditWriter` in reach, so it **collects** violations into the new `ManagedPolicyLoad { policy, violations: Vec<String> }`. The boot/reload call site calls the new `load_managed_policy_audited(config_dir, &AuditWriter) -> Result<ManagedPolicy>`, which emits one `ManagedSettingsViolation` per collected violation, then one `ManagedSettingsLoaded` summary. The V0.1 `load_managed_policy(config_dir) -> Result<ManagedPolicy>` keeps its exact signature + `tracing::warn!`-only behaviour (drops violations) for the existing permission-mode call sites that have no writer. Two new `AuditKind` variants added: `ManagedSettingsLoaded` / `ManagedSettingsViolation` (wire: `managed_settings_loaded` / `managed_settings_violation`). **NB: `load_managed_policy_audited` is provided but NOT yet wired into `boot.rs`** — whoever owns the boot config-dir load (or 212 when it first needs the policy) should call it once at startup so the audit events actually fire in production; it is exercised today only by the Tier-1 integration test. Recorded under Open questions.

**`auditForwardEndpoint` provided-not-wired boundary (resolves Task 112's deferred question)**: 211 parses + exposes `audit_forward_endpoint()` — this `managed.json` field is the config home Task 112 anticipated. **Registering** the `SyslogSubscriber`/`HttpsForwarderSubscriber` from it (the one-line `boot.rs` subscriber-`vec!` extension) is **explicitly out of scope** and left to a later audit-pipeline/ops task. 211 supplies the field only.

- **Drift from plan**: `crates/core/src/security/mod.rs` was modified (re-export `load_managed_policy_audited` + `ManagedPolicyLoad`) although the task said to touch it "only if a `ManagedEnforcement` wrapper is added." No wrapper was added — predicates live as methods directly on `ManagedPolicy` (simpler, no extra type; the task allowed "methods on `ManagedPolicy`, or a thin wrapper"). The mod.rs change is just two added re-export names. `crates/core/src/audit/event.rs` was also modified (two additive `AuditKind` variants) — this is in `design/12 §3.7`'s enum and necessary for the violation audit, but `event.rs` was not listed in `Outputs`; flagging it here.
- **Open questions for next task**: (1) `load_managed_policy_audited` needs a single boot-time call site to emit the load/violation audits in production — currently unwired (no `ManagedSettingsSource`/boot consumer exists yet); 212 (first relay consumer of `remote_disabled()`) or a boot-load task should call it. (2) 212 wires `remote_disabled()` into the transport listener — remember mDNS stays on. (3) 214 uses `relay_url()`. (4) 207 must use the hex-`device_id` fingerprint format above for `is_pairing_allowed`, and check `max_paired_devices()` against the active-`devices` count.
- **Deliberate debt**: none (no `TODO`/`unimplemented!`/`todo!` in new code).
- **Smoke-gate state**: unchanged (no `scripts/smoke.sh` change; no new capability check).
