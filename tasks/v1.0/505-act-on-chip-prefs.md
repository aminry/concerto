# Task 505 — `ActOnChip` dispatch + preference hierarchy + per-workspace opt-out

| Field | Value |
|---|---|
| Phase | 5 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium |
| Depends on | 504 |
| Touches subsystem(s) | 14 (Notifications) |
| Smoke gate | unchanged |

## Goal
Add chip-action dispatch + the push-preference resolver. `ActOnChip` finds a chip by `rule_id`,
classifies its free-form `action` token into a `ChipDispatch` (ResolveApproval / SendMessage /
Navigate, design/14 §6.3), and records the idempotent first-wins marker. The preference resolver
decides push-vs-inbox per the §3.8 hierarchy (kind default → per-workspace opt-out → per-device DND).

## Inputs to read before starting
- `tasks/v1.0/PHASE5_PLANNING.md` D4 (chip identity/action map), D5 (first-wins); `design/14 §3.5/§3.8/§6.3`.
- `crates/core/src/notifications/{model,fanout}.rs` (504); `crates/proto/.../suggestions.proto` (`Chip`).

## Scope — in
- `crates/core/src/notifications/chip_dispatch.rs`: `ChipDispatch`, `classify_action` (prefix-matched
  D4 map), `ActOutcome`, `act_on_chip` (chip lookup by `rule_id` + idempotent `set_action_taken`).
- `crates/core/src/notifications/prefs.rs`: `default_push_for_kind`, `should_push` (§3.8 hierarchy),
  `parse_workspace_opt_out` (the `notifications_opt_out` `settings_json` key).
- Tests: classify unit; prefs unit (default per kind, workspace opt-out, DND window, parse shapes);
  a core integration test for `act_on_chip` (classify + first-wins across two devices + NotFound).

## Scope — out
- Executing the dispatch (calling `Sessions.ResolveApproval`/`SendMessage`) + `approval.cancelled`
  broadcast (507, holds the supervisor handle). Reading the workspace `settings_json` from the DB +
  the device `dnd_until` at notify time (507 threads them into `should_push`).

## Public interface this task locks
- `chip_dispatch::{ChipDispatch, classify_action, ActOutcome, act_on_chip}`;
  `prefs::{default_push_for_kind, should_push, parse_workspace_opt_out}`.

## Implementation notes
- `act_on_chip` records the **denormalized** marker; the real first-wins guard is the existing
  `tool_approvals`/`ResolveApproval` idempotency 507 hits when executing the dispatch (D5).
- `classify_action` matches by prefix so the free-form `Chip.action` catalog grows without a wire break.

## Verification
**Tier 1.** `cargo clippy -p concerto-core --all-targets -- -D warnings` · `cargo test -p concerto-core
--lib notifications::chip_dispatch` (1) · `--lib notifications::prefs` (4) · `--test notifications_chip`
(1) · `cargo fmt --all -- --check`. No proto/schema change ⇒ no regen.

## Definition of Done
- [x] chip classification + `act_on_chip` (idempotent first-wins marker)
- [x] preference resolver (§3.8 hierarchy) + per-workspace opt-out parse
- [x] 6 tests green; clippy/fmt clean
- [x] Single commit with the message below

## Outputs
- `crates/core/src/notifications/{chip_dispatch,prefs}.rs` (new) + `notifications/mod.rs` (mod lines)
- `crates/core/tests/notifications_chip.rs` (new)

## Commit message
```
phase-5: ActOnChip dispatch + push-preference resolver + per-workspace opt-out

Adds chip-action classification (rule_id lookup + action->ResolveApproval/
SendMessage/Navigate, D4) with an idempotent first-wins marker, and the §3.8
push-preference resolver (kind default -> per-workspace opt-out -> per-device
DND) + the notifications_opt_out settings_json parse. Dispatch execution +
settings/DND threading are 507.

Refs: tasks/v1.0/505-act-on-chip-prefs.md
```

## Handoff Notes (filled in when finishing)
- 507 executes `ActOutcome.dispatch`: ResolveApproval→supervisor, SendMessage→supervisor, Navigate→event;
  on `already_resolved` it surfaces AlreadyResolved + broadcasts `approval.cancelled`.
- 507 reads the workspace `settings_json` (→ `parse_workspace_opt_out`) + the device `dnd_until` and
  passes them into `should_push` before fanning out.
