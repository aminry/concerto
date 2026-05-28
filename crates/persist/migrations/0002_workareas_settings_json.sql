-- 0002_workareas_settings_json.sql — Add the `settings_json` column to
-- `workareas` (Task 30).
--
-- The 0001 migration is FROZEN per tasks/09; new fields land as forward
-- migrations. `workareas.settings_json` mirrors the `workspaces.settings_json`
-- column shape:
--
--   * TEXT NOT NULL DEFAULT '{}' — JSON blob, schema described in
--     design/03 §3.14 (`exclude_from_maestro`) and design/04 §3.12
--     (per-workarea deliberation/reasoning/personality defaults).
--   * Task 30 stamps `{"files_to_copy_applied": true}` on this column so
--     re-creating a workarea after a crash mid-create skips the resolver
--     idempotently (per `tasks/30 §Scope — in` last bullet).
--
-- ALTER TABLE … ADD COLUMN is well-supported by SQLite; the DEFAULT '{}'
-- backfills existing rows with the empty-object literal. No CHECK constraint
-- — JSON validation is a runtime concern (and applying a CHECK on the JSON
-- payload would require sqlite's `json_valid()` which is gated on the JSON1
-- extension that the design doc avoids relying on).

ALTER TABLE workareas
    ADD COLUMN settings_json TEXT NOT NULL DEFAULT '{}';
