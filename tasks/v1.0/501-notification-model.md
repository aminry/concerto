# Task 501 — Notification model + `notifications`/`notification_deliveries` tables + 6 kinds (FROZEN proto messages)

| Field | Value |
|---|---|
| Phase | 5 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium |
| Depends on | 500 |
| Touches subsystem(s) | 14 (Notifications), 09 (Persistence) |
| Smoke gate | unchanged |

## Goal
Lay the Phase-5 contract-first root for sub-system 14: freeze the notification **wire message
types** + the **persistence schema** so 502–507 + the inbox UI (523) build against a stable shape.
Adds `notifications.proto` (messages + enums only — the `Notifications` *service* is Task 507),
migration `0017` (`notifications` + `notification_deliveries`), the `crates/persist/src/notifications.rs`
CRUD, and the `crates/core/src/notifications/` module with the typed domain model (kind/subject/severity
enums + DB⇄proto mapping + `NotifyRequest`). No actor, handle, push, or service yet.

## Inputs to read before starting
- `tasks/v1.0/PHASE5_PLANNING.md` §4.1 (FROZEN messages), D3 (subject taxonomy), D4 (chips reuse `Chip`).
- `design/14_Notifications_Push.md` §3.1 (6 kinds + default severities), §4 (data model).
- `crates/persist/src/tool_approvals.rs` — the multi-row table-module template.
- `crates/persist/migrations/0015_maestro_state.sql` — the migration header/style precedent.
- `crates/proto/proto/concerto/v1/suggestions.proto:29` — the `Chip` reused via import.

## Scope — in
- `crates/proto/proto/concerto/v1/notifications.proto`: `NotificationKind` (6), `NotificationSubjectKind`
  (D3 5-value), `ToolApprovalContext`, `Notification` (the canonical inbox/fetch row, superset of
  design's `NotificationPayload`), `NotificationDelivery`, `InboxFilter`. `import suggestions.proto` →
  `repeated Chip chips`. Timestamps `int64` unix-ms. **No service.** FROZEN.
- `crates/persist/migrations/0017_notifications.sql`: both tables + the two partial unread indexes +
  a `created_at` feed index. `kind`/`subject_kind`/`severity` CHECKs; `superseded_by` self-FK; the
  scoping FKs `ON DELETE CASCADE`; `approval_json` for tool-approval rows.
- `crates/persist/src/notifications.rs`: `NewNotification`/`NotificationRow`/`NewDelivery`/`DeliveryRow`
  + `insert`/`get`/`list_inbox`/`mark_read`/`set_action_taken`/`set_superseded`/`upsert_delivery`/
  `list_deliveries`. Registered in `lib.rs`.
- `crates/core/src/notifications/{mod,model}.rs`: typed `NotificationKind`/`SubjectKind`/`Severity`
  (DB⇄proto + `default_severity` per §3.1), `NotificationId`, `NotifyRequest`, `row_to_proto`.
  Registered in `crates/core/src/lib.rs`.
- Tests: `crates/persist/tests/notifications.rs` (schema, round-trip, inbox/unread filter, idempotent
  mark-read/action, CHECK rejection, FK cascade, delivery upsert+cascade) + `model.rs` unit tests
  (DB round-trips, severity defaults).

## Scope — out
- The `Notifications` gRPC service + `NotificationHandle` + `notification.events` (507).
- Push/`PushBackend`/`WakeupPayload` fields (503); fan-out/first-wins (504); `ActOnChip`/prefs (505);
  de-dup engine + retention (502); privacy property test (506).

## Public interface this task locks
- **`notifications.proto` messages + enums** (field numbers + enum values FROZEN, PHASE5_PLANNING §4.1).
- **Migration `0017`** schema (the two tables; CHECK sets are the locked taxonomies).
- The `crates/persist/notifications` CRUD signatures + the `core::notifications::model` enums/mapping.

## Implementation notes
- DB stores snake_case strings (`tool_approval_needed`, `workarea`, `high`); the proto uses enums;
  `core/model.rs` is the single mapping seam. `Chip` is imported, not copied (cross-proto imports are
  established: `streams.proto` imports `suggestions.proto`).
- First-wins guard is `tool_approvals` (D5); `notifications.action_taken` is a denormalized marker —
  502/505 own its semantics, 501 just provides the idempotent `set_action_taken` column + helper.

## Verification
**Tier 1.** `cargo check -p concerto-proto -p concerto-persist -p concerto-core` · `cargo clippy
-p concerto-persist -p concerto-core --all-targets -- -D warnings` · `cargo fmt --all -- --check` ·
`cargo test -p concerto-persist --test notifications` (7) · `cargo test -p concerto-core --lib
notifications::model` (3) · `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/`
(commit the proto.md + schema.md regen). Smoke gate unchanged.

## Definition of Done
- [x] `notifications.proto` messages/enums FROZEN; persist + core modules compile
- [x] Migration 0017 + persist CRUD + 7 integration tests green
- [x] core model enums + mapping + 3 unit tests green
- [x] clippy/fmt clean; interfaces regenerated + committed
- [x] No service/handle/push (those are 502–507)
- [x] Single commit with the message below

## Outputs
- `crates/proto/proto/concerto/v1/notifications.proto` (new)
- `crates/persist/migrations/0017_notifications.sql` (new)
- `crates/persist/src/notifications.rs` (new) + `crates/persist/src/lib.rs` (mod line)
- `crates/persist/tests/notifications.rs` (new)
- `crates/core/src/notifications/{mod,model}.rs` (new) + `crates/core/src/lib.rs` (mod line)
- `docs/interfaces/{proto,schema}.md` (regen)

## Commit message
```
phase-5: notification model + tables (0017) + frozen proto messages

Contract-first root for sub-system 14: notifications.proto messages/enums
(6 kinds, D3 subject taxonomy, ToolApprovalContext, Notification, InboxFilter;
Chip reused via import), migration 0017 (notifications + notification_deliveries
+ indexes), the persist CRUD, and the core notifications domain model (typed
kind/subject/severity + DB<->proto mapping + NotifyRequest). No service yet (507).

Refs: tasks/v1.0/501-notification-model.md
```

## Handoff Notes (filled in when finishing)
- Added `approval_json` column (beyond design §4) so a post-wakeup client renders the tool-approval
  without a second round-trip; `Notification.approval` mirrors it.
- `Notification` is one message serving both `GetInbox` and `GetNotification` (design's
  `NotificationPayload` is a subset). 507's service returns it directly.
- `list_inbox` excludes `superseded_by IS NOT NULL` rows; 502's de-dup sets `superseded_by`.
- 502 next: de-dup query (`find_unread_for_dedup_key`), 5-min window, retention/archive helper.
