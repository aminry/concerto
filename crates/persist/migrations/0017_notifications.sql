-- 0017_notifications.sql — Concerto V1.0 Notifications & Inbox root (Task 501).
--
-- Adds the two notification tables per `design/14 §4` (reconciled by Task 500 /
-- PHASE5_PLANNING §4.1). This is the persistence root of sub-system 14; the
-- inbox feed + de-dup (502), push fan-out (503/504), and the gRPC service +
-- `notification.events` (507) all build on it.
--
-- `notifications`
--   * `id` is the caller-allocated ULID PK (chronological-sortable, like the
--     other TEXT-id tables); `created_at` is the explicit ordering cursor for
--     the feed.
--   * `kind` is the snake_case notification kind (`design/14 §3.1`); CHECK-
--     constrained to the six V1.0 kinds. New kinds = a future CHECK-rewrite
--     (the 0010 `writable_schema` pattern), never a silent widen.
--   * `subject_kind` is the PHASE5_PLANNING D3 taxonomy:
--     `workspace | workarea | session | pull_request | schedule_run` (workarea
--     first-class; `session`, not `agent_session`).
--   * `workspace_id` / `workarea_id` / `session_id` are the optional scoping FKs,
--     each `ON DELETE CASCADE` so deleting a workspace/workarea/session reaps its
--     notifications (the inbox never dangles past its subject).
--   * `body` is short — full content is fetched via `Notifications.GetNotification`
--     (507). `chips_json` is the persisted top-3 `Chip` slate (suggestions.proto
--     shape; `ActOnChip` keys on `Chip.rule_id`, D4). `approval_json` is the
--     `ToolApprovalContext` for `tool_approval_needed` rows (NULL otherwise) so a
--     post-wakeup client can render+resolve without a second round-trip.
--   * `severity` CHECK `low | medium | high` (`design/14 §3.1`).
--   * `read_at` NULL ⇒ unread (drives the two partial unread indexes).
--   * `superseded_by` is the de-dup self-FK (502 updates the prior unread row's
--     body+`at` instead of inserting; `design/14 §3.7`).
--   * `action_taken` / `action_taken_at` / `action_taken_by_device_id` are the
--     DENORMALIZED first-wins UI marker (set AFTER the underlying
--     `Sessions.ResolveApproval` succeeds — the real guard is `tool_approvals`,
--     PHASE5_PLANNING D5).
--
-- `notification_deliveries` is the per-(notification, device) delivery ledger
--   (`delivered_at` = wakeup enqueued; `fetched_at` = body pulled over Iroh),
--   PK `(notification_id, device_id)`, `notification_id` `ON DELETE CASCADE`.
--
-- Indexes: the two partial unread indexes from `design/14 §4` (inbox-by-workarea
-- and inbox-by-workspace) + a `created_at` index for the chronological feed.
--
-- Forward-only, additive: two new tables + three indexes. No CHECK-widen, no
-- DROP. (Migration 0018 widens `devices.push_platform` for Expo — Task 503.)

CREATE TABLE notifications (
    id              TEXT PRIMARY KEY,                  -- ULID
    kind            TEXT NOT NULL CHECK (kind IN (
        'tool_approval_needed','agent_completed_with_message','agent_crashed',
        'pr_state_changed','check_run_failed','schedule_run_completed'
    )),
    subject_kind    TEXT NOT NULL CHECK (subject_kind IN (
        'workspace','workarea','session','pull_request','schedule_run'
    )),
    subject_id      TEXT NOT NULL,
    workspace_id    TEXT REFERENCES workspaces(id) ON DELETE CASCADE,
    workarea_id     TEXT REFERENCES workareas(id) ON DELETE CASCADE,
    session_id      TEXT REFERENCES sessions(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    body            TEXT NOT NULL,
    chips_json      TEXT,
    approval_json   TEXT,
    severity        TEXT NOT NULL CHECK (severity IN ('low','medium','high')),
    created_at      INTEGER NOT NULL,
    read_at         INTEGER,
    superseded_by   TEXT REFERENCES notifications(id),
    action_taken    TEXT,
    action_taken_at INTEGER,
    action_taken_by_device_id TEXT REFERENCES devices(id)
);

CREATE INDEX idx_notifications_inbox ON notifications(workarea_id, read_at) WHERE read_at IS NULL;
CREATE INDEX idx_notifications_workspace ON notifications(workspace_id, read_at) WHERE read_at IS NULL;
CREATE INDEX idx_notifications_created ON notifications(created_at);

CREATE TABLE notification_deliveries (
    notification_id TEXT NOT NULL REFERENCES notifications(id) ON DELETE CASCADE,
    device_id       TEXT NOT NULL REFERENCES devices(id),
    delivered_at    INTEGER,
    fetched_at      INTEGER,
    PRIMARY KEY (notification_id, device_id)
);
