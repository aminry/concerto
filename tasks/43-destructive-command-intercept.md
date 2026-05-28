# Task 43 — Destructive Command Intercept

| Field | Value |
|---|---|
| Phase | 3 |
| Size | small (≤4h) |
| Depends on | 42 |
| Touches subsystem(s) | 12 (Security), 04 (Agent Supervisor) |
| Smoke gate | unchanged |

## Goal
Add the destructive-command pattern intercept from `design/04 §3.10` and `design/12 §3.6`: a set of regex patterns (`rm -rf`, `git push --force`, `DROP TABLE`, `kubectl delete`, etc.) that ALWAYS require approval regardless of permission mode, with red urgent styling. Bypassed only when `bypass_destructive_guard = true` AND the entry ceremony was completed.

## Inputs to read before starting
- `design/04_Agent_Supervisor.md` §3.10 (destructive table — always confirm unless bypass).
- `design/12_Security_Identity.md` §3.6 (destructive command patterns).

## Scope — in
- Add `crates/core/src/security/destructive.rs`:
  - `pub fn is_destructive(tool: &ToolCall) -> bool` — pattern-matches against the tool's args.
  - Patterns include (as regex):
    - `rm -rf|--recursive --force` (and equivalents).
    - `git push --force(-with-lease)?`.
    - `git reset --hard`.
    - `git branch -D` / `git tag -d`.
    - `DROP TABLE` / `TRUNCATE TABLE` (case-insensitive).
    - `kubectl delete`.
    - `docker rm` / `docker volume rm` / `docker system prune`.
    - `mkfs`, `dd of=/dev/`, `parted`, `wipefs`.
    - `sudo` (any).
  - Each pattern carries a human-readable label (`"force-push"`, `"recursive-delete"`, etc.) surfaced in the approval prompt.
- Wire into `PermissionResolver` (Task 33 + 42): before mode-based logic, run `is_destructive`. If true:
  - If `bypass_destructive_guard = true`: return `AutoApprove`.
  - Else: return `MustAsk` AND mark the approval row with `urgent = true` (extend `tool_approvals` schema in migration `0006_destructive.sql`).
- Add red-urgent styling: extend the `AwaitingApproval` proto event with an `urgent: bool` field and a `destructive_label: optional string` field.
- Tests:
  - Each pattern in the list: a fixture tool-call body matches → `is_destructive = true`.
  - With `bypass_destructive_guard=true` + the pattern → AutoApprove + audit row.
  - With `yolo` mode + `bypass_destructive_guard=false` + pattern → MustAsk (yolo doesn't bypass).
  - Negative cases: `rm file.txt` (no flags) → not destructive.

## Scope — out
- User-customizable patterns (V1.0).
- Per-project additional destructive patterns (V1.0).
- Pattern-based audit metrics dashboards (V2.0).

## Public interface this task locks
- Rust: `crates/core/src/security/destructive.rs::is_destructive`. Frozen.
- Pattern list as written. Adding patterns is fine; removing requires explicit security justification.
- Proto: `AwaitingApproval` adds `urgent` + `destructive_label` fields.
- DB: `tool_approvals.urgent INTEGER DEFAULT 0` via migration `0006_destructive.sql`.

## Implementation notes
- Pattern matching is done against a stringified version of the tool's args (`serde_json::to_string`). For Bash-style tools where the command is a single string, this is direct. For structured tools (`Edit { file_path, ... }`), match against the file_path or tool name as appropriate.
- Use `once_cell::sync::Lazy<Vec<(Regex, &'static str)>>` for the pattern table.
- Be conservative: false positives are fine (user gets one extra prompt); false negatives are catastrophic.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core destructive` → all positive + negative cases pass.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual: in `yolo` mode (no bypass), have the agent attempt `rm -rf node_modules`; verify the approval prompt appears with the red-urgent flag; resolve.
5. `./scripts/regen-interfaces.sh && git diff` → committed.
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] Every pattern matches its target.
- [ ] No false negatives on a smoke set of 20 dangerous commands.
- [ ] Yolo-no-bypass still asks for destructive commands.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/persist/migrations/0006_destructive.sql` (new)
- `crates/core/src/security/destructive.rs` (new)
- `crates/core/src/agent_supervisor/approval.rs` (modified)
- `crates/proto/proto/concerto/v1/streams.proto` (modified — AwaitingApproval urgent/destructive_label)
- `crates/core/tests/destructive_intercept.rs` (new)
- `docs/interfaces/proto.md`, `schema.md` (regenerated)

## Commit message
```
phase-3: destructive command intercept

is_destructive() matches a curated pattern list (rm -rf, force push,
DROP TABLE, kubectl delete, dd of=/dev/, sudo, etc.). Intercept runs
before mode logic; bypass requires bypass_destructive_guard=true.
AwaitingApproval carries urgent + destructive_label for red-urgent
client styling.

Refs: tasks/43-destructive-command-intercept.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** patterns are hardcoded; user customization is V1.0.
- **Smoke-gate state:** unchanged.
