# Task 40 — Suggestion Engine: Rule Engine + Chips

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 22, 33 |
| Touches subsystem(s) | 07 (Suggestion Engine) |
| Smoke gate | unchanged |

## Goal
Implement the V0.1 rule engine — a small set of built-in rules that listen for agent events and produce suggestion chips. After this task, when the agent's context window crosses thresholds, when tests fail, when the agent finishes a turn — the relevant chip surfaces via `Suggestions.GetSuggestions(workarea_id)` and a `suggestion.events` stream subject. V0.1 has no learning loop yet (per design).

## Inputs to read before starting
- `design/07_Suggestion_Engine.md` (whole — focus on §1 scope, §2 V0.1 row "rule engine only", §3 rule schema, §4 schema, §5 RPC surface, §6 emission flow).
- `design/09_Persistence.md` §4.5 (`suggestion_learn` — present in schema; V0.1 doesn't write to it).

## Scope — in
- Implement `crates/core/src/suggestions/`:
  - `SuggestionEngineActor` (impl `Actor`) subscribed to `session.events` from the broadcast channel.
  - Built-in rules (V0.1 ships 6 from `design/07`):
    1. `context_window_50` — when `ContextUsage.pct >= 50`, emit chip "Compress context now".
    2. `context_window_80` — when `pct >= 80`, emit chip "Start new session with a summary".
    3. `tests_failed` — when an agent message says "tests fail" (regex on content), emit chip "Investigate test failure".
    4. `turn_complete_with_uncommitted` — on TurnComplete, if `gix status` shows uncommitted changes, emit chip "Commit and push".
    5. `awaiting_approval` — when AwaitingApproval, emit chip "Review tool call".
    6. `agent_crashed` — when Crashed, emit chip "Resume agent" or "Start new session".
  - Each rule is a `Box<dyn SuggestionRule>` with: `id`, `name`, `applies(workarea_state, event) -> Option<Chip>`, `priority`.
  - The engine maintains a per-workarea in-memory `WorkareaState` summarizing recent events; rules read this state to decide.
- gRPC `Suggestions.GetSuggestions(workarea_id) -> Suggestions` returns the current chip list for a workarea.
- New stream subject `suggestion.events` (filter on workarea_id) delivers chips as they arise.
- Skip persisting `suggestion_learn` rows in V0.1 — the table exists from the schema (Task 09 didn't create it; add it now in a new migration `0005_suggestion_learn.sql`).
- Tests:
  - Fixture agent emits `ContextUsage { pct: 55 }`; assert the engine emits the `context_window_50` chip.
  - Fixture agent emits `TurnComplete`; with a fixture worktree having uncommitted edits, assert the commit chip emits.
  - Engine de-duplicates: same chip in a 60s window only emits once.

## Scope — out
- Learning loop (V1.0 — updates `suggestion_learn` based on accept/dismiss).
- Org-shared rules (V2.0).
- Push-action chips via subsystem 14 (V1.0).
- LLM-ranked mode (V2.0).
- UI rendering of chips (Phase 3 — Desktop polish task; the API exists here).

## Public interface this task locks
- Rust: `crates/core/src/suggestions/mod.rs` — `SuggestionEngineHandle::list_for_workarea`, the trait `SuggestionRule`. Frozen.
- Proto: `Suggestions.GetSuggestions` + `Suggestions.RecordSuggestionOutcome` (stub in V0.1; just logs). Frozen.
- Stream subject `suggestion.events` (filter: workarea_id). Frozen.
- 6 built-in rule IDs above are reserved namespace; new rules use new IDs.

## Implementation notes
- The engine doesn't tail every stream — it subscribes to `session.events` from the AgentSupervisor's broadcast channel; the supervisor's per-session sender is shared by reference.
- Per-workarea state in `Arc<RwLock<HashMap<WorkareaId, WorkareaState>>>`.
- Chip deduplication: a `HashSet<(WorkareaId, RuleId)>` with TTL; expire entries after 60s.
- For the `tests_failed` regex: a coarse pattern like `(?i)\d+ (test|spec) failed`.
- `RecordSuggestionOutcome` in V0.1 just logs the event via `tracing::info!`; Task 44's audit log will pick it up later.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core suggestions` → tests pass.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual: spawn a session; fake a high context-usage event via test injection; verify the chip is returned by `GetSuggestions`.
5. `./scripts/regen-interfaces.sh && git diff` → committed.
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] All 6 built-in rules fire under their conditions.
- [ ] De-duplication verified.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/persist/migrations/0005_suggestion_learn.sql` (new)
- `crates/persist/src/suggestion_learn.rs` (new — basic insert/read only; learning is V1.0)
- `crates/core/src/suggestions/mod.rs` (new)
- `crates/core/src/suggestions/actor.rs` (new)
- `crates/core/src/suggestions/rules/*.rs` (new — six rule files)
- `crates/core/src/suggestions/state.rs` (new — per-workarea aggregator)
- `crates/proto/proto/concerto/v1/suggestions.proto` (new)
- `crates/proto/proto/concerto/v1/streams.proto` (modified — adds suggestion.events filter)
- `crates/core/src/handlers/suggestions.rs` (new)
- `crates/core/src/main.rs` (modified)
- `crates/core/tests/suggestion_rules.rs` (new)
- `docs/interfaces/proto.md`, `rust-api.md`, `schema.md` (regenerated)

## Commit message
```
phase-3: suggestion engine — rule engine + chip emission

Six built-in rules over session.events. Per-workarea state
aggregator. GetSuggestions + suggestion.events stream surface the
chips to clients. Learning loop is V1.0.

Refs: tasks/40-suggestion-rule-engine.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** suggestion_learn table created but unused; learning loop is V1.0.
- **Smoke-gate state:** unchanged.
