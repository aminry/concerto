# Task 32 — Permission-Mode Inheritance Resolver

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 19, 20, 22 |
| Touches subsystem(s) | 03 (Workspace Manager), 04 (Agent Supervisor), 09 (Persistence) |
| Smoke gate | unchanged |

## Goal
Implement the permission-mode inheritance chain from `design/03 §3.8` and `design/04 §3.10`: a session's effective mode = first non-NULL value walking `sessions → workareas → workspaces → projects → managed.json → global default ("normal")`. The same chain for `bypass_destructive_guard`. After this task, the `PermissionResolver` (from `design/04 §3.10`) can be constructed correctly per session; enforcement of decisions happens in Tasks 33 (tool-approval intercept) and 41/42/43 (Security subsystem tasks).

## Inputs to read before starting
- `design/03_Workspace_Session_Manager.md` §3.8 (permission mode hierarchy), §3.13 (project settings precedence — same chain shape).
- `design/04_Agent_Supervisor.md` §3.10 (full permission-mode taxonomy: strict / normal / auto / yolo + bypass_destructive_guard; override precedence; persistence model; entry-ceremony strings).
- `design/12_Security_Identity.md` §3.8 (skim — `managed.json.maxPermissionMode` cap).

## Scope — in
- Create `crates/core/src/security/permission.rs`:
  - `pub enum PermissionMode { Strict, Normal, Auto, Yolo }` (plus `From<i32>` / proto enum conversion).
  - `pub struct EffectiveMode { pub mode: PermissionMode, pub bypass_destructive_guard: bool, pub source: ModeSource }`.
  - `pub enum ModeSource { Session, Workarea, Workspace, Project, Managed, Default }`.
  - `pub async fn resolve_effective_mode(db: &Reader, session_id: SessionId) -> Result<EffectiveMode>` walking the chain.
- Read `managed.json` (V0.1 minimal — just look up the file if it exists at `<config_dir>/managed.json`; parse `max_permission_mode` and `allow_yolo` / `allow_bypass_destructive_guard` fields; absence is fine).
- Add RPCs to update mode at each level:
  - `Workspaces.UpdateWorkspaceSettings(WorkspaceId, settings)` — `permission_mode` field.
  - `Workareas.UpdateWorkareaPermissionMode(WorkareaId, mode)` + `SetWorkareaBypassDestructiveGuard(WorkareaId, bool)`.
  - `Sessions.UpdateSessionPermissionMode(SessionId, mode)` — V0.1 keeps this optional.
- Enforcement entry ceremony at the RPC layer:
  - Setting `auto`: no ceremony.
  - Setting `yolo`: require an `acknowledgement` field literally equal to `"I understand"`.
  - Setting `bypass_destructive_guard = true`: require `acknowledgement = "I understand the risks"`.
  - Server rejects with `FAILED_PRECONDITION` if the ceremony string is wrong.
- Cap enforcement: if `managed.json.max_permission_mode = "auto"`, an attempt to set `yolo` returns `PERMISSION_DENIED` + `ConcertoError{code="policy.locked"}`.
- Audit emission (via `tracing::info!` for now — the real JSONL writer comes in Task 44): every mode change records `(workarea_id, from, to, source, by_device_id, acknowledgement_provided)`.
- Tests:
  - Inheritance chain: insert mode at each level, assert the resolver returns the right effective mode + source.
  - Ceremony strings: wrong string is rejected; right string accepted.
  - Managed cap: cap to `auto`, attempt `yolo`, expect denial.
  - On workarea restore (Task 31), confirm `permission_mode` is NULL again (inherits from workspace).

## Scope — out
- Filesystem allow/deny enforcement (Task 41).
- Tool-approval intercept in the supervisor (Task 33).
- Destructive command intercept (Task 43).
- `tool_approvals` row writes for auto-approved tools (Task 33).
- `managed.json` full schema parsing (V0.1 only reads the three fields above).

## Public interface this task locks
- Rust: `crates/core/src/security/permission.rs` types: `PermissionMode`, `EffectiveMode`, `ModeSource`, `resolve_effective_mode`.
- Acknowledgement strings frozen: `"I understand"` and `"I understand the risks"`.
- `managed.json` field names: `max_permission_mode`, `allow_yolo`, `allow_bypass_destructive_guard`.
- Source-of-record fields: `workareas.permission_mode`, `workspaces.permission_mode`, `workareas.bypass_destructive_guard`, etc.

## Implementation notes
- The walk is best done in a single SQL `SELECT` with `COALESCE(session.mode, workarea.mode, workspace.mode, project.default_mode)` joined across tables. Then apply managed cap in Rust.
- `managed.json` parsing: simple `serde_json::from_str`; fail gracefully on missing file or malformed JSON (warn + default).
- Update Task 22's `start_session` to call `resolve_effective_mode` and stash the result on the `Session` struct.
- For the test fixture, helper functions in `crates/test-harness` to insert rows at each level with known modes.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core permission` → inheritance + ceremony + cap tests pass.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual: with Core running, set `managed.json` to `{"max_permission_mode": "auto"}`; restart Core; attempt to set workarea to yolo via gRPC; verify rejection.
5. Manual: set workspace mode `auto`; create new workarea; query effective mode for a session in it; expect `auto` with source `Workspace`.
6. `./scripts/regen-interfaces.sh && git diff` → committed.
7. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] Inheritance chain verified at every level (table-driven).
- [ ] Entry-ceremony strings enforced.
- [ ] Managed-cap enforced.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/core/src/security/mod.rs` (new — module declaration)
- `crates/core/src/security/permission.rs` (new)
- `crates/core/src/security/managed.rs` (new)
- `crates/persist/src/workspaces.rs`, `workareas.rs`, `sessions.rs`, `projects.rs` (modified — mode getters/setters)
- `crates/proto/proto/concerto/v1/workspaces.proto`, `workareas.proto`, `sessions.proto` (modified)
- `crates/core/src/handlers/workspaces.rs`, `workareas.rs`, `sessions.rs` (modified)
- `crates/core/src/agent_supervisor/spawn.rs` (modified — uses resolver)
- `crates/core/tests/permission_inheritance.rs` (new)
- `docs/interfaces/proto.md`, `rust-api.md` (regenerated)

## Commit message
```
phase-3: permission-mode inheritance + entry ceremony

resolve_effective_mode walks session→workarea→workspace→project→
managed→default per design/04 §3.10. Entry ceremony enforced for
yolo and bypass_destructive_guard. managed.json cap denies elevated
modes when policy locks them.

Refs: tasks/32-permission-mode-inheritance.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** audit writes use tracing; structured JSONL audit log is Task 44.
- **Smoke-gate state:** unchanged.
