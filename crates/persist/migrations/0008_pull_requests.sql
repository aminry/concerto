-- Migration 0008 — Task 45: VCS Provider Integration (V0.1, `gh` CLI).
--
-- Adds the `pull_requests` table that the VCS Provider Integration
-- (`design/13`) uses to cache per-(workarea, repository) PR state. The
-- schema mirrors `design/09 §4.5` with the V0.1 column subset frozen by
-- `tasks/45-vcs-gh-cli.md` §"Pre-decisions".
--
-- Canonical state lives on GitHub; this table is a cache that:
--   * The UI reads from for fast workarea-PR list rendering without a
--     synchronous `gh` round-trip on every panel open.
--   * `Vcs.GetPullRequest` upserts on every read, so the cached row
--     reflects the latest `view_pr` JSON response.
--
-- `UNIQUE(workarea_id, repository_id)` enforces the V0.1 invariant that
-- a workarea has at most one PR per repository — single-repo workareas
-- naturally satisfy this; the multi-repo extension in V1.0 keeps the
-- same key.
--
-- `provider` is a free-form TEXT column with `'github'` as the only
-- V0.1 value; V2.0 adapters (`gitlab`, `bitbucket`) plug into the same
-- column without a schema change.
--
-- `pr_number`, `head_sha`, `state`, `title`, `body`, `url`, `base_ref`,
-- `head_ref` mirror the `gh pr view --json` fields the VCS module
-- reads (`gh_cli::view_pr`). Timestamps follow the workspace convention
-- (INTEGER unix epoch milliseconds; `crates/persist/src/api.rs`).

CREATE TABLE pull_requests (
    id              TEXT PRIMARY KEY,
    workarea_id     TEXT NOT NULL REFERENCES workareas(id) ON DELETE CASCADE,
    repository_id   TEXT NOT NULL REFERENCES repositories(id),
    provider        TEXT NOT NULL,
    pr_number       INTEGER NOT NULL,
    base_ref        TEXT NOT NULL,
    head_ref        TEXT NOT NULL,
    state           TEXT NOT NULL,
    title           TEXT NOT NULL,
    body            TEXT NOT NULL DEFAULT '',
    url             TEXT NOT NULL DEFAULT '',
    head_sha        TEXT NOT NULL DEFAULT '',
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL,
    UNIQUE(workarea_id, repository_id)
);

CREATE INDEX idx_pull_requests_workarea ON pull_requests(workarea_id);
CREATE INDEX idx_pull_requests_repo ON pull_requests(repository_id);
