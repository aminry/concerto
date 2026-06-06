-- 0009_workspace_repos_position.sql — Add the deterministic `position`
-- ordinal to `workspace_repos` (Task 306).
--
-- Migration 0001 is FROZEN per tasks/09; new fields land as forward
-- migrations. `workspace_repos` has been N-capable since 0001 (the
-- `PRIMARY KEY (workspace_id, repository_id)` already permits many repos
-- per workspace); the only gap was a *deterministic, stable* repo order.
--
--   * position INTEGER NOT NULL DEFAULT 0 — the per-`(workspace_id)`
--     0-based ordinal. `workspaces::update_repos` assigns
--     `position = slice index` (insertion order = declaration order =
--     merge/UI order); `workspaces::list_repos` returns rows ordered by
--     `(position, repository_id)`. FROZEN by Task 306.
--   * Backfill: existing single-repo workspaces have one row each; the
--     `DEFAULT 0` correctly stamps them position 0 (they are the only /
--     first repo).
--   * This is the ordering Task 309's reference repo ("first by
--     position") and the stable multi-repo UI (Task 322) key off — do
--     NOT re-derive repo order from `repository_id` after this task.
--
-- A plain `ALTER TABLE … ADD COLUMN … DEFAULT 0` (the 0002 precedent)
-- backfills correctly: no recreate-table, no CHECK to widen (contrast
-- Task 307's `workareas.status` widen). The `(workspace_id, repository_id)`
-- PK is unchanged.

ALTER TABLE workspace_repos
    ADD COLUMN position INTEGER NOT NULL DEFAULT 0;

-- Ordered-read index for `list_repos` (and Task 309's first-by-position
-- reference-repo lookup).
CREATE INDEX idx_workspace_repos_position ON workspace_repos(workspace_id, position);
