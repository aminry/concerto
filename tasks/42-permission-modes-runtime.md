# Task 42 — Permission Modes at Runtime + Managed.json Cap

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 32, 33, 41 |
| Touches subsystem(s) | 12 (Security), 04 (Agent Supervisor) |
| Smoke gate | new check |

## Goal
Tie together everything from Tasks 32, 33, 41 so the four permission modes (`strict`/`normal`/`auto`/`yolo`) and the `bypass_destructive_guard` flag are fully enforced end-to-end. Add the `managed.json` schema validator and enforcement of `max_permission_mode` across the inheritance chain. After this task, a session in `yolo` actually auto-approves everything except destructive commands; a session in `strict` actually asks for everything; and an org policy actually prevents elevation.

## Inputs to read before starting
- `design/04_Agent_Supervisor.md` §3.10 (full table mode × tool class → decision; persistence model).
- `design/12_Security_Identity.md` §3.8 (`managed.json` schema, max_permission_mode enforcement, allow_yolo, allow_bypass_destructive_guard).
- `tasks/33-tool-approval-intercept.md` and `tasks/41-filesystem-allow-deny.md` → "Handoff Notes".

## Scope — in
- Flesh out `crates/core/src/security/managed.rs`:
  - Full schema parsing for `managed.json` covering at minimum: `max_permission_mode`, `allow_yolo`, `allow_bypass_destructive_guard`, `preamble_template_path`, `max_reasoning_level` (parsed but not enforced in V0.1).
  - Hot-reload via `notify`-rs watcher on the file; on change, broadcasts a `ConfigChanged` event to running sessions.
  - Versioning: `managed.json.version` integer; reject unknown versions with a clear error.
- Update `PermissionResolver` to:
  - Use full classification table from `design/04 §3.10` for the four modes.
  - Re-fetch the effective mode + cap from the inheritance chain on every tool call (so mid-session mode changes apply on the next turn).
  - Always honor the deny-list from Task 41 — no mode bypasses it.
- Add explicit UI-facing rejections in the gRPC layer when mode changes are blocked:
  - `Workareas.UpdateWorkareaPermissionMode(mode=yolo)` when `managed.json.allow_yolo=false` → `PERMISSION_DENIED` + `ConcertoError{code="policy.yolo_blocked"}` + audit log entry.
- Tool-approval-row writes for auto-approved cases (per Task 33's foundation): every auto-approved tool persists a row with `decision = "auto_<mode>"`. This is the "audit trail is always available" guarantee.
- Update the smoke gate v2 to add a quick mode-enforcement check (after creating workarea, set its mode to `auto`, verify a fake auto-approvable tool call doesn't raise `AwaitingApproval`). This requires the smoke client to make a `Sessions.UpdateSessionPermissionMode` call AND inject a fake tool-call event — for V0.1 this may be impractical; if so, document and skip the smoke check.
- Tests:
  - All four modes × tool classes (read/write/shell/network/MCP-trusted/MCP-untrusted) → assert correct Decision per the table.
  - Managed cap: cap to `auto`; setting yolo at any level → PERMISSION_DENIED.
  - `allow_yolo=false`: setting yolo blocked.
  - `allow_bypass_destructive_guard=false`: setting bypass blocked.
  - managed.json hot reload: change the file mid-test; verify cap takes effect within 1s.
  - Deny-list bypass: even with `yolo + bypass_destructive_guard`, the deny-list still rejects.

## Scope — out
- Concrete destructive-command intercept (Task 43).
- Full audit JSONL writer (Task 44).
- Schedule-level permission mode (V1.0 — scheduler integration).
- `max_reasoning_level` cap enforcement (V1.0 — deliberation controls are V1.0).

## Public interface this task locks
- `managed.json` schema for V0.1 fields: `version`, `max_permission_mode`, `allow_yolo`, `allow_bypass_destructive_guard`. FROZEN.
- Hot-reload behavior: file change → ConfigChanged event within 1s.
- Error codes: `policy.yolo_blocked`, `policy.bypass_blocked`, `policy.locked`. FROZEN.

## Implementation notes
- `notify-rs = "8"` (current major) — set up a recursive watch on the config dir.
- For the broadcast: a `tokio::sync::watch::Sender<ManagedSettings>`; subscribers `await sender.changed()`.
- Tool classification table lives in `crates/core/src/security/tool_classes.rs` as a const map (HashMap initialized in a `LazyLock` from a `.rs` file; reading from TOML is V1.0 polish).
- Add an entry to the smoke client for "set permission mode" so the smoke gate can exercise it.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core permission_runtime` → all mode/cap tests pass.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual: write `~/.concerto/managed.json` with `{"max_permission_mode":"auto"}`; restart Core; attempt yolo via gRPC; verify rejection; remove file; verify yolo allowed again.
5. `./scripts/regen-interfaces.sh && git diff` → committed.
6. `scripts/smoke.sh` → if smoke gate updated to include the mode check, it passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] All four modes verified end-to-end.
- [ ] Managed cap + allow_yolo + allow_bypass_destructive_guard verified.
- [ ] Hot reload verified.
- [ ] Deny-list-still-applies-in-yolo verified.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/core/src/security/managed.rs` (modified — full schema + hot reload)
- `crates/core/src/security/tool_classes.rs` (new)
- `crates/core/src/security/permission.rs` (modified — uses tool_classes table)
- `crates/core/src/agent_supervisor/approval.rs` (modified)
- `crates/core/src/handlers/workareas.rs` (modified — policy-error mapping)
- `crates/core/tests/permission_runtime.rs` (new)
- `tools/smoke-client/src/cmd/set_perm_mode.rs` (new, optional)
- `scripts/smoke.sh` (possibly modified)

## Commit message
```
phase-3: permission modes end-to-end + managed.json cap

PermissionResolver consults the full classification table and the
managed-settings cap with hot reload. Yolo and bypass_destructive
gates honor allow_yolo / allow_bypass_destructive_guard. The
deny-list (Task 41) is the hard floor — never bypassed.

Refs: tasks/42-permission-modes-runtime.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** tool classification table is const; tomled config is V1.0.
- **Smoke-gate state:** mode-enforcement check added if feasible (note in handoff).
