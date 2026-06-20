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

### 507b progress (this session)
- **507b-1 DONE (5546877):** `notification.events` subject (`Subject::NotificationEvents` +
  `parse_subject` + `with_notification_events` + `map_notification_event`) + `NotificationStreamEvent`
  carrier + `notifications::events::{to_frame, NotificationEventSender, channel}` bridge. Streams +
  events tests green.
- **507b-2 DONE (9716b03):** `Notifications` gRPC service in `notifications.proto`
  (GetInbox/GetNotification/MarkRead/ActOnChip/UpdateWorkspaceSettings) + `handlers/notifications.rs`
  (delegates to `NotificationHandle`) + `NotificationHandle::set_workspace_opt_out`. Compiles; not
  registered.
- **507b-3 PREP DONE (12bad56):** `NotificationHandle::with_event_channel()` + `events_sender()`
  accessors (the live producer seam).

### 507b-3 remaining wiring sequence (the deterministic plan — execute as ONE focused pass, build after each step)
The handle must be built ONCE in boot + shared so the `Notifications` service, the `notification.events`
producer, and the maestro `notify_user` sink all use the SAME `notification.events` channel. Mirror the
`maestro_handle` threading exactly. Touch order:
1. **boot.rs (~before the ApiServer spawn, ~1115):** `let notification_handle = Some(NotificationHandle::new(Arc::clone(&persistence), Arc::new(crate::notifications::push::ExpoPushBackend::new(None)), Arc::new(crate::notifications::handle::NoEvents)).with_event_channel());` + `let factory_notification_handle = notification_handle.clone();`.
2. **api_server.rs `CoreServiceSet`:** add `pub notifications: Option<NotificationHandle>` (NO cfg) + `notifications: None` in `runtime_only`.
3. **api_server.rs `ApiServerActor` + `with_managers`:** add a `notifications` param (mirror `maestro`) → store on the actor → thread into `run_uds` + the bridge `BridgeServices`.
4. **api_server.rs `run_uds`:** add `notifications` param → set it in the `481` `CoreServiceSet` literal.
5. **boot.rs `serve_iroh` literal (~1199/1216):** add `notifications: notification_handle.clone(),`.
6. **api_server.rs `add_core_services` destructure (~646) + apply chain:** register `NotificationsServer::new(NotificationsHandler::new(n.clone()))` (cross-platform, near devices/repositories) when `notifications` is `Some`; inside the `#[cfg(unix)]` streams block add `if let Some(tx) = notifications.as_ref().and_then(|n| n.events_sender()) { handler = handler.with_notification_events(tx); }`.
7. **connect_bridge.rs `BridgeServices` (~182) + `serve`:** add the `notifications` field + register the service + the streams producer (D9 site 2). *(May be deferred to Task 520 when the web bridge is first exercised — note it if so.)*
8. **507b-ii — maestro `notify_user` live sink:** in boot, build a `NotificationHandle`-backed `NotifySink` (the SAME `notification_handle`) and pass it where 407 wired `NotifyRecorder` (`maestro/tools` boot site); make `read_inbox_summary` (`maestro/tools/read.rs`) call `notification_handle.get_inbox(...)`.
9. **507b-iii — smoke:** add `scripts/smoke.d/NN-notifications.sh` (create via the handle → `GetInbox` over loopback → `ActOnChip`) + append to `scripts/smoke.manifest`.
Verify: full `cargo test -p concerto-core` (esp. `auth_middleware`/`device_revocation`/`pairing` + a new `CoreUnderTest` notifications round-trip) + `cargo check --workspace` + `scripts/smoke.sh --only notifications`.
