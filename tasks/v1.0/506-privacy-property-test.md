# Task 506 — Privacy property test (no PII in `WakeupPayload`; no body for enterprise-private)

| Field | Value |
|---|---|
| Phase | 5 |
| Task type | rust |
| Verification tier | 1 |
| Size | small |
| Depends on | 503 |
| Touches subsystem(s) | 14 (Notifications) |
| Smoke gate | unchanged |

## Goal
Prove — over arbitrary inputs — the wakeup-privacy invariants: the wakeup payload is strictly
`{notification_id, kind, source}` (no notification content can structurally leak to Apple/Google/Expo),
and an enterprise-private (opted-out) workspace + a DND device never push (design/14 §3.2/§3.8/§10,
locked `00 §7.2`). Adds `proptest` (cargo-deny-vetted).

## Inputs to read before starting
- `tasks/v1.0/PHASE5_PLANNING.md` D6 (WakeupBody shape), D13 (proptest = Stop-and-ask on advisory);
  `design/14 §10` (property-based no-PII test), §3.2 (ID-only).
- `crates/core/src/notifications/push/mod.rs` (`WakeupBody`), `prefs.rs` (`should_push`).

## Scope — in
- Add `proptest = "1"` to `crates/core` dev-dependencies; **vet with `cargo deny check`**.
- `crates/core/tests/notifications_privacy.rs`: 4 `proptest!` properties — wakeup is strictly 3 keys
  for any id/kind; tagged content never appears in wakeup bytes; opted-out workspace never pushes (all
  kinds, any time); DND window suppresses push.

## Scope — out
- The live notify() that applies `should_push` before fan-out (507). Real-Expo delivery (Tier-3).

## Public interface this task locks
- None (test + dev-dep). Enforces the FROZEN `WakeupBody` (503) + `should_push` (505) invariants.

## Implementation notes
- The structural "exactly 3 keys" property is airtight by construction (`WakeupBody` has only those
  fields) — the test guards against a future field being added without re-examining privacy.
- `cargo deny check` must pass clean; an advisory/ban/license hit from proptest's tree is a Stop-and-ask.

## Verification
**Tier 1.** `cargo test -p concerto-core --test notifications_privacy` (4) · `cargo deny check`
(advisories/bans/licenses/sources ok) · `cargo clippy -p concerto-core --all-targets -- -D warnings` ·
`cargo fmt --all -- --check`. `Cargo.lock` updated (proptest + rand 0.9 — trivially mergeable).

## Definition of Done
- [x] proptest dev-dep added + cargo-deny clean (no Stop-and-ask)
- [x] 4 privacy properties green; clippy/fmt clean
- [x] Single commit with the message below

## Outputs
- `crates/core/Cargo.toml` (proptest dev-dep) · `crates/core/tests/notifications_privacy.rs` (new) ·
  `Cargo.lock` (proptest + transitive)

## Commit message
```
phase-5: privacy property test — no PII in WakeupPayload (proptest)

Adds proptest (cargo-deny clean) + 4 property-based privacy invariants: the
wakeup is strictly {notification_id, kind, source} for any input, tagged
content never appears in the wakeup bytes, and opted-out workspaces + DND
devices never push (design/14 §3.2/§3.8/§10).

Refs: tasks/v1.0/506-privacy-property-test.md
```

## Handoff Notes (filled in when finishing)
- `cargo deny check` = advisories/bans/licenses/sources ok with proptest 1.11 + rand 0.9 — no new
  advisory acceptance needed.
- 507 (the capstone) applies `should_push` before `deliver`, wires the live `notify()` + the
  `Notifications` gRPC service + `notification.events` + live `notify_user`/`read_inbox_summary`.
