# Task 502 — SQLite inbox feed + 5-min de-dup window + retention

| Field | Value |
|---|---|
| Phase | 5 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium |
| Depends on | 501 |
| Touches subsystem(s) | 14 (Notifications), 09 (Persistence) |
| Smoke gate | unchanged |

## Goal
Add the de-duplication engine + the retention policy on top of 501's tables: the persist-side de-dup
key lookup + in-place body refresh, and the pure [`dedup::decide`] decision + retention floor so the
`notify()` path (507) refreshes a live duplicate instead of inserting a new row (and sends no second
wakeup), per `design/14 §3.7`.

## Inputs to read before starting
- `tasks/v1.0/PHASE5_PLANNING.md §2` (502 row), `design/14 §3.7` (de-dup), §3.9 R-9 (retention).
- `crates/persist/src/notifications.rs` (501) + `crates/core/src/notifications/model.rs`.

## Scope — in
- `crates/persist/src/notifications.rs`: `find_unread_for_dedup_key` (workarea- or workspace-scoped
  key, within-window, unread, non-superseded, newest), `update_body_and_at` (the de-dup refresh),
  `count_older_than` (retention reporting).
- `crates/core/src/notifications/dedup.rs`: `DEDUP_WINDOW_MS` (5 min), `RETENTION_DAYS` (90),
  `retention_floor_ms`, `DedupDecision`, the pure `decide(existing, now, window)` + unit tests.
- Tests: persist de-dup query (matches same-key unread within window; ignores read / other-workarea /
  out-of-window) + `update_body_and_at`; core dedup unit tests (synthetic time).

## Scope — out
- Wiring de-dup into a live `notify()` (507). Push fan-out (504). The scheduler-driven archival cron
  (P6). Per-workspace window override (505 reads `settings_json`).

## Public interface this task locks
- `find_unread_for_dedup_key`/`update_body_and_at`/`count_older_than` + `dedup::{decide, DedupDecision,
  DEDUP_WINDOW_MS, RETENTION_DAYS, retention_floor_ms}`.

## Verification
**Tier 1.** `cargo clippy -p concerto-persist -p concerto-core --all-targets -- -D warnings` ·
`cargo test -p concerto-persist --test notifications` (8) · `cargo test -p concerto-core --lib
notifications::dedup` (4) · `cargo fmt --all -- --check`. No proto/schema change ⇒ no regen.

## Definition of Done
- [x] de-dup key query + in-place refresh + retention floor; pure `decide` unit-tested
- [x] clippy/fmt clean; persist + core tests green
- [x] Single commit with the message below

## Outputs
- `crates/persist/src/notifications.rs` (modified — 3 fns) · `crates/core/src/notifications/dedup.rs`
  (new) + `crates/core/src/notifications/mod.rs` (mod line) · `crates/persist/tests/notifications.rs`
  (modified — de-dup test)

## Commit message
```
phase-5: notification inbox de-dup window + retention floor

Adds the de-dup key lookup + in-place body refresh (persist) and the pure
DedupDecision + 90-day retention floor (core), per design/14 §3.7/§3.9. The
live notify() that uses them is Task 507.

Refs: tasks/v1.0/502-inbox-feed-dedup.md
```

## Handoff Notes (filled in when finishing)
- `decide` takes the persist row so 507 calls `find_unread_for_dedup_key` then `decide`.
- Retention is kept-not-deleted (R-9): `count_older_than` reports; no deletion in V1.0; cron is P6.
