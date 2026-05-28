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
- [ ] Verification commands pass.
- [ ] Symlink-escape via a deny-list path is caught.
- [ ] Path classification is exhaustively tested (allowed / outside / denied; with absolute, relative, symlink, and missing paths).
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

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
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** path extraction from `Bash` tool args is best-effort.
- **Smoke-gate state:** unchanged.
