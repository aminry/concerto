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
- [x] Verification commands pass.
- [x] All four modes verified end-to-end.
- [x] Managed cap + allow_yolo + allow_bypass_destructive_guard verified.
- [x] Hot reload verified.
- [x] Deny-list-still-applies-in-yolo verified.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

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
- **Drift from plan:**
  - **`load_managed_policy` now returns `Result<ManagedPolicy>` instead of
    `ManagedPolicy`.** The schema-version tripwire (pre-decision 11) needs
    a typed error path: a `version > 1` file is `Error::Internal` so the
    operator notices forward-compat mismatch. RPC-handler callers in
    `workspace_manager/actor.rs`, `workspace_manager/workarea.rs`, and
    `agent_supervisor/actor.rs::update_session_permission_mode` propagate
    via `?` — those return `Result` already. The two resolver-time
    callers (`security::permission::resolve_effective_mode` and the
    supervisor's `resolve_for_new_session` helper) use
    `.unwrap_or_default()` so a broken org artifact degrades to
    permissive for session resolution; the RPC entry points are where
    the loud failure surfaces.
  - **Subcode wire strings are embedded in `Error::PolicyLocked` message
    bodies, not promoted to dedicated error variants.** The wire code
    returned by `Error::wire_code()` is still `policy.locked` (Task 32
    contract); the message body carries
    `policy.yolo_blocked` / `policy.bypass_blocked` / `policy.locked`
    (the generic) as a prefix. `ConcertoError.code` over the wire stays
    `policy.locked`; clients switch on the message prefix. This mirrors
    Task 19's `validation` + embedded `workspace.v0_single_repo_only`
    pattern, so we did not add new wire codes for this task.
  - **`ManagedPolicySource` watches the parent directory non-recursively**
    instead of the file itself. `notify`-rs cannot deliver create events
    for a not-yet-existing file (the file path is not registered yet),
    so a watcher rooted at `<config_dir>` survives a future
    `managed.json` materialization. The debounce loop coalesces the
    typical write+rename event burst (500 ms window) into one
    re-parse.
  - **`notify = "8"` (CC0-1.0) added to `deny.toml`.** CC0-1.0 is a
    public-domain dedication — FSF-approved, functionally permissive,
    no copyleft and no attribution requirement. The allowlist entry
    documents the justification. `notify` 9.x is RC at this writing;
    pre-decision 5 explicitly said "use latest stable", so we pinned at
    8.2.0. Default features set to `["macos_fsevent"]` only — V0.1 ships
    macOS-first and we do not want the implicit kqueue / inotify
    backends pulling extra deps on platforms we are not targeting yet.
  - **`PermissionResolver::classify` now consults `tool_classes::TOOL_CLASSES`
    (a `LazyLock<HashMap>`) rather than the inline `match` Task 33 shipped.**
    The canonical tool names are now the Claude Code built-ins
    (`Read`, `Glob`, `Grep`, `Write`, `Edit`, `NotebookEdit`, `Bash`,
    `Delete`). Unknown tools default to `Restricted` (conservative
    posture per `tool_classes` module docs) — flipped from Task 33's
    `Safe` default. The tool-approval integration tests
    (`crates/core/tests/tool_approval.rs`) were updated to use the new
    canonical names; the existing parser fixture still emits lowercase
    `"edit"`, which is independent of the classify table and remains
    untouched.
  - **`ManagedPolicy` gained `version`, `preamble_template_path`, and
    `max_reasoning_level` fields.** All struct-update-syntax callers
    (`..ManagedPolicy::default()`) keep working; the two new fields are
    parsed but not enforced in V0.1, locking the schema surface ahead
    of Tasks 44 (audit) and the V1.0 deliberation work.
- **Open questions for next task:**
  - **Task 43 (destructive-command intercept)** should slot into the
    supervisor's `dispatch_parse_event` between the resolver's
    `decide()` and the `policy_override()` call. The classification
    table's `Bash` entry stays `Restricted`; Task 43's pattern matcher
    promotes specific command lines (e.g. `rm -rf`, `git push --force`)
    to `Dangerous` so `auto` mode asks for them. The promoted-class
    decision string would naturally fold into the existing
    `auto_<mode>` / `denied_by_policy` row strings.
  - **Task 44 (audit JSONL writer)** should grep for the embedded
    subcode prefixes (`policy.yolo_blocked`, `policy.bypass_blocked`,
    `policy.locked`) inside `tracing::warn!`/`error!` events for the
    refusal audit channel. The strings are frozen here.
  - **`ManagedPolicySource` is plumbed at the type level but not yet
    wired into the runtime startup.** The synchronous
    `load_managed_policy(&config_dir)?` calls inside the RPC handlers
    are still the enforcement path (re-read on every RPC; cheap because
    the file is < 1 KB). A future task can build one
    `ManagedPolicySource` at `main.rs` boot, hand the receiver to the
    supervisor's `SessionEntry`, and skip the per-RPC file read — at
    that point the resolver's bypass / cap re-fetch lands "for free"
    on the next turn boundary.
  - **`crates/core/src/security/tool_classes.rs` is still inline.**
    The TOML-driven `tool-classifications.toml` flagged in `design/04
    §3.10` is V1.0 polish; until then, adding a tool to the table is a
    one-line edit at the head of the const init.
- **Deliberate debt:**
  - **Tool classification table is const.** The TOML file is V1.0.
  - **Subcodes are message-body strings, not dedicated error variants.**
    See drift note. Promotion to a new `Error::PolicyYoloBlocked` /
    `Error::PolicyBypassBlocked` variant is straightforward when Task 44
    wants typed-error switching at the audit boundary; the wire
    contract (string-prefix discrimination) is frozen here.
  - **`ManagedPolicySource` is built-but-unwired.** The watcher
    + debounce loop is tested in isolation
    (`crates/core/tests/permission_runtime.rs::hot_reload_observes_managed_json_changes`)
    but no production code consumes it yet. See open question.
  - **No `TODO`/`FIXME`/`todo!()`/`unimplemented!()` markers in new code.**
- **Smoke-gate state:** unchanged. Per pre-decision 8 the mode-enforcement
  smoke check is SKIPPED for V0.1 — exercising it requires the smoke
  client to drive `Sessions.UpdateSessionPermissionMode` AND fake a
  tool-call event, which is impractical against the existing echo
  agent. The behaviour is covered by
  `crates/core/tests/permission_runtime.rs` instead;
  `scripts/smoke.sh` still exits 0 with "Smoke gate v2: PASSED".
