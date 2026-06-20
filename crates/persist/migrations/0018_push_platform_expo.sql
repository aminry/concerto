-- 0018_push_platform_expo.sql — Widen `devices.push_platform` to add `'expo'`
-- + add `devices.dnd_until` (Task 503).
--
-- Phase 5 push delivery uses Expo Push (design/14 §3.6) wrapping APNs/FCM, so a
-- device's `push_platform` may now be `expo` in addition to `apns`/`fcm`.
-- Migration 0001's CHECK omits it:
--
--   push_platform IS NULL OR push_platform IN ('apns','fcm')
--
-- FROZEN widened value set (Task 503): NULL | apns | fcm | expo.
--
-- ## Why an in-place `sqlite_master` CHECK rewrite, NOT a recreate-table
--
-- Identical reasoning to migration 0010 (see its header): SQLite has no
-- `ALTER TABLE … ALTER CONSTRAINT`; the migrator runs each migration in a
-- transaction with `PRAGMA foreign_keys = ON`, so a recreate-table `DROP TABLE
-- devices` would fire `ON DELETE` on every child that references `devices(id)`
-- (`tool_approvals.decided_by_device_id`, and the Phase-5
-- `notifications.action_taken_by_device_id` / `notification_deliveries.device_id`)
-- — destroying or orphaning rows. Instead we edit ONLY the CHECK list via
-- `PRAGMA writable_schema`, leaving every column + the physical rows untouched,
-- then add the new nullable `dnd_until` column with a normal additive
-- `ALTER TABLE ADD COLUMN` (safe — no drop, no cascade).
--
-- `dnd_until` is a nullable unix-ms `INTEGER`: per-device Do-Not-Disturb floor
-- (design/14 §3.8 — push suppressed while `now < dnd_until`; the inbox still
-- receives). Consumed by Task 505's preference resolver.
--
-- The replacement CREATE TABLE text reproduces 0001's `devices` columns verbatim
-- with ONLY the `push_platform` CHECK widened; `dnd_until` is added separately
-- AFTER the rewrite so the rewritten schema text matches the 8 physical columns
-- the rewrite operates on.

PRAGMA writable_schema = ON;

UPDATE sqlite_master
SET sql = 'CREATE TABLE devices (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    public_key      BLOB NOT NULL,
    paired_at       INTEGER NOT NULL,
    last_seen_at    INTEGER,
    revoked_at      INTEGER,
    push_token      TEXT,
    push_platform   TEXT CHECK (push_platform IS NULL OR push_platform IN (''apns'',''fcm'',''expo''))
)'
WHERE type = 'table' AND name = 'devices';

PRAGMA writable_schema = RESET;

ALTER TABLE devices ADD COLUMN dnd_until INTEGER;
