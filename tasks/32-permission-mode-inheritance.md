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
- [x] Verification commands pass.
- [x] Inheritance chain verified at every level (table-driven).
- [x] Entry-ceremony strings enforced.
- [x] Managed-cap enforced.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

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
- **Drift from plan:**
  - **New error variants `Error::Policy` and `Error::PolicyLocked`** added to `crates/error/src/api.rs`. Wire codes are `"policy"` (→ `Code::FailedPrecondition`) and `"policy.locked"` (→ `Code::PermissionDenied`) per `design/12 §3.8`. Mapping wired in `crates/core/src/error_map.rs`. Both variants are additive to the Task 19 surface; existing tests untouched.
  - **`WorkspaceManager::new` / `WorkareaManager::new` / `AgentSupervisorHandle::new` all gained an `Arc<PathBuf>` `config_dir` parameter** so each handler can call `load_managed_policy(&config_dir)` at RPC time. The actor wrappers gained matching constructor signatures; `main.rs` now plumbs a single `Arc::new(config.config_dir.clone())` through every spawn site. Task 17's integration tests (`agent_spawn.rs`) updated.
  - **`UpdateWorkareaPermissionModeRequest.permission_mode = PERMISSION_MODE_UNSPECIFIED` clears the override** (inherit-from-workspace). The proto wire is a non-optional enum (vs the optional one on `Workspace.permission_mode`); `UNSPECIFIED` is the agreed "clear" sentinel for the update RPC. `Workspaces.UpdateWorkspaceSettings` uses the `WorkspaceSettings` wrapper message instead (V0.1 fields: `optional PermissionMode permission_mode = 1`), so omitting the field is "no change" and `Some(UNSPECIFIED)` is rejected as `INVALID_ARGUMENT`.
  - **`SessionEntry` gained a `permission_mode: PermissionMode` field** so the Agent Supervisor caches the effective mode at `start_session` and updates it on `update_session_permission_mode`. Task 33's tool-approval intercept reads this field instead of round-tripping through the DB. The DB row is still the source of truth.
  - **`AgentSupervisor::start_session` resolves the effective mode in Rust BEFORE inserting the session row.** When `req.permission_mode` is `None`, the supervisor calls a private `resolve_for_new_session` helper that walks workarea → workspace → project → managed → default (no session row exists yet). The session row is then inserted with the resolved mode as the value — so the row always carries the effective mode from row 1, and the inheritance is observable to anyone reading `sessions.permission_mode`.
  - **`projects::set_settings_json` + `projects::get_settings_json` added** to the persistence surface so future tasks can patch per-project settings without inventing a new helper. Mirrors `workareas::set_settings_json` (Task 30).
  - **Workareas RPCs frozen at the V0.1 set + the two new Task 32 RPCs** (`UpdateWorkareaPermissionMode`, `SetWorkareaBypassDestructiveGuard`). Adding the workspace-level `UpdateWorkspaceSettings` was done with a wrapper message so V1.0 can grow workspace settings without renaming.
- **Open questions for next task:**
  - **Task 33 (tool-approval intercept)** should read the cached `SessionEntry.permission_mode` on each tool call rather than re-resolving — the resolver is correct but the cache eliminates a DB round trip on the hot path. Refresh-on-write semantics live in `update_session_permission_mode` already.
  - **Task 41/42/43 (filesystem allow/deny + destructive intercept)** should call `crate::security::resolve_effective_mode` with `(&persistence, &config_dir, &session_id)` directly. The resolver is the single canonical place for the chain walk + managed cap; everything else should defer to it.
  - **Task 44 (audit JSONL writer)** can grep for `audit.kind = "permission_mode_changed"` / `audit.kind = "bypass_destructive_guard_changed"` `tracing::info!` events as the structured event source. The field set today: `audit.kind`, `audit.scope` (workspace|workarea|session), `audit.workspace_id`/`audit.workarea_id`/`audit.session_id`, `audit.from`, `audit.to`, `audit.acknowledgement_provided`. The acknowledgement string itself is NOT logged (per pre-decision 9 — non-sensitive but not file-appender material in V0.1).
  - **`Sessions.UpdateSessionPermissionMode` does NOT accept UNSPECIFIED.** `sessions.permission_mode` is `NOT NULL` in the schema (the row always has a concrete value); the wire enum rejects `UNSPECIFIED` with `INVALID_ARGUMENT`. The workarea-level RPC accepts `UNSPECIFIED` because workareas inherit.
- **Deliberate debt:**
  - **Audit writes use `tracing::info!`** with structured fields; the JSONL audit-log writer lands in Task 44. The redaction filter (Task 16) already strips known-secret field names; the acknowledgement strings are NOT secret but we kept them out of the log payload per pre-decision 9.
  - **`managed.json` is read synchronously every RPC call.** The file is tiny and the RPC is rare; an in-process cache invalidated on `SIGHUP` is a Phase 3 micro-optimisation if profiling demands it.
  - **`PermissionMode::Strict` is the lowest rank for the cap walk**, but no caller actually requests `strict` via the public RPCs yet (Phase 3). The ordering is locked here for forward use.
  - **No `TODO`/`FIXME`/`todo!()`/`unimplemented!()` markers in new code.**
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` (v2) still boots Core → creates project/repo/workspace/workarea → spawns an echo session → asserts output → archives — the Task 32 RPCs are exercised by `crates/core/tests/permission_inheritance.rs` via the Task 17 harness, not by the smoke gate.
