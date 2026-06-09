-- 0005_skills_index.sql — Concerto V0.1 skills registry (Task 39).
--
-- Adds `skills_index` per `design/09 §4.5` (and `design/06 §4`), scoped
-- down for V0.1: discovery + per-(scope, workspace) enable/disable only.
-- The marketplace columns (`marketplace_id`, `pinned_version`,
-- `visibility`, `last_used_at`, `invocation_count`, `kind`) from
-- `design/06 §4` arrive with V1.0's marketplace surface in a later
-- numbered migration so the V0.1 wire shape stays small.
--
-- Updated 2026-06-08: re-scoped from project to workspace as part of
-- the Project→Workspace collapse (D5).
--
-- Columns:
--
--   * `scope` is one of `personal | workspace | plugin | enterprise`
--     (CHECK enforced). V0.1 actively discovers `personal` and
--     `workspace`; `plugin` / `enterprise` are stubs (the row shape is
--     locked here so a later migration doesn't break the FK).
--   * `workspace_id` is `NULL` for scopes that are not workspace-bound
--     (`personal`, `plugin`, `enterprise`); it MUST be set for
--     `scope='workspace'` rows. The FK references `workspaces(id)` so
--     deleting a workspace cascades the row.
--   * `name` is the skill's frontmatter `name` (or the skill directory
--     name when `name` is missing) — used as the human label and as the
--     uniqueness key alongside `scope` + `workspace_id`.
--   * `slash_command` is the optional `slash-command` frontmatter field;
--     V0.1 surfaces it but the Maestro/agent execution path for slash
--     commands is V1.0 per `tasks/39 §"Scope — out"`.
--   * `tools_json` is the JSON-encoded `tools: [..]` list from the
--     frontmatter (or `'[]'` when missing) so the UI can display the
--     skill's declared tool requirements without re-parsing YAML.
--   * `source_path` is the absolute path to the skill directory (the
--     parent of `SKILL.md`). The fs watcher in a later task uses it as
--     the soft-delete key when the directory disappears.
--   * `enabled` defaults to `1`; the toggle path flips it without
--     touching any other column so re-discovery preserves the user's
--     choice.
--   * `discovered_at` is unix-epoch milliseconds — written on every
--     upsert so the UI can render a "last seen" stamp without a
--     separate audit table.
--
-- The `UNIQUE(scope, workspace_id, name)` constraint matches the upsert
-- key: re-running discovery rewrites the existing row in place rather
-- than inserting a duplicate. SQLite treats `NULL != NULL` in UNIQUE
-- so the personal/plugin/enterprise scopes (workspace_id IS NULL) still
-- collapse correctly because `scope` differentiates them on those rows.

CREATE TABLE skills_index (
    id              TEXT PRIMARY KEY,
    scope           TEXT NOT NULL
        CHECK (scope IN ('personal','workspace','plugin','enterprise')),
    workspace_id    TEXT REFERENCES workspaces(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    slash_command   TEXT,
    description     TEXT,
    tools_json      TEXT NOT NULL DEFAULT '[]',
    source_path     TEXT NOT NULL,
    enabled         INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0,1)),
    discovered_at   INTEGER NOT NULL,
    UNIQUE(scope, workspace_id, name)
);

CREATE INDEX idx_skills_index_scope ON skills_index(scope);
CREATE INDEX idx_skills_index_workspace ON skills_index(workspace_id);
