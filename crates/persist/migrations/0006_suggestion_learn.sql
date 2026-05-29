-- 0006_suggestion_learn.sql — Concerto V0.1 suggestion-engine table (Task 40).
--
-- Adds `suggestion_learn` per `design/09 §4.5`. V0.1 does NOT write to this
-- table — the rule engine ships without a learning loop (per `design/07 §2`
-- "rule engine only" row and `tasks/40 §"Scope — out"`). The table is created
-- here so V1.0's learning loop (Maestro-style accept/dismiss weighting) can
-- land behind the existing `Suggestions.RecordSuggestionOutcome` RPC stub
-- without a wire-format break.
--
-- Columns:
--
--   * `id` is a UUIDv7 string (TEXT PRIMARY KEY) matching the convention
--     used across every other V0.1 table.
--   * `workarea_id` is nullable: a `RecordSuggestionOutcome` call from a
--     Maestro-level chip (no workarea context) writes NULL. When set, the
--     FK references `workareas(id)` so deleting a workarea cascades the
--     row.
--   * `rule_id` is the static rule identifier (`context_window_50`, ...) —
--     the six V0.1 IDs are listed in `tasks/40 §"Scope — in"` and are
--     reserved namespace; new rules use new IDs without rewriting the
--     column type.
--   * `outcome` is a free-form short string (`accept | dismiss | snooze`
--     plus future `acted_upon`). V0.1 does not CHECK the set so V1.0
--     learning experiments can add values without a migration.
--   * `context_hash` is a stable hash of the chip-emission context (the
--     subset of `WorkareaState` the rule consulted). V0.1 writes `''`
--     when the engine is in rule-only mode; V1.0 will start populating it
--     for the bucketed weighting in `design/07 §6.2`.
--   * `created_at` is unix-epoch milliseconds (caller-supplied).
--
-- Indices match the V1.0 read patterns: list-by-workarea and a bucket
-- lookup by `(rule_id, context_hash)`. They are cheap on a table that
-- never has more than a few thousand rows per user-decade.

CREATE TABLE suggestion_learn (
    id            TEXT PRIMARY KEY,
    workarea_id   TEXT REFERENCES workareas(id) ON DELETE CASCADE,
    rule_id       TEXT NOT NULL,
    outcome       TEXT NOT NULL,
    context_hash  TEXT NOT NULL DEFAULT '',
    created_at    INTEGER NOT NULL
);

CREATE INDEX idx_suggestion_learn_workarea ON suggestion_learn(workarea_id);
CREATE INDEX idx_suggestion_learn_rule ON suggestion_learn(rule_id, context_hash);
