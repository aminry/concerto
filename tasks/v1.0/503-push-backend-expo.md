# Task 503 — `PushBackend` + `ExpoPushBackend` + `MockPushBackend` + ID-only `WakeupBody` + `UpdateDevicePushToken` (0018)

| Field | Value |
|---|---|
| Phase | 5 |
| Task type | rust |
| Verification tier | 2 |
| Size | medium |
| Depends on | 501 |
| Touches subsystem(s) | 14 (Notifications), 12 (Devices), 09 (Persistence) |
| Smoke gate | unchanged |

## Goal
Add the push-delivery seam + the device push-token plumbing. Freezes the `PushBackend` trait (Expo
live, Mock double, DirectApnsFcm frozen V1.5 seam), the **ID-only `WakeupBody`** (`{notification_id,
kind, source}` — the privacy contract), the deferred `Devices.UpdateDevicePushToken` RPC, and
migration `0018` (`push_platform`+`'expo'` widen + `dnd_until`).

## Inputs to read before starting
- `tasks/v1.0/PHASE5_PLANNING.md` D6/D7/D8 + §4.3; `design/14 §3.2` (ID-only), §3.6 (`PushBackend`/Expo), §8.
- `crates/core/src/maestro/provider.rs` (the trait template); `crates/persist/migrations/0010_*.sql`
  (the `writable_schema` CHECK-widen pattern); `devices.proto:173` (the deferred RPC).

## Scope — in
- `crates/core/src/notifications/push/{mod,expo,mock}.rs`: `PushBackend` trait, `PushPlatform`,
  `PushTarget`, `WakeupBody` (+ `to_bytes`), `DeliveryReport` (+ `is_device_not_registered`),
  `ExpoPushBackend` (reqwest POST to exp.host, BYO access token, `build_message`/`parse_response`),
  `MockPushBackend` (records sends, programmable outcome incl. transport-error + stale-token).
- `crates/proto/proto/concerto/v1/devices.proto`: `UpdateDevicePushToken` RPC + request message
  (appended after `GetCoreInfo`, new number).
- `crates/core/src/handlers/devices.rs`: `update_device_push_token` impl.
- `crates/core/src/security/devices.rs`: `DeviceManager::update_push_token` (validates platform →
  `Validation`/`INVALID_ARGUMENT`; unknown id → `NotFound`).
- `crates/persist/migrations/0018_push_platform_expo.sql`: `writable_schema` CHECK widen (+ `'expo'`)
  + `ALTER TABLE devices ADD COLUMN dnd_until INTEGER`.
- Tests: push unit tests (ID-only invariant, platform round-trip, Expo build/parse, Mock record/fail)
  + persist 0018 test (expo accepted, bogus rejected, `dnd_until` writable).

## Scope — out
- Wiring `ExpoPushBackend` into boot + fan-out/retry across devices (504); the privacy property test
  (506); the live `notify()` (507). Real Expo network test (Tier-3 / opt-in).

## Public interface this task locks
- `PushBackend`/`PushTarget`/`WakeupBody`/`DeliveryReport`/`PushPlatform`; the `WakeupBody` JSON shape
  (D6); `Devices.UpdateDevicePushToken` (rpc number); migration `0018`.

## Implementation notes
- `WakeupBody` is the entire payload — exactly 3 keys, asserted by a test (506 proves it exhaustively).
- Expo `register_device`/`revoke_device` are no-ops (Expo tokens are per-install; the `devices` table
  is the registry). Retry/backoff is 504's fan-out, not the backend.
- 0018 uses the `0010` in-place rewrite (CHECK-widen is banned as DROP+recreate under `foreign_keys=ON`).

## Verification
**Tier 2** (double = `MockPushBackend` + the pure `build_message`/`parse_response`; does NOT cover real
Expo/APNs/FCM delivery → Phase-5 Tier-3 checklist). `cargo check/clippy -p concerto-proto
-p concerto-persist -p concerto-core --all-targets -- -D warnings` · `cargo test -p concerto-persist`
(9 in notifications + full suite) · `cargo test -p concerto-core --lib notifications::push` (8) ·
`cargo fmt --all -- --check` · regen interfaces (proto.md + schema.md) committed.

## Definition of Done
- [x] `PushBackend` + Expo + Mock + ID-only `WakeupBody`; 8 push unit tests
- [x] `UpdateDevicePushToken` RPC + handler + `DeviceManager::update_push_token`
- [x] migration 0018 (expo widen + dnd_until) + persist test
- [x] clippy/fmt clean; interfaces regenerated + committed
- [x] Single commit with the message below

## Outputs
- `crates/core/src/notifications/push/{mod,expo,mock}.rs` (new) + `notifications/mod.rs` (mod line)
- `crates/proto/proto/concerto/v1/devices.proto` (modified) · `crates/core/src/handlers/devices.rs`
  (modified) · `crates/core/src/security/devices.rs` (modified)
- `crates/persist/migrations/0018_push_platform_expo.sql` (new) · `crates/persist/tests/notifications.rs`
  (modified) · `docs/interfaces/{proto,schema}.md` (regen)

## Commit message
```
phase-5: PushBackend + Expo/Mock + ID-only WakeupBody + UpdateDevicePushToken (0018)

Adds the push-delivery seam (PushBackend trait; ExpoPushBackend live, BYO
creds; MockPushBackend Tier-2 double), the ID-only WakeupBody privacy contract
({notification_id,kind,source}), the deferred Devices.UpdateDevicePushToken RPC
+ DeviceManager::update_push_token, and migration 0018 (push_platform+'expo'
widen via writable_schema + dnd_until). Fan-out/retry + live notify() are 504/507.

Refs: tasks/v1.0/503-push-backend-expo.md
```

## Handoff Notes (filled in when finishing)
- `WakeupBody::to_bytes()` is what 507/516 stuff into `concerto_transport::WakeupPayload`.
- `DeliveryReport::is_device_not_registered()` → 504 nulls `devices.push_token` on that signal.
- `dnd_until` column is added but consumed by 505's preference resolver (not 503).
- 504 next: eligible-device set (revoked IS NULL + push_token present − actively-viewing) + fan-out +
  first-wins (over tool_approvals) + retry using `MockPushBackend::set_outcome(TransportError)`.
