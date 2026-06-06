-- 0010_workareas_status_finished_partial.sql — Widen the
-- `workareas.status` CHECK to add `finished` + `partial` (Task 307).
--
-- The full workarea status FSM (design/03 §3.1,
-- crates/core/src/workspace_manager/fsm.rs) has always defined `Finished`,
-- and Task 307 adds `Partial` (a multi-repo workarea where ≥1 repo's
-- `git worktree add` failed, design/03 §8). Both must be storable in
-- `workareas.status`, but migration 0001's CHECK constraint omits them:
--
--   status IN ('created','active','running','awaiting','paused','archived','crashed')
--
-- FROZEN widened value set (Task 307):
--   created | active | running | awaiting | paused | finished | partial | archived | crashed
--
-- ## Why an in-place `sqlite_master` CHECK rewrite, NOT a recreate-table
--
-- SQLite has no `ALTER TABLE … ALTER/DROP CONSTRAINT`. The textbook recipe
-- is recreate-table (new table + copy + DROP old + rename). That recipe is
-- UNSAFE for `workareas` in *this* persistence layer:
--
--   * the migration runner connection has `PRAGMA foreign_keys = ON`
--     (crates/persist/src/api.rs `base_connect_options`), and
--   * sqlx-sqlite's migrator ALWAYS wraps each migration in its own
--     transaction (it ignores the `-- no-transaction` directive on
--     SQLite), and
--   * `PRAGMA foreign_keys` is a no-op inside a transaction, so it cannot
--     be turned off for the duration of the recreate.
--
-- With foreign keys ON, `DROP TABLE workareas` performs an implicit DELETE
-- that fires the `ON DELETE CASCADE` on every child table that references
-- `workareas(id)` — `workarea_repos`, `sessions`, `checkpoints`,
-- `pull_requests`, `tool_approvals` — silently destroying their rows on
-- any existing install with live workareas. Neither `PRAGMA
-- defer_foreign_keys` nor `PRAGMA legacy_alter_table` suppresses that
-- cascade (verified empirically on SQLite 3.51).
--
-- Instead we edit the table's stored schema text directly via
-- `PRAGMA writable_schema`, swapping ONLY the CHECK list. This reaches the
-- identical FROZEN end-state — every column, the FK to `workspaces(id)`,
-- the `UNIQUE(workspace_id, composer_name)` constraint, the
-- `permission_mode`/`bypass_destructive_guard` CHECKs, and both indexes
-- (`idx_workareas_status`, `idx_workareas_workspace`) are unchanged because
-- the table is never dropped — WITHOUT touching a single row, so no child
-- cascade can fire. The replacement SQL below reproduces 0001's base
-- columns + 0002's `settings_json` column verbatim with only the `status`
-- CHECK widened. SQLite bumps the schema cookie on COMMIT so other
-- connections re-read the new definition; a post-migration `quick_check`
-- (run by `Persistence::open`) verifies the rewritten schema is valid.
--
-- The trailing `PRAGMA writable_schema = RESET` reloads the in-memory
-- schema on the migrator's own connection (so the SAME connection that
-- ran the edit immediately enforces the widened CHECK, not just freshly
-- opened ones) and clears the writable_schema flag.

PRAGMA writable_schema = ON;

UPDATE sqlite_master
SET sql = 'CREATE TABLE workareas (
    id                          TEXT PRIMARY KEY,
    workspace_id                TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    composer_name               TEXT NOT NULL,
    branch_name                 TEXT NOT NULL,
    worktree_root               TEXT NOT NULL,
    status                      TEXT NOT NULL CHECK (status IN (
        ''created'',''active'',''running'',''awaiting'',''paused'',''finished'',''partial'',''archived'',''crashed''
    )),
    permission_mode             TEXT CHECK (permission_mode IS NULL OR permission_mode IN (''strict'',''normal'',''auto'',''yolo'')),
    bypass_destructive_guard    INTEGER CHECK (bypass_destructive_guard IS NULL OR bypass_destructive_guard IN (0,1)),
    created_at                  INTEGER NOT NULL,
    archived_at                 INTEGER,
    last_activity_at            INTEGER,
    settings_json               TEXT NOT NULL DEFAULT ''{}'',
    UNIQUE(workspace_id, composer_name)
)'
WHERE type = 'table' AND name = 'workareas';

PRAGMA writable_schema = RESET;
