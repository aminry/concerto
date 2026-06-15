# Task 507 — `Notifications` gRPC service + `NotificationHandle` + live `notify_user` + `notification.events`

| Field | Value |
|---|---|
| Phase | 5 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (split a/b) |
| Depends on | 504, 407 |
| Touches subsystem(s) | 14 (Notifications), 08 (Maestro), 10/11 (API) |
| Smoke gate | new:notifications |

## Goal
The Track-A capstone: expose the notifications sub-system. Builds `NotificationHandle` (the `notify()`
orchestration + the caller/gRPC reads), the `Notifications` gRPC service registered at both front-door
sites, the `notification.events` streams subject, boot construction, and the live `notify_user` +
`read_inbox_summary` wiring (replacing the 407 stub sink), plus a `smoke.d` capability.

## Split (to keep `main` green)
- **507a (DONE):** `NotificationHandle` + the `notify()` flow (de-dup → insert/update → event emit →
  push fan-out → record `delivered_at` + null stale tokens) + `get_inbox`/`get_notification`/
  `mark_read`/`act_on_chip`, decoupled behind `NotificationEvents` + `Clock` seams. 3 integration
  tests (insert+push+dedup-refresh, opted-out-no-push, inbox-only-no-push). `clear_push_token` persist
  helper. Build green, no wiring touched.
- **507b (PENDING — the wiring):** add the `Notifications` SERVICE to `notifications.proto`
  (GetNotification/GetInbox/MarkRead/ActOnChip/UpdateWorkspaceSettings/RegisterDevicePushToken) +
  `handlers/notifications.rs` delegating to `NotificationHandle`; `Subject::NotificationEvents` +
  `parse_subject` + `StreamsHandler::with_notification_events`; the two-site registration
  (`add_core_services` + `connect_bridge`) + `CoreServiceSet` field; boot construction
  (`ExpoPushBackend` from `managed.json`/keychain) + the live `NotifySink` (drains the 407 recorder
  into `NotificationHandle::notify`) + live `read_inbox_summary` over `get_inbox`; `scripts/smoke.d/
  NN-notifications.sh` + manifest.

## Inputs to read before starting (507b)
- `tasks/v1.0/PHASE5_PLANNING.md` D9 (notification.events, two-site reg), §4.2.
- `crates/core/src/api_server.rs` (`add_core_services` + `CoreServiceSet`), `connect_bridge.rs`
  (mirror site), `boot.rs` (maestro handle construction precedent), `handlers/streams.rs`
  (`Subject::MaestroEvents`/`with_maestro_events` pattern), `maestro/tools/side.rs` (`NotifySink`
  swap), `maestro/tools/read.rs` (`read_inbox_summary`).

## Public interface this task locks
- 507a: `NotificationHandle` + `NotificationEvents`/`NotificationEvent`/`Clock`.
- 507b: the `Notifications` gRPC service + the `notification.events` subject (FROZEN, §4.2).

## Verification
**Tier 1.** 507a: `cargo clippy -p concerto-core --all-targets -- -D warnings` · `cargo test
-p concerto-core --test notifications_handle` (3) · `cargo fmt --all -- --check`. 507b adds: interface
regen (proto.md) · the two-site-registration integration test (a `CoreUnderTest` notifications_client
round-trip) · `scripts/smoke.sh --only notifications`.

## Definition of Done
- [x] 507a: `NotificationHandle` + notify() flow + 3 integration tests green; clippy/fmt clean
- [ ] 507b: gRPC service + two-site registration + boot + live notify_user/read_inbox_summary + smoke

## Outputs
- 507a: `crates/core/src/notifications/handle.rs` (new) + `notifications/mod.rs` (mod line) +
  `crates/persist/src/notifications.rs` (`clear_push_token`) + `crates/core/tests/notifications_handle.rs`
- 507b: `notifications.proto` (service) · `crates/core/src/handlers/notifications.rs` ·
  `handlers/{mod,streams}.rs` · `api_server.rs` · `connect_bridge.rs` · `boot.rs` ·
  `maestro/tools/{side,read}.rs` · `scripts/smoke.d/NN-notifications.sh` + manifest

## Commit message (507a)
```
phase-5: NotificationHandle + notify() orchestration (507a)

Adds the notifications sub-system handle: the notify() flow (de-dup ->
insert/update -> event emit -> push fan-out -> record delivered_at + null
stale tokens) + get_inbox/get_notification/mark_read/act_on_chip, decoupled
behind NotificationEvents + Clock seams. 3 integration tests (push+dedup,
opted-out-no-push, inbox-only). The gRPC service + boot wiring is 507b.

Refs: tasks/v1.0/507-notifications-grpc-service.md
```

## Handoff Notes (filled in when finishing)
- 507a landed the full notify() logic + reads; the build is green with NO wiring touched
  (`CoreServiceSet`/`boot`/proto-service untouched), so `main` stays green between a and b.
- 507b is the focused wiring pass (the highest-break-risk seams: two-site registration + boot). It
  reuses the maestro precedents verbatim (`with_maestro_events` → `with_notification_events`; the
  unimplemented-then-fill service pattern). The `NotifySink` swap drains 407's `NotifyRecorder`.
