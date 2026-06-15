# Task 504 — Multi-device fan-out + first-wins + active-viewing + post-wakeup fetch

| Field | Value |
|---|---|
| Phase | 5 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium |
| Depends on | 503, 209 |
| Touches subsystem(s) | 14 (Notifications), 12 (Devices) |
| Smoke gate | unchanged |

## Goal
Add the wakeup fan-out: compute the eligible device set, subtract actively-viewing devices, send the
ID-only wakeup to each with bounded retry, and provide the post-wakeup fetch (record `fetched_at` +
return the wire payload). First-to-approve-wins reuses the existing `tool_approvals`/`ResolveApproval`
guard (D5); the loser's `approval.cancelled` broadcast is 507's wiring.

## Inputs to read before starting
- `tasks/v1.0/PHASE5_PLANNING.md` D5 (first-wins guard), §6 (deps); `design/14 §3.4` (fan-out +
  active-viewing), §6.1/§6.2 (notify + fetch flows), §8 (retry).
- `crates/core/src/notifications/push/` (503: `PushBackend`/`MockPushBackend`/`WakeupBody`).
- `crates/core/src/handlers/streams.rs` (the subscription registry — keys by subscriber id, NOT
  device id → active-viewing-by-device is a seam).

## Scope — in
- `crates/persist/src/notifications.rs`: `list_pushable_devices(now)` — active + push-token + non-DND.
- `crates/core/src/notifications/fanout.rs`: `ActiveViewing` trait + `NoActiveViewing` default,
  `eligible_targets` (persist tuples → `PushTarget`, drop unknown platforms), `plan_fanout` (subtract
  active-viewing), `deliver` (send loop, retry ≤ `MAX_SEND_ATTEMPTS`, per-device `DeliveryReport`),
  `fetch_for_device` (load → record `fetched_at` → `row_to_proto`).
- Tests: persist eligibility filter; fanout unit tests (eligible-mapping, plan subtraction, retry-count,
  ok/stale-token); a core integration test for `fetch_for_device` (payload + recorded `fetched_at`).

## Scope — out
- The live `notify()` that calls `deliver` + records `delivered_at` + nulls stale tokens, and the gRPC
  `GetNotification` RPC (507). `ActOnChip` resolution + `approval.cancelled` broadcast (505/507). The
  **real** device-tagged active-viewing oracle (a deferred infra seam, see Handoff).

## Public interface this task locks
- `list_pushable_devices`; `fanout::{ActiveViewing, NoActiveViewing, eligible_targets, plan_fanout,
  deliver, FanoutResult, fetch_for_device, MAX_SEND_ATTEMPTS}`.

## Implementation notes
- `deliver` retries transport `Err` up to MAX (backoff timing is a refinement); a `DeliveryReport`
  with `is_device_not_registered()` signals 507 to null `devices.push_token`.
- `fetch_for_device` upserts the delivery `fetched_at` (idempotent COALESCE upsert).

## Verification
**Tier 2** (double = `MockPushBackend` + a real `Persistence`; does NOT cover real Expo delivery or the
real cross-device active-viewing oracle → Phase-5 Tier-3). `cargo clippy -p concerto-persist
-p concerto-core --all-targets -- -D warnings` · `cargo test -p concerto-persist --test notifications`
(10) · `cargo test -p concerto-core --lib notifications::fanout` (5) · `cargo test -p concerto-core
--test notifications_fanout` (1) · `cargo fmt --all -- --check`. No proto/schema change ⇒ no regen.

## Definition of Done
- [x] eligible-set query + fan-out planner + retrying send loop + post-wakeup fetch
- [x] active-viewing as a trait seam (default no-suppression) + documented real-impl gap
- [x] 16 tests green (10 persist + 5 fanout + 1 integration); clippy/fmt clean
- [x] Single commit with the message below

## Outputs
- `crates/persist/src/notifications.rs` (modified) · `crates/core/src/notifications/fanout.rs` (new) +
  `notifications/mod.rs` (mod line) · `crates/persist/tests/notifications.rs` (modified) ·
  `crates/core/tests/notifications_fanout.rs` (new)

## Commit message
```
phase-5: notification fan-out + post-wakeup fetch + active-viewing seam

Adds the eligible-device query (active + push-token + non-DND), the fan-out
planner (subtract actively-viewing), the retrying send loop over PushBackend,
and the post-wakeup fetch (records fetched_at + returns payload). Active-viewing
is a pluggable trait (default no-suppression); the real device-tagged oracle is
a documented seam. First-wins reuses tool_approvals/ResolveApproval (D5).

Refs: tasks/v1.0/504-wakeup-fanout-firstwins.md
```

## Handoff Notes (filled in when finishing)
- **Active-viewing real impl is a deferred seam:** `StreamsHandler` keys subscriptions by a monotonic
  subscriber id, not device id (`handlers/streams.rs`), so "device X is viewing workarea Y" needs
  auth-tagged subscriptions + a 30s recency window — net-new infra. `NoActiveViewing` is the
  conservative default (wake everyone); the real oracle plugs into `plan_fanout` when that lands.
- 505 next: `ActOnChip` action-token → ResolveApproval/SendMessage dispatch + prefs hierarchy +
  per-workspace opt-out (reads `dnd_until` + `settings_json`).
- 507 wires `deliver` into the live `notify()`, records `delivered_at`, nulls stale tokens, and adds
  `GetNotification` over the gRPC service.
