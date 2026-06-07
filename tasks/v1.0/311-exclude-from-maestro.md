# Task 311 — `exclude_from_maestro` Per-Workarea Toggle (schema key + typed proto + API)

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 1 |
| Size | small (≤4h) |
| Depends on | 307 |
| Touches subsystem(s) | 03 (Workspace/Session Mgr), 08 (Maestro — consumer, not built here) |
| Smoke gate | unchanged |

## Goal
Give each workarea a user-controllable **`exclude_from_maestro`** privacy toggle so a sensitive workarea (e.g. a security-incident investigation in the `mozart` workarea) can be kept out of the Concerto chat's summaries while its siblings participate normally. Today there is no API to set or read this flag: `design/03 §3.14` reserves `workareas.settings_json.exclude_from_maestro` as its home (the `settings_json` column exists since migration `0002` and currently holds only `{"files_to_copy_applied": true}`), and `design/08 §3.3` says Maestro reads it to blank summaries — but nothing writes it. This task adds `WorkareaManager::set_exclude_from_maestro(id, bool)` (a **read-modify-write** of `settings_json` that preserves existing keys), surfaces the flag as a **typed `bool` field on the `Workarea` proto message** (next free field number **11**) derived from `settings_json` in `workarea_to_proto`, and adds a `SetWorkareaExcludeFromMaestro` RPC on the `Workareas` service returning the updated `Workarea`. This is deliberately the *derived-settings-key* precedent (`PHASE3_PLANNING §2`, row 311): a JSON key stored in `settings_json` but projected as a typed proto bool for clients. The actual privacy *enforcement* (blanking summaries, showing `[private workarea, name only]`) is Maestro Task 413 in Phase 4 — this task ships only the storage + the toggle + the typed read.

## Inputs to read before starting
- `design/03_Workspace_Session_Manager.md` §3.14 — the authoritative spec: the toggle lives **on the workarea, not the workspace** (one workspace can hold both sensitive and normal workareas), stored in `workareas.settings_json.exclude_from_maestro`. Reproduce that storage location exactly.
- `design/08_Maestro_Agent.md` §3.3 — the **consumer** contract (Phase 4): when `workareas.settings_json.exclude_from_maestro = true`, Maestro exposes only the hard facts (status, branch, repo names) and blanks summaries; the workarea shows as `[private workarea, name only]`. Read it to get the semantics right, but **do not implement any Maestro behavior here** (no Maestro crate exists until P4).
- `crates/persist/src/workareas.rs` — `set_settings_json` (currently **OVERWRITES the whole blob** — line ~398; you must NOT call it with a clobbering payload), `get`/`row_to_workarea` (the `Workarea` struct already carries `settings_json: String`). You add a read-modify-write helper (e.g. `set_settings_json_key(conn, id, key, value)` or merge in the manager) that preserves `files_to_copy_applied` and any future keys.
- `crates/persist/src/api.rs` (or wherever `Workarea` is defined) — the `Workarea` struct with `settings_json: String`; no schema change needed (the column exists since `0002`).
- `crates/proto/proto/concerto/v1/workareas.proto` — the `Workarea` message (fields 1–10 used; **11 is the next free field**) and the `Workareas` service. **Field numbers in this file are FROZEN as of Task 20**; you APPEND `optional bool exclude_from_maestro = 11;` and APPEND the new RPC — never renumber 1–10 or the existing RPCs. Note Task 307 (a dependency) widens the `status` CHECK + updates the `status` proto *comment* but does **not** add a `Workarea` field, so 11 is yours.
- `crates/core/src/handlers/workareas.rs` — `workarea_to_proto` (line ~255 — where you derive the proto `exclude_from_maestro` bool from `settings_json`) and the RPC-impl pattern (`update_workarea_permission_mode` at line ~182 is the closest template: validate → call the manager → return the updated `Workarea` proto).
- `crates/core/src/workspace_manager/workarea.rs` — the `WorkareaManager` handle (holds `persistence`, broadcasts `WorkareaEvent`); `update_workarea_permission_mode` (line ~610) is the method shape to mirror for `set_exclude_from_maestro`.
- `tasks/v1.0/307-parallel-workareas-fsm.md` → "Handoff Notes" — 307 is the dependency (the full workarea FSM + migration 0010 widening the status CHECK). Confirm the `Workarea` proto/struct shape 307 left (it may have touched the status comment); your field 11 append sits on top of that.
- `tasks/v1.0/PHASE3_PLANNING.md` §2 (row 311: "Typed proto `bool` on the `Workarea` message (next free field number), derived from `workareas.settings_json.exclude_from_maestro`. Sets the precedent for future derived settings keys.") + §3 (311 = `settings_json` JSON key, **no migration**).

## Scope — in
- `WorkareaManager::set_exclude_from_maestro(&self, id: &WorkareaId, exclude: bool) -> Result<Workarea>`:
  - Load the workarea; **read-modify-write** `settings_json`: parse the existing JSON object, set/overwrite the `exclude_from_maestro` key, re-serialize, persist — **preserving all other keys** (`files_to_copy_applied`, etc.). Reject a non-object `settings_json` defensively (treat malformed/empty as `{}`).
  - Return the updated `Workarea` row.
  - (Optional, mirror siblings) broadcast a `WorkareaEvent` if the existing event surface warrants it — but the flag is read on demand by Maestro, so a dedicated event is not required. Decide in-task; if added, keep it append-only on `WorkareaEvent`.
- A persist helper that does the JSON-key merge without clobbering (e.g. `workareas::set_settings_json_key` or a read-modify-write in the manager calling the existing `get` + `set_settings_json`). Whichever you pick, the contract is: **never overwrite sibling keys**.
- Proto: append `optional bool exclude_from_maestro = 11;` to `Workarea` and `message SetWorkareaExcludeFromMaestroRequest { string workarea_id = 1; bool exclude = 2; }` + `rpc SetWorkareaExcludeFromMaestro(SetWorkareaExcludeFromMaestroRequest) returns (Workarea);` on the `Workareas` service.
- `workarea_to_proto`: derive `exclude_from_maestro` by parsing `settings_json` (absent/false/malformed ⇒ `false`/`Some(false)`); set it on every `Workarea` the service returns (Get/List/Create/the new RPC) so clients always see the current value.
- The handler impl `set_workarea_exclude_from_maestro` (validate the id → call the manager → return the updated proto), registered like the other `Workareas` RPCs.
- Tests (Tier 1): set true then read back via `get_workarea`/`workarea_to_proto`; **the read-modify-write preserves `files_to_copy_applied`** (set the flag on a workarea whose `settings_json` already has it; assert both keys present); set false clears it; a workarea with empty/`{}`/malformed `settings_json` toggles cleanly; the proto field is populated on Get/List.

## Scope — out
- **All Maestro-side enforcement** (blanking summaries, hard-facts-only exposure, the `[private workarea, name only]` rendering) — **Task 413** (`design/08 §3.3`), Phase 4. This task ships the toggle + storage + typed read only.
- A migration — **none**; `settings_json` exists since `0002` and this is a JSON key (`PHASE3_PLANNING §3`).
- Workspace-level or session-level exclude — the flag is **per-workarea** by design (`§3.14`); do not add it anywhere else.
- The Desktop toggle UI — Desktop tasks (322+) consume the proto field; this task ships the field, not the control.
- Any change to `set_settings_json`'s existing whole-blob-overwrite callers (e.g. the files-to-copy path) — leave them; add the non-clobbering helper alongside.

## Public interface this task locks
- **Proto (FROZEN):** `Workarea.exclude_from_maestro = 11` (`optional bool`) — the derived-settings-key projection of `workareas.settings_json.exclude_from_maestro`. `SetWorkareaExcludeFromMaestroRequest { workarea_id = 1; exclude = 2; }` + `Workareas.SetWorkareaExcludeFromMaestro` RPC. Field numbers FROZEN; appended after Task 20's frozen 1–10 + the existing RPCs.
- **Storage (FROZEN by `design/03 §3.14`, re-stated):** the flag lives in `workareas.settings_json.exclude_from_maestro` (a JSON bool); the proto field is a **derived projection**, not a separate column. Writes are read-modify-write and preserve sibling keys.
- **Rust (FROZEN):** `WorkareaManager::set_exclude_from_maestro(id, bool) -> Workarea`.

## Implementation notes
- **Read-modify-write is the one load-bearing detail.** `set_settings_json` today overwrites the entire blob — if you call it naively you wipe `files_to_copy_applied`. Parse `settings_json` into a `serde_json::Value::Object`, mutate the one key, re-serialize. A malformed/empty existing blob ⇒ start from `{}`. The test that sets the flag on a workarea already carrying `files_to_copy_applied` and asserts both survive is the proof — make it explicit.
- **Derived proto field, not a new column.** This is the precedent for future settings projected to typed proto fields. Keep the *source of truth* in `settings_json`; `workarea_to_proto` is the only place the JSON→bool projection happens, so every `Workarea` the service emits is consistent. Use `optional bool` (proto3 `optional`) so a future "unknown" state is representable, matching the `permission_mode` optionality already on the message.
- **Cross-platform / tiny.** Pure persist + proto + handler; no FS, no OS calls. Builds on every CI lane (Task 113) with no special handling.
- **Don't pre-build Maestro hooks.** It is tempting to add a `WorkareaContext.exclude_from_maestro` accessor for 413 — leave that to 413; this task's surface is the toggle + the proto read. Note the consumer in Handoff.
- Regen: proto change ⇒ `./scripts/regen-interfaces.sh` updates `docs/interfaces/proto.md`; commit it.

## Verification
Tier 1. (Pure schema-passthrough + API; the privacy *behavior* is verified in Maestro Phase 4 — see scope note.)
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core exclude_from_maestro` (or the workareas-handler test module) → set/read round-trip; **sibling-key preservation** (`files_to_copy_applied` survives); clear; malformed/empty `settings_json` toggles cleanly; proto field populated on Get/List.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `cargo deny check` → green (no new deps).
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (`proto.md` gains `Workarea.exclude_from_maestro` + the new RPC).
7. `scripts/smoke.sh` → **unchanged** (no new capability; co-located happy path stays green).

**Tier-1 scope note (for the phase checklist):** Tier-1 covers the toggle + storage + typed read. What it does NOT cover is the actual **privacy enforcement** — that an excluded workarea leaks only hard facts to the Concerto chat — which is **Maestro Task 413**, verified at the **Phase-4** manual checklist ("confirm an excluded workarea leaks only hard facts"). No Phase-3 Tier-3 line is needed here.

## Definition of Done
- [x] `WorkareaManager::set_exclude_from_maestro(id, bool) -> Workarea` with **non-clobbering** read-modify-write of `settings_json`
- [x] `Workarea.exclude_from_maestro = 11` (`optional bool`) appended + derived in `workarea_to_proto` on every returned `Workarea`
- [x] `SetWorkareaExcludeFromMaestro` RPC + request message appended to the `Workareas` service; handler wired
- [x] No migration (JSON key in existing `settings_json` column)
- [x] Tests cover set/read, sibling-key preservation, clear, malformed-blob, proto-field-on-Get/List
- [x] Verification commands pass; interfaces regenerated; smoke gate unchanged + green
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (Maestro consumer seam noted in Handoff)
- [x] Single commit with the message below

## Outputs
- `crates/proto/proto/concerto/v1/workareas.proto` (modified — `Workarea.exclude_from_maestro = 11` + `SetWorkareaExcludeFromMaestroRequest` + the RPC)
- `crates/core/src/workspace_manager/workarea.rs` (modified — `set_exclude_from_maestro`)
- `crates/persist/src/workareas.rs` (modified — non-clobbering `settings_json` key-merge helper)
- `crates/core/src/handlers/workareas.rs` (modified — `set_workarea_exclude_from_maestro` handler + derive in `workarea_to_proto`)
- `crates/core/tests/workareas_exclude_maestro.rs` (new — or extend an existing workareas test)
- `docs/interfaces/proto.md` (regenerated)

## Commit message
```
phase-3: exclude_from_maestro per-workarea toggle

Adds WorkareaManager::set_exclude_from_maestro (non-clobbering
read-modify-write of workareas.settings_json) + the derived
Workarea.exclude_from_maestro proto bool (field 11) + the
SetWorkareaExcludeFromMaestro RPC. Storage stays in settings_json
(design/03 §3.14); Maestro enforcement is Task 413 (Phase 4). No
migration — JSON key on the existing column.

Refs: tasks/v1.0/311-exclude-from-maestro.md
```

## Handoff Notes (filled in when finishing)
- Drift from plan — None. Built exactly to `Scope — in`: persist `workareas::set_settings_json_key(conn, id, key, serde_json::Value)` (the non-clobbering read-modify-write helper, sibling to `set_settings_json`), `WorkareaManager::set_exclude_from_maestro(id, bool) -> Workarea` (load → key-merge in a txn → reload), proto `Workarea.exclude_from_maestro = 11` (`optional bool`) + `SetWorkareaExcludeFromMaestroRequest {workarea_id=1; exclude=2}` + the `SetWorkareaExcludeFromMaestro` RPC, derived in `workarea_to_proto` via the new `pub(crate) derive_exclude_from_maestro(&str) -> bool`. No `WorkareaEvent` variant added (the plan made it optional and the flag is read on demand by Maestro). Field numbers 1–10 + existing RPCs untouched; only `docs/interfaces/proto.md` regenerated (no schema/rust-api delta → confirms no migration).
- Open questions for next task — **P4 Maestro Task 413** is the consumer: it reads `exclude_from_maestro` (typed proto bool, or `workareas.settings_json.exclude_from_maestro` directly) to expose only hard facts (status/branch/repo) and blank summaries, rendering `[private workarea, name only]` (`design/08 §3.3`). This task ships only the toggle + storage + typed read — no Maestro behavior, no `WorkareaContext.exclude_from_maestro` accessor (left to 413 per the task's "don't pre-build Maestro hooks"). Desktop 322+ consumes the proto field for the toggle UI.
- Deliberate debt — None. The derive defaults absent/`false`/non-bool/malformed/non-object `settings_json` → `false` (workarea visible unless explicitly excluded); the persist merge discards a malformed/non-object existing blob as `{}` (defensive, per Scope — in). The projection lives in exactly one place (`derive_exclude_from_maestro`) so every emitted `Workarea` is consistent. `set_settings_json_key` is the reusable precedent for future derived settings keys (`PHASE3_PLANNING §2`).
- Smoke-gate state — Unchanged + green. No new capability, no `scripts/smoke.sh` edit; pure persist + proto + handler. Full `rust` gate clean: `cargo check --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --no-fail-fast` (all pass, incl. the 6 new `workareas_exclude_maestro` tests), `cargo deny check` (advisories/bans/licenses/sources ok — no new deps), `cargo fmt --all -- --check`, and `regen-interfaces.sh` + `git diff --exit-code docs/interfaces/` (only `proto.md`, committed).
