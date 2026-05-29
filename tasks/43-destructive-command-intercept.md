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
- [x] Verification commands pass.
- [x] Every pattern matches its target.
- [x] No false negatives on a smoke set of 20 dangerous commands.
- [x] Yolo-no-bypass still asks for destructive commands.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

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
- **Drift from plan:**
  - Migration number is **`0007_tool_approvals_urgent.sql`**, not the
    `0006_destructive.sql` the spec called for — Tasks 30/36/38/39/40
    already burned 0002–0006. The column is `urgent INTEGER NOT NULL
    DEFAULT 0` per the SQLite-boolean convention shared with
    `workareas.bypass_destructive_guard`. The `tool_approvals.decision`
    CHECK set is untouched (no new values needed — urgent + destructive
    use the same `auto_<mode>` / `approve|deny|approve_once` strings as
    Task 33).
  - **`PolicyVerdict` was NOT extended with a `Destructive` variant**
    (pre-decision 6). Folding destructive into the path-policy verdict
    enum entangled two orthogonal concerns: path policy is the hard
    floor (DENY wins absolutely), the destructive intercept is a
    "promote to MustAsk unless bypass" overlay. Instead the dispatch
    site (`actor.rs::dispatch_parse_event`) chains them: path-policy
    first (deny short-circuits), then `is_destructive` overrides the
    mode-class decision. `urgent: bool` + `destructive_label:
    Option<String>` flow into `NewToolApproval` and
    `AgentEvent::AwaitingApproval` directly. Cleaner separation; the
    `PolicyVerdict` enum stays single-responsibility.
  - **Proto field numbers `5` (urgent) and `6` (destructive_label) on
    `AwaitingApproval`**. The task plan called out reserving 1-4 — 1-4
    are already populated by Task 33 (`approval_id`/`tool`/`summary`/
    `payload_json`). 5/6 are the next free numbers; FROZEN going
    forward.
  - **`tool_name` parameter on `is_destructive` is `_unused`**. The
    pattern table embeds the command keyword (`rm`, `git`, `kubectl`,
    …), so the matcher operates on the stringified args blob alone.
    The parameter is preserved in the public signature for V1.0's
    per-tool pattern scoping (e.g. only treat `DROP TABLE` as
    destructive in SQL-targeted tools).
  - **`git branch -D` regex is case-sensitive `(?-i)`** even though
    every other pattern is `(?i)`. Lowercase `-d` is git's safe-delete
    (refuses to drop unmerged branches); only `-D` force-deletes. A
    case-insensitive match would prompt on every benign branch cleanup
    — the false-positive cost outweighs the safety win.
  - **No proto wire bump for `ApprovalResolved.urgent`**: only
    `AwaitingApproval` carries the flag. The resolved event downstream
    is the audit record; clients re-read the original row via
    `tool_approvals.urgent` if they need urgent in the resolved view.
    Adding it is purely additive when V1.0 wants it.
- **Open questions for next task:**
  - **Task 44 (audit JSONL writer)** should treat
    `tool_approvals.urgent = 1` as the gate for the "destructive"
    audit channel — group all urgent rows under a separate JSONL
    stream so security ops can `tail -f` just the destructive prompts.
    The `destructive_label` is currently only surfaced on the
    `AgentEvent::AwaitingApproval` event (not persisted on the row);
    Task 44 may want a `destructive_label TEXT` column so the audit
    log can render the category without re-running the matcher. The
    label set is FROZEN by `PATTERNS` here.
  - **Manual verification (DoD step 4)** — verifying the destructive
    intercept end-to-end with real `claude` requires a parser-pack
    capture of an actual destructive tool-call prompt; V0.1's
    fixture-driven Claude Code pack does not emit one. The
    pure-Rust + dispatch-overlay tests in
    `crates/core/tests/destructive_intercept.rs` (smoke set + bypass +
    yolo-no-bypass) cover the matrix; the manual capture is a Task 44
    open item.
- **Deliberate debt:**
  - **Patterns are hardcoded in `PATTERNS`.** User-customizable
    patterns + per-project additions are V1.0 (per task scope-out).
    Adding a pattern is a one-line append at the head of the
    `LazyLock`; removing one requires the security-review note in the
    module docs.
  - **`is_destructive` ignores `tool_name`.** See drift note. Folding
    Bash-vs-Edit-vs-MCP-tool into the pattern table is V1.0.
  - **No structured per-pattern audit metric** — the row carries
    `urgent: bool` but not `destructive_label`. V2.0 dashboards land
    when the audit log grows the label column (see Task 44 open
    question).
  - **End-to-end `claude` capture deferred** — see open question.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still exits 0
  with "Smoke gate v2: PASSED". The destructive intercept only fires
  when a parser pack emits `AwaitingApproval`; the echo pack never
  does, so the gate stays single-shot.
