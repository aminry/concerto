# Task 310 — Three-Layer Project/Repository Settings Precedence Resolver

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | — |
| Touches subsystem(s) | 03 (Workspace/Session Mgr), 01 (Core Runtime), 12 (Security — `managed.json`), 09 (Persistence) |
| Smoke gate | unchanged |

## Goal
Build the **per-field settings precedence resolver** `design/03 §3.13` specifies: for any project/repo settings field, return the effective value **and** the layer it came from, walking `managed.json` > checked-in (`<project_root>/.concerto/project_settings.json` + per-repo `<repo_root>/.concerto/action_prefs.toml`) > local DB (`projects.settings_json` / the new `repositories.action_prefs_json`) > global defaults. Today only the **permission-mode** sub-chain is resolved (`crates/core/src/security/permission.rs::resolve_effective_mode`, a bespoke 4-step walk over `projects.settings_json`), and there is no reader for the **checked-in** `.concerto/project_settings.json` file at all — a team cannot ship `scripts`/`files_to_copy_rules`/`enterprise_data_privacy` in git. This task introduces a `crates/core/src/settings/` module with a generalized `ProjectSettingsResolver` that resolves every field independently, reuses the existing `ManagedPolicySource` hot-reload machinery (`crates/core/src/security/managed.rs`, `notify`-rs + 500 ms debounce + `ManagedSettings…` audit) for the checked-in files, adds migration **0011** (`repositories.action_prefs_json TEXT`) as the local-DB action-prefs layer, emits one `ProjectSettingsResolved{project_id, field, value_source}` audit per field at Core start, honors the per-machine `~/.concerto/concerto.json[project_id].opt_out_of_checked_in_fields` escape hatch, and publishes the `project_settings.json` JSON-schema artifact for editor autocomplete. It also lands the **D9(b)** one-line design amendment: `managed.json` canonicalizes on **camelCase** with serde `alias` back-compat for the already-shipped snake_case keys. After this task, every Phase-3 settings consumer (309's files-to-copy rules, 312's `action_prefs`, 321's PR prefs, 413's `enterprise_data_privacy` gate) reads its effective value — with source provenance for the read-only-lock UI — from one resolver instead of ad-hoc per-field JSON pokes.

## Inputs to read before starting
- `design/03_Workspace_Session_Manager.md` §3.13 — **the authoritative spec**: the three layers + the `managed.json > checked-in > local DB > defaults` precedence, *per-field* (a project may lock `scripts` checked-in while `files_to_copy_rules` stays local), live reload ~500 ms, the read-only lock UI + tooltip source naming, the `opt_out_of_checked_in_fields` escape hatch, the `ProjectSettingsResolved{project_id, field, value_source}` boot audit, and the **`project_settings.json` file schema** (the `jsonc` block listing `scripts`, `run_script_mode`, `enterprise_data_privacy`, `default_permission_mode`, `default_deliberation_mode`, `default_reasoning_level`, `files_to_copy_rules`, `writable_paths_outside_worktree`) + the published `https://concerto.build/schemas/project_settings.json` autocomplete artifact. Reproduce the field set faithfully.
- `design/04_Agent_Supervisor.md` §3.13 — the per-repo `action_prefs` (the seven action keys `code_review`/`pr_create`/`error_fix`/`conflict_resolve`/`branch_rename`/`commit_message`/`digest_summary`), where they live (`repositories.action_prefs_json` + a checked-in `.concerto/action_prefs.toml` override on the **same** precedence stack as §3.13), and the `compose_action_prompt(action, repo_id, base_prompt)` consumer (built by Task 312, fed by *this* resolver's `action_prefs` read). **This task owns the storage + resolution of `action_prefs`, not the prompt composition.**
- `design/12_Security_Identity.md` §3.8 — the `managed.json` V1.0 schema (the authoritative **camelCase** spelling: `enterpriseDataPrivacy`, `defaultModel`, `claudeExecutablePath`/`codexExecutablePath`/`geminiExecutablePath`, `defaultPermissionMode`, `maxPermissionMode`, …). This is the **canonical spelling D9(b) adopts**; the shipped code mixes `disable_remote` (snake) + `allowedPairingDevices` (camel) — you add `alias` back-compat, you do **not** break Task 211's parsing.
- `crates/core/src/security/managed.rs` — **REUSE, do not fork**: `ManagedPolicySource::{new,subscribe,current}` (the `notify`-rs watcher + `HOT_RELOAD_DEBOUNCE` = 500 ms debounce + the `parse_*` → `ManagedPolicyLoad{policy, violations}` structured-violation pattern + the `load_managed_policy_audited` audit-emitting entry point). The checked-in-file watcher mirrors this verbatim; the validation/violation-revert discipline is the template. Note the existing `ManagedFile` serde struct uses `#[serde(rename = "…")]` to pin spellings — D9(b) converts the shipped snake_case keys to `#[serde(rename = "<camelCase>", alias = "<snake_case>")]`.
- `crates/core/src/security/permission.rs::resolve_effective_mode` + `project_default_from_settings` + the `ModeSource` enum — the **per-field-walk pattern to generalize**, NOT duplicate. `resolve_effective_mode` already walks session→workarea→workspace→`projects.settings_json`→`managed.json` cap and reports a `ModeSource`. Your resolver covers the **project/repo layers** (managed/checked-in/local-DB/default) for the §3.13 field set; the per-entity DB override chain (session/workarea/workspace) stays in `permission.rs` and is *below* the project layer — do not absorb it. `default_permission_mode` is the one field both touch: keep `resolve_effective_mode` authoritative for the live permission decision; your resolver reports the **project-default layer** value + source for the Settings UI. Document the boundary.
- `crates/persist/src/projects.rs` (`get_settings_json`/`set_settings_json` — the local-DB project layer) + `crates/persist/src/repositories.rs` (`Repository` struct + `insert`/`get`/`list_by_project` SELECTs — you ADD `action_prefs_json` to the struct + SELECTs + the migration). Confirm the highest migration on `main` is `0008_pull_requests.sql` (it is, per `PHASE3_PLANNING §3`); 306 reserves 0009, 307 reserves 0010, so **you own 0011**. If a higher migration landed, shift per the `PHASE3_PLANNING §3` author-check rule and note it in Handoff.
- `crates/core/src/audit/event.rs` — the `AuditKind` enum + `as_str()` map; you ADD `ProjectSettingsResolved` (snake `project_settings_resolved`). Mirror the `ManagedSettingsLoaded` precedent.
- `tasks/v1.0/211-managed-settings-enforcement.md` → "Handoff Notes" — the FROZEN `managed.json` predicate names (`remote_disabled`/`is_pairing_allowed`/…) + spellings 211 shipped. **You must not regress them**; you ADD project-settings fields to the same `ManagedFile`/reader and add camelCase canonicalization with snake aliases.
- `tasks/v1.0/PHASE3_PLANNING.md` §1 D9, §2 (row 310), §3 (migration 0011), §4 — the locked decisions this task lands.

## Scope — in
- New module `crates/core/src/settings/` (e.g. `mod.rs` + `resolver.rs` + `project_file.rs`):
  - A `ProjectSettingsField` enum naming every resolvable field (the §3.13 `project_settings.json` superset: `scripts.*`, `run_script_mode`, `enterprise_data_privacy`, `default_permission_mode`, `default_deliberation_mode`, `default_reasoning_level`, `files_to_copy_rules`, `writable_paths_outside_worktree`) **plus** the per-repo `action_prefs.<action>` keys (`design/04 §3.13`).
  - A `SettingsSource` enum (`Managed | CheckedIn | LocalDb | Default`) — the per-field provenance the UI renders as the lock icon + tooltip ("Locked by `.concerto/project_settings.json`" / "Locked by org policy").
  - `ProjectSettingsResolver` with a **per-field** resolve API: `resolve_field(field) -> Resolved { value, source }` and typed convenience getters for the hot consumers (`enterprise_data_privacy() -> bool` for 413; `files_to_copy_rules() -> Vec<Rule>` for 309; `action_pref(repo_id, action) -> Option<String>` for 312/321; `scripts()`; `run_script_mode()`). Each getter walks managed → checked-in → local-DB → default and returns the value **and** its `SettingsSource`.
  - The checked-in **`project_settings.json` reader** (`<project_root>/.concerto/project_settings.json`, jsonc — strip `//` line comments) + the per-repo **`action_prefs.toml` reader** (`<repo_root>/.concerto/action_prefs.toml`). Both validate-and-revert per field (a single bad field reverts to the lower layer + records a violation), mirroring `managed.rs`'s `ManagedPolicyLoad` discipline — never refuse to resolve because one field is malformed.
  - A `ProjectSettingsSource` hot-reload watcher mirroring `ManagedPolicySource`: `notify`-rs on the `<project_root>/.concerto/` dir (non-recursive) + per-repo `.concerto/` dirs, `HOT_RELOAD_DEBOUNCE` (reuse the 500 ms const), re-resolve on save, publish via `tokio::sync::watch`.
  - The per-machine **opt-out**: read `~/.concerto/concerto.json` → `[project_id].opt_out_of_checked_in_fields: [String]`; a field listed there **skips the checked-in layer** (falls through to local-DB/default) for that project on this machine.
- Migration **0011** `crates/persist/migrations/0011_repositories_action_prefs.sql`: `ALTER TABLE repositories ADD COLUMN action_prefs_json TEXT NOT NULL DEFAULT '{}';` (plain `ADD COLUMN` — no CHECK, no table recreate). Add `action_prefs_json` to the `Repository` struct + `insert`/`get`/`list_by_project`/`list_all` SELECTs in `crates/persist/src/repositories.rs`.
- The **D9(b)** `managed.json` amendment in `crates/core/src/security/managed.rs`: convert the shipped snake_case V1.0 keys to canonical camelCase with `#[serde(alias)]` for the old spelling (`disable_remote` → keep readable; `allow_yolo`/`allow_bypass_destructive_guard`/`max_permission_mode`/`preamble_template_path`/`max_reasoning_level` → add camelCase canonical + snake alias). Add the **new** project-relevant managed fields the resolver caps on top of: `defaultPermissionMode`, `enterpriseDataPrivacy`, `defaultModel`, `claudeExecutablePath`/`codexExecutablePath`/`geminiExecutablePath` (parsed + exposed as predicates; the resolver consults them as the top layer). Keep ALL Task 211 tests green (their snake-case JSON must still parse via the alias). One-line design-amendment note in the file's module doc citing `design/12 §3.8` + `PHASE3_PLANNING D9(b)`.
- New `AuditKind::ProjectSettingsResolved` + emit one per field at Core boot (the resolver's `audit_resolved_at_boot()` called from `boot.rs`, mirroring how `load_managed_policy_audited` is called once).
- The published schema artifact `schemas/project_settings.json` (JSON Schema draft, the `$id` `https://concerto.build/schemas/project_settings.json`) — the file `project_settings.json`'s `$schema` points at, driving VS Code / JetBrains autocomplete. Ship it in-repo; do not stand up a web server.
- Tests (Tier 1): table-driven per-field precedence (each of the four layers winning for a given field while another field resolves lower); the opt-out skipping the checked-in layer; a malformed checked-in field reverting + recording a violation while siblings resolve; the `managed.json` snake→camel alias round-trip (211's snake JSON still parses); `action_prefs_json` round-trips the migration; a `notify`-driven re-resolve within the debounce window; the boot audit emits one event per field with the right `value_source`.

## Scope — out
- The `compose_action_prompt(action, repo_id, base_prompt)` helper + the `OneShotLlm` seam — **Task 312** owns `crates/core/src/llm/oneshot.rs` (`PHASE3_PLANNING §4.4`); this task only stores + resolves the `action_prefs` value 312 reads. 321 reuses 312's helper for PR title/body.
- The `.worktreeinclude` parser + files-to-copy *application* — **Task 309** owns `files_to_copy.rs`; this resolver returns the resolved `files_to_copy_rules` list (DB rules + checked-in-file precedence per §3.13) but does not copy/symlink anything. (309 reads `projects.settings_json` directly today; after 310 it should read through this resolver — note the seam.)
- The session/workarea/workspace **per-entity permission-mode override chain** — stays in `permission.rs::resolve_effective_mode` (it sits *below* the project layer). This resolver reports the project-default layer + source only.
- The Settings → Project **UI** that renders the lock icon/tooltip — Desktop tasks (322+) consume `SettingsSource`; this task ships the provenance, not the renderer.
- The Maestro `enterprise_data_privacy` *enforcement* (blanking external summaries) — **Task 413**; this resolver is the source of truth 413 reads.
- A gRPC `Settings` service — not required by Phase 3; consumers call the resolver in-process. If a later task needs RPC read access, it appends then.

## Public interface this task locks
- **Migration 0011 (FROZEN):** `repositories.action_prefs_json TEXT NOT NULL DEFAULT '{}'` — the local-DB action-prefs layer (`design/04 §3.13`). Plain `ADD COLUMN`, no CHECK.
- **Rust (FROZEN):** the `crates/core/src/settings` surface — `ProjectSettingsResolver` (the per-field `resolve_field` + the typed getters listed in Scope-in), the `SettingsSource { Managed, CheckedIn, LocalDb, Default }` enum, the `ProjectSettingsField` enum field set (the §3.13 superset + `action_prefs.<action>`), and `ProjectSettingsSource` (the watcher, mirroring `ManagedPolicySource::{new,subscribe,current}`). Consumers (309/312/321/413) depend on these names.
- **`project_settings.json` checked-in schema (FROZEN):** the field set `{ scripts{setup,setup_workarea,run,archive}, run_script_mode, enterprise_data_privacy, default_permission_mode, default_deliberation_mode, default_reasoning_level, files_to_copy_rules, writable_paths_outside_worktree }` (`design/03 §3.13`), `$schema = https://concerto.build/schemas/project_settings.json`. The published JSON-schema artifact `schemas/project_settings.json` is the autocomplete contract. New fields append-only.
- **`managed.json` canonical spelling (FROZEN, D9(b)):** **camelCase** is canonical; every shipped snake_case key gets a `#[serde(alias = "<snake>")]` so 211's files keep parsing. New project-layer managed fields: `defaultPermissionMode`, `enterpriseDataPrivacy`, `defaultModel`, `{claude,codex,gemini}ExecutablePath`.
- **`AuditKind::ProjectSettingsResolved`** (wire `project_settings_resolved`) with `{project_id, field, value_source}` details — one per field at boot.

## Implementation notes
- **Per-FIELD, not per-file, is the whole point.** Do not return a merged settings *struct* and call it done — a project can lock `scripts` checked-in while `files_to_copy_rules` is local-only, and the UI must show exactly which control is locked by which layer. Resolve each field independently and carry its `SettingsSource`. The table-driven test is the proof.
- **Reuse `ManagedPolicySource`, don't reinvent the watcher.** The checked-in-file watcher is the same shape: `notify`-rs on the parent `.concerto/` dir (so create/replace of a not-yet-existing file still fires), `std::sync::mpsc` → `spawn_blocking` recv → 500 ms debounce → re-parse → `watch::Sender::send`. Lift the pattern; share `HOT_RELOAD_DEBOUNCE`. A failed mid-write re-parse leaves the previous resolution in place (warn-only), exactly as `debounce_loop` does.
- **Validation discipline = `ManagedPolicyLoad`.** Each layer's parser collects `violations: Vec<String>` and reverts the bad field to the *next lower layer* (not to a hard default — a malformed checked-in field should fall through to local-DB, then default). Surface violations to the audit (`ProjectSettingsResolved` carries the resolved value; a sibling `ManagedSettingsViolation`-style event or a violation field documents the revert). Never refuse to resolve.
- **`notify`-rs cross-platform.** The crate is currently pinned macOS-fsevent-only (`default-features = false, features = ["macos_fsevent"]`). The Windows/Linux Core CI lanes (Task 113) need the right backend features — add `["macos_fsevent"]` on mac + the default cross-platform backend on win/linux via a `[target.'cfg(...)']` feature split, OR enable the portable default. Confirm the watcher compiles + the debounce test runs on all three lanes; note the exact feature flags in Handoff. (211 already ships this watcher on mac — match its posture and extend for win/linux.)
- **jsonc, not strict JSON.** `project_settings.json` allows `//` comments (the design block has them). Strip line comments before `serde_json::from_str`, or use a jsonc-tolerant read — keep it minimal (no new heavy dep; a line-strip is fine since the schema has no `//` inside strings in practice — but guard string literals).
- **Don't break `resolve_effective_mode`.** It stays the live permission-decision path. Your resolver's `default_permission_mode` getter reports the *project-layer* value + source for the Settings UI and for consumers that want provenance; the per-entity override walk is unchanged. Add a doc note at both call sites pointing at the other.
- **Boot wiring is one call.** Mirror `load_managed_policy_audited`: at Core start, after the resolver is built per project, call `audit_resolved_at_boot(audit_writer)` which emits one `ProjectSettingsResolved` per (project, field). Synchronous-ish; the files are tiny.
- Regen: new migration + new `AuditKind` ⇒ `./scripts/regen-interfaces.sh` updates `docs/interfaces/schema.md` + `rust-api.md`; commit them. (No proto change in this task.)

## Verification
Tier 1. (The deterministic resolver + table-driven precedence is fully CI-provable; live-reload is testable via `notify` exactly as `managed.rs`'s watcher is.)
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core settings` → per-field precedence table (each layer wins for some field while another resolves lower), opt-out skips checked-in, malformed-field-reverts-to-lower-layer + violation, `notify` re-resolve within debounce, boot-audit one-event-per-field.
4. `cargo test -p concerto-core managed` + `cargo test -p concerto-persist repositories` → 211's snake-case `managed.json` tests still pass via the new aliases; `action_prefs_json` migration + struct round-trips.
5. `cargo test --workspace --no-fail-fast` → all pass.
6. `cargo deny check` → green (no new heavy dep; the jsonc line-strip is hand-rolled, `notify` already pinned).
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`schema.md` gains `repositories.action_prefs_json`; `rust-api.md` gains the `settings` module + `ProjectSettingsResolved`).
8. `scripts/smoke.sh` → **unchanged** (this task adds no capability; the co-located happy path must stay green since boot now also resolves project settings — confirm no boot regression).

**Tier-1 scope note (for the phase checklist):** Tier-1 covers the deterministic resolution + live-reload-via-`notify` + the migration. What it does NOT cover and is a **Phase-3 Tier-3 checklist line**: confirming the published `project_settings.json` schema actually drives autocomplete in a real VS Code / JetBrains install (editor integration is external).

## Definition of Done
- [ ] `crates/core/src/settings/` ships `ProjectSettingsResolver` with per-field `resolve_field` + typed getters + `SettingsSource` provenance, reusing the `ManagedPolicySource` watcher pattern
- [ ] Checked-in `project_settings.json` (jsonc) + per-repo `action_prefs.toml` readers with per-field validate-and-revert-to-lower-layer
- [ ] Migration 0011 adds `repositories.action_prefs_json`; the `Repository` struct + all SELECTs read it
- [ ] `managed.json` canonicalized on camelCase with snake-case `serde(alias)` back-compat; all Task 211 tests green; new project-layer managed fields parsed
- [ ] Per-machine `opt_out_of_checked_in_fields` skips the checked-in layer for the named fields
- [ ] `AuditKind::ProjectSettingsResolved` emits one event per field at boot
- [ ] `schemas/project_settings.json` JSON-schema artifact published in-repo
- [ ] `notify`-rs feature flags compile + watcher runs on the win/linux CI lanes (Task 113)
- [ ] Verification commands pass; interfaces regenerated; smoke gate unchanged + still green
- [ ] No TODO/FIXME/unimplemented!()/todo!() in new code (deliberate seams in Handoff)
- [ ] Single commit with the message below

## Outputs
- `crates/core/src/settings/mod.rs` (new)
- `crates/core/src/settings/resolver.rs` (new — `ProjectSettingsResolver` + `SettingsSource` + `ProjectSettingsField`)
- `crates/core/src/settings/project_file.rs` (new — jsonc `project_settings.json` + `action_prefs.toml` readers + the `ProjectSettingsSource` watcher)
- `crates/core/src/lib.rs` (modified — `pub mod settings`)
- `crates/core/src/security/managed.rs` (modified — camelCase canonicalization + snake aliases + new project-layer fields + module-doc amendment note)
- `crates/core/src/audit/event.rs` (modified — `ProjectSettingsResolved` variant + `as_str` arm)
- `crates/core/src/boot.rs` (modified — build the resolver per project + emit the boot audit)
- `crates/persist/migrations/0011_repositories_action_prefs.sql` (new)
- `crates/persist/src/repositories.rs` (modified — `action_prefs_json` on struct + SELECTs/insert)
- `schemas/project_settings.json` (new — published JSON-schema artifact)
- `docs/interfaces/schema.md` + `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-3: three-layer project settings precedence resolver

Per-field ProjectSettingsResolver (managed > checked-in > local DB >
defaults) with SettingsSource provenance, reusing the managed.json
notify-rs hot-reload watcher. Adds migration 0011
(repositories.action_prefs_json), the checked-in project_settings.json +
action_prefs.toml readers, the ProjectSettingsResolved boot audit, the
opt_out_of_checked_in_fields escape hatch, the published JSON-schema
artifact, and the D9(b) managed.json camelCase canonicalization with
snake-case serde aliases (211 stays green).

Refs: tasks/v1.0/310-settings-precedence-resolver.md
```

## Handoff Notes (filled in when finishing)
- Drift from plan — —
- Open questions for next task — —
- Deliberate debt — —
- Smoke-gate state — —
