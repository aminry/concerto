-- Migration 0007 — Task 43: destructive-command intercept urgency flag.
--
-- Adds the `urgent` column to `tool_approvals` so the Agent Supervisor can
-- mark approvals that fired against the destructive-command pattern table
-- (`design/04 §3.10` + `design/12 §3.6`). Clients render urgent rows with
-- red styling; the audit log groups them under the "destructive" channel.
--
-- The column is `INTEGER NOT NULL DEFAULT 0` so existing rows backfill to
-- 0 (not urgent) without a separate UPDATE, and SQLite treats the boolean
-- as a 0/1 integer — the same convention used by every other boolean
-- column in the schema (`workareas.bypass_destructive_guard`, etc.).

ALTER TABLE tool_approvals ADD COLUMN urgent INTEGER NOT NULL DEFAULT 0;
