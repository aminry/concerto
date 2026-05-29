# Task 41 — Filesystem Allow-List and Hard Deny-List

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 32, 33 |
| Touches subsystem(s) | 12 (Security), 04 (Agent Supervisor) |
| Smoke gate | unchanged |

## Goal
Enforce the filesystem allow-list / deny-list from `design/00 §7.2` and `design/12`. The allow-list is "the workarea's worktree + `.context/` + project-declared additional paths." The deny-list is the hard floor (`~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.kube`, `~/.netrc`, `~/.docker/config.json`). Tool calls that target paths outside the allow-list go through `MustAsk`; tool calls into the deny-list go through `MustAsk` regardless of mode, with red urgent styling.

## Inputs to read before starting
- `design/00_Architecture_Overview.md` §7.2 (filesystem allow-list + deny-list).
- `design/12_Security_Identity.md` §3.5–§3.7 (paths + how they integrate).
- `design/04_Agent_Supervisor.md` §3.10 (PermissionResolver — deny-list is the hard floor).

## Scope — in
- Implement `crates/core/src/security/path_policy.rs`:
  - `pub struct AllowList { roots: Vec<PathBuf> }` — adds `workarea.worktree_root`, each `repo.worktree_path`, the project's `writable_paths_outside_worktree` (from settings), and `~/concerto/` itself.
  - `pub struct DenyList { roots: Vec<PathBuf> }` — hardcoded list (`~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.kube`, `~/.netrc`, `~/.docker/config.json`).
  - `pub fn classify(path: &Path, allow: &AllowList, deny: &DenyList) -> PathDecision`:
    - `Denied` if matches deny list.
    - `Allowed` if inside allow list.
    - `Outside` if neither (requires `MustAsk` per `auto` mode rules).
  - Path canonicalization via `dunce::canonicalize` to resolve symlinks before classification.
- Wire into `PermissionResolver` (Task 33):
  - For a tool call that includes a path (parse from tool args — heuristic for V0.1, e.g., `Write` / `Edit` / `Bash with rm/cp -r/...` tools name the path), classify the path:
    - Denied → return `AutoDeny` AND mark the approval row with `decision = "denied_by_policy"`.
    - Outside in `auto` mode → return `MustAsk` (per the design's table).
    - Allowed → fall through to the normal mode logic.
- Add a path-extraction utility per tool: when the tool's args are JSON, look up known fields (`file_path`, `path`, `target` for common tools). V0.1 covers the Claude Code built-in tools.
- Tests:
  - Allow-list: path inside the workarea allows.
  - Deny-list: writing to `~/.ssh/config` is denied regardless of mode.
  - Outside both: `auto` mode + outside → MustAsk.
  - Symlink escape: a symlink in `.context/` pointing to `~/.aws/credentials` → classified as Denied.

## Scope — out
- Network allow/deny (V1.0).
- Per-project additional deny patterns (V1.0).
- Docker sandbox enforcement (V1.0 — `design/00 §7.2` mentions opt-in).
- Audit log writing of policy decisions (Task 44).

## Public interface this task locks
- Rust: `crates/core/src/security/path_policy.rs` exports `AllowList`, `DenyList`, `PathDecision`, `classify`. Frozen.
- Hardcoded deny list above. Adding paths to it is fine; removing requires explicit justification.

## Implementation notes
- Use `path-clean` (`path-clean = "1"`) to normalize paths without canonicalizing to ensure deny-prefix comparisons work even on paths that don't yet exist.
- The path-extraction per tool needs to handle nested JSON; use `serde_json::Value::pointer()` for common locations.
- For tools whose args are unparseable (e.g., a `Bash` tool with a shell command that includes a path), V0.1 conservatively classifies as `Outside` and requires `MustAsk`. The destructive-command intercept (Task 43) provides a second line of defense.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core path_policy` → all classification tests pass.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual: in `auto` mode, attempt an agent action that targets `~/.ssh/config` → verify the resolver returns `MustAsk` with a "policy" badge.
5. `scripts/smoke.sh` still passes.

## Definition of Done
- [x] Verification commands pass.
- [x] Symlink-escape via a deny-list path is caught.
- [x] Path classification is exhaustively tested (allowed / outside / denied; with absolute, relative, symlink, and missing paths).
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `crates/core/src/security/path_policy.rs` (new)
- `crates/core/src/security/mod.rs` (modified)
- `crates/core/src/agent_supervisor/approval.rs` (modified — invokes path_policy)
- `crates/core/src/agent_supervisor/tool_args.rs` (new — per-tool arg parsing)
- `crates/core/tests/path_policy.rs` (new)

## Commit message
```
phase-3: filesystem allow-list + deny-list enforcement

AllowList = workarea worktree + .context/ + writable_paths_outside.
DenyList = hardcoded ~/.ssh, ~/.aws, ~/.gnupg, ~/.kube, ~/.netrc,
~/.docker/config.json. PermissionResolver consults path classification
before mode logic. Symlink escapes are caught via canonicalization.

Refs: tasks/41-filesystem-allow-deny.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **`path-clean` 1.0.1 is used instead of `dunce::canonicalize`** for the
    lexical fallback. `dunce` is Windows-only correctness glue; the V0.1
    port is Unix-only (`design/00 §6.11`), and `path-clean` ships the same
    `path.Clean` algorithm the design references for prefix-matching on
    not-yet-created paths. The two-stage strategy (`std::fs::canonicalize`
    first; `path_clean::clean` on error) is documented in
    `crates/core/src/security/path_policy.rs` module docs.
  - **`AllowList::for_workarea` takes an explicit `home: &Path`** so tests
    can fake the home dir without touching `$HOME`. The actor-side
    wrapper `build_path_policy` reads `home::home_dir()` at the
    call site; fallback to `/` on lookup failure keeps the deny-list
    expansion conservative (the deny prefixes still match the lexical
    fallback for `~/.ssh`-style literals via `canonicalize_or_clean`).
  - **`for_workarea_from_db` is a new free function in
    `security::path_policy`** (rather than a method on `AllowList`).
    It encapsulates the three SQL reads needed (`workareas::get`,
    `list_workarea_repos`, `workspaces::get` for project lookup, then
    `projects::get_settings_json`) so the supervisor's dispatch path
    stays small. Pre-decision 3 spec'd `AllowList::for_workarea(workarea,
    repos, settings)` which is the pure constructor; this DB wrapper
    layers on top.
  - **`DENIED_BY_POLICY = "denied_by_policy"`** lives in
    `agent_supervisor::approval` alongside `user_decision_string` —
    same module as the other `tool_approvals.decision` wire strings.
    Frozen by Task 41.
  - **`PolicyVerdict` enum** added to `approval.rs` (variants
    `Passthrough | Outside | Denied`). The supervisor consults it
    BEFORE the resolver's mode-class decision so `Denied` forces
    `AutoDeny` regardless of mode. `Outside` is its own variant so the
    integration with destructive-command intercept (Task 43) can
    distinguish "outside the allow-list but not in deny" from "allowed"
    when it lands.
  - **Path extraction handles `Read`/`Write`/`Edit`/`Bash`/`NotebookEdit`**.
    `Bash` parsing is best-effort regex-style token scan for absolute or
    `~/`-prefixed paths (per `tasks/41` §"Implementation notes"); other
    tool names return `None` and the policy passes through.
- **Open questions for next task:**
  - **Task 42 (runtime mode enforcement)** should consume
    `PolicyVerdict` directly from `agent_supervisor::approval` rather
    than re-derive the classifier. The supervisor's `dispatch_parse_event`
    is the canonical call site.
  - **Task 43 (destructive-command intercept)** can fold its own
    decision into the `denied_by_policy` row string when a command
    matches both a destructive pattern AND a deny-list path. V0.1 only
    handles the path-policy side; the destructive intercept layers on
    top.
  - **Task 44 (audit JSONL writer)** should emit a structured event
    when `tool_approvals.decision = "denied_by_policy"` — the string
    is the discriminator between a user `"deny"` and a policy floor.
  - **Performance**: `build_path_policy` runs three SQL reads per
    `AwaitingApproval`. The cost is negligible inside the approval gate
    (human-latency-bound), but the V1.0 work to cache the
    `(AllowList, DenyList)` pair on `SessionEntry` is straightforward:
    seed at `start_session` time, invalidate on workarea settings
    update.
- **Deliberate debt:**
  - **Path extraction from `Bash` tool args is best-effort** — V0.1
    matches any whitespace-separated token starting with `/` or `~/`
    and strips one layer of surrounding quotes and a leading `>`
    redirect. Complex shell constructs (`$(cmd)`, glob expansion,
    `cd` + relative paths) classify as `Outside` (no path extracted →
    passthrough), and Task 43's destructive-command intercept is the
    second line of defense.
  - **Symlink escape via `..`-through-a-symlink** is not caught by the
    lexical fallback when the candidate path doesn't exist
    (`std::fs::canonicalize` would resolve it but only for existing
    paths). Documented in `path_policy.rs` module docs; the
    "open + readlinkat the parent" scheme is V1.0 per
    `design/12 §3.5`.
  - **`AllowList::for_workarea` reads `project_settings_json` for the
    `writable_paths` array** — keys are case-sensitive, malformed JSON
    yields an empty list, and the lookup uses a flat top-level field
    (no nested object). V1.0 grows the project-settings schema.
  - **No `TODO`/`FIXME`/`todo!()`/`unimplemented!()` markers in new code.**
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still exits 0
  with "Smoke gate v2: PASSED". The path-policy code path is only
  reached on an `AwaitingApproval` event; the echo agent never emits
  one, so the gate exercises the new code negatively (no regression).
