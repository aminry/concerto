-- Migration 0014 — Task 319: PR-set semantics (merge_order + GraphQL handles).
--
-- Makes the implicit per-workarea PR set (the set of `pull_requests` rows
-- keyed by `workarea_id`, `design/13 §4`) a first-class *ordered* plan and
-- gives octocrab (313/316/320) the two GraphQL handles it needs per PR row.
--
-- Purely additive `ADD COLUMN` — the frozen 0008 `UNIQUE(workarea_id,
-- repository_id)` invariant and every shipped column stay untouched (no
-- table recreation).
--
--   * `merge_order` — the user-reorderable position of this PR within its
--     workarea's merge plan (`PHASE3_PLANNING.md D7`). Default = insertion
--     order (`max(merge_order)+1` per workarea), assigned by the caller on
--     first insert and PRESERVED across re-syncs (the upsert keeps it out of
--     `DO UPDATE SET`, like `created_at`). `SetMergeOrder` (Task 319) lets
--     the user reorder; the coordinated merge loop (Task 320) iterates in
--     `(merge_order, pr_number)` order, reverts in reverse. No
--     dependency-graph inference (that is R-6 / V2.0).
--   * `external_id` — the PR's GraphQL node id (octocrab needs it for the
--     review-thread / resolve mutations, Task 316). Refreshed on re-sync.
--   * `repository_full_name` — the `owner/repo` string the GraphQL endpoint
--     keys on (octocrab, Task 316). Refreshed on re-sync.
--
-- The latter two default to '' for rows created before Task 313 wires
-- octocrab — harmless; GraphQL paths only run for octocrab-backed repos that
-- carry them.

ALTER TABLE pull_requests ADD COLUMN merge_order INTEGER NOT NULL DEFAULT 0;
ALTER TABLE pull_requests ADD COLUMN external_id TEXT NOT NULL DEFAULT '';
ALTER TABLE pull_requests ADD COLUMN repository_full_name TEXT NOT NULL DEFAULT '';
