# Task 500 — Phase-5 architecture reconciliation (design amendment to `14`/`16`/`17`)

| Field | Value |
|---|---|
| Phase | 5 |
| Task type | doc |
| Verification tier | 3 |
| Size | small |
| Depends on | — |
| Touches subsystem(s) | 14 (Notifications), 16 (Mobile), 17 (Web) |
| Smoke gate | unchanged |

## Goal
Reconcile the canonical design docs (`design/14`/`16`/`17`) with the **built reality** of the code
Phase 5 lands on, so the eight (then web/mobile) task authors transcribe correct signatures instead
of the docs' idealized ones. Runs **first** in Phase 5 (doc, `design/` only — zero code collision),
exactly like Task 200 / 315.0 / 400 ran first in their phases. The canonical decisions live in
`tasks/v1.0/PHASE5_PLANNING.md §1–§4`; this task folds the load-bearing ones into the design docs as
dated amendment blocks so the design stays canonical (decision V7) and self-consistent.

## Inputs to read before starting
- `tasks/v1.0/PHASE5_PLANNING.md` §1 (D3–D15), §2, §4 — the locked reconciliations.
- `design/14_Notifications_Push.md` §3.3/§3.5/§4 — the chip/subject/payload shapes to reconcile.
- `crates/proto/proto/concerto/v1/suggestions.proto:29` — the REAL `Chip` (`rule_id`/`action`, no `chip_id`).
- `crates/proto/proto/concerto/v1/devices.proto:173` — the deferred `UpdateDevicePushToken` + `push_platform` CHECK.
- `crates/core/src/connect_bridge.rs` + `crates/core/src/api_server.rs:270` — the live, default-OFF bridge.

## Scope — in
Append a dated **"Amendment (2026-06-14 — Phase-5 planning reconciliation)"** block to each of
`design/14`, `design/16`, `design/17` recording (and only recording — no behavioral redesign):
- **14:** subject_kind taxonomy = `{workspace, workarea, session, pull_request, schedule_run}` (D3);
  chips reuse the real `Chip{rule_id,action,…}` shape, `ActOnChip` keys on `rule_id`, the
  `action`-token→dispatch map (D4); first-wins guard is the existing `tool_approvals`/`ResolveApproval`
  (D5); `WakeupPayload` carries `{notification_id, kind, source}` only (D6); `push_platform` widens to
  add `'expo'` + `UpdateDevicePushToken` lands (D8); `notification.events` rides `Event.checks_opaque=17`
  (D9); timestamps are `int64` unix-ms.
- **16:** native module is **iroh-ffi-first** (D12); the user-facing chat tab is **"Concerto"** (D14);
  the workspaces drill-down has **no project tier** (the Project→Workspace collapse is done; D14);
  the RN diff perf budget's on-device verdict is PENDING operator field measurement (514 ships behind
  the V1.5 native fallback).
- **17:** mobile shares only `@concerto/client`; web reuses `@concerto/ui` (D11); TS proto codegen is
  net-new (buf + connect-es, gRPC-Web binary; D10); the Connect-Web bridge is default-OFF and the
  auth-less/TLS-less bridge is never exposed on a non-loopback interface (D15); ephemeral pairing's
  Tier-2 path uses a stub-phone signer (real phone-mediated = Tier-3).

## Scope — out
- Any code, proto, migration, or test change (those are 501+). This task edits `design/*.md` only.
- Re-deciding any §1 decision (that is a new planning conversation, PHASE5_PLANNING authority line).

## Public interface this task locks
- None (doc). It records contracts FROZEN by later tasks (501/503/507/507.5/509); it locks no wire shape.

## Implementation notes
- Mirror the README's existing amendment style (the 2026-06-08 Project→Workspace block): a short dated
  block near the top of each doc, bullet-form, each bullet citing the PHASE5_PLANNING decision id +
  the built-code anchor. Keep the original prose; amendments supersede where they conflict.

## Verification
**Tier 3 (doc).** `git diff --stat` shows only `design/14`/`16`/`17` changed. Human-read gate: each
amendment bullet matches a PHASE5_PLANNING §1 decision and a real code anchor. No code/proto/test
touched ⇒ `cargo`/`pnpm`/smoke unaffected; `./scripts/regen-interfaces.sh` produces no diff.

## Definition of Done
- [ ] Amendment blocks appended to `design/14`, `design/16`, `design/17`
- [ ] Every bullet cites a PHASE5_PLANNING decision id + a built-code anchor
- [ ] No files outside `design/*.md` modified
- [ ] Single commit with the message below

## Outputs
- `design/14_Notifications_Push.md` (modified — amendment block)
- `design/16_Mobile_Clients.md` (modified — amendment block)
- `design/17_Web_Client.md` (modified — amendment block)

## Commit message
```
phase-5: reconcile design 14/16/17 with built reality (planning amendment)

Folds the PHASE5_PLANNING decisions into the canonical design docs:
subject_kind taxonomy, real Chip/action dispatch, first-wins guard,
WakeupPayload shape, push_platform widen, iroh-ffi-first native module,
Concerto naming + no project tier, @concerto/client/ui split, bridge
posture, ephemeral-pairing stub-phone. Doc-only; runs first in Phase 5.

Refs: tasks/v1.0/500-phase5-architecture-reconciliation.md
```

## Handoff Notes (filled in when finishing)
- Amendments appended to all three docs; no code touched. 501 is unblocked.
