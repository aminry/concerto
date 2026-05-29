-- 0003_sessions_last_acked_seq.sql — Add the `last_acked_seq` column to
-- `sessions` (Task 36).
--
-- Per `tasks/36-pty-hot-reconnect.md §"Public interface this task locks"`,
-- the column is the persistent watermark of bytes the Core has consumed
-- from the agent-host's bridge ring buffer. On boot, `adopt_orphans`
-- reads this value and passes it as `HostFrame::Hello { last_seq }` so
-- the host replays only the unacked tail.
--
-- The column is `NOT NULL DEFAULT 0` so existing rows (sessions that
-- predate this migration) start at zero — equivalent to "Core never
-- acked anything", which is the same starting point a fresh session
-- gets and matches `HostFrame::Hello { last_seq: 0 }` first-connect
-- semantics. `cookie` and `host_socket` already exist on the row from
-- migration 0001, so no other columns are needed for hot reconnect.
--
-- ALTER TABLE … ADD COLUMN is the well-supported SQLite pattern; the
-- DEFAULT 0 backfills atomically.

ALTER TABLE sessions
    ADD COLUMN last_acked_seq INTEGER NOT NULL DEFAULT 0;
