-- 0015_maestro_state.sql — Concerto V1.0 Maestro budget + lifecycle root (Task 403).
--
-- Adds `maestro_state` per `design/08 §4.1`. This is the FIRST daily-counter
-- / budget table in the codebase: `schedules` (migration 0004) deliberately
-- deferred its `tokens_in`/`tokens_out`/`daily_budget_tokens` columns (see the
-- doc comment in `crates/persist/src/schedules.rs`), so there is no precedent
-- to copy — this migration establishes the pattern.
--
-- Columns:
--
--   * `id` is `INTEGER PRIMARY KEY CHECK (id = 1)` — the singleton mechanism.
--     The PK + CHECK together mean exactly one row can ever exist (`id = 1`);
--     there is no sentinel column and no UNIQUE index, because a one-row table
--     needs neither. `INSERT OR IGNORE INTO maestro_state (id, ...) VALUES (1,
--     ...)` is the idempotent bootstrap.
--   * `daily_in_today` / `daily_out_today` are the cumulative-across-backends
--     token counters (Task 412). `DEFAULT 0`; bumped additively in SQL
--     (`SET x = x + ?`) so concurrent writer-mutex bumps never clobber, and
--     zeroed together on reset.
--   * `budget_resets_at` is `NOT NULL` unix-ms `INTEGER`: the next instant the
--     daily counters reset (UTC-midnight or manual, Task 412). The bootstrap
--     supplies the first value.
--   * `last_digest_at` is a nullable unix-ms `INTEGER`: the digest-cadence
--     cursor (Task 414). NULL until the first digest is produced.
--   * `enabled` is a `0/1` boolean (`INTEGER NOT NULL DEFAULT 1`): the
--     Maestro on/off flag (Task 414 `set_enabled` / `enterpriseDataPrivacy`
--     disable). Mapped `i64 != 0 → bool` in Rust.
--
-- The singleton `chats(kind='maestro', session_id NULL)` row that anchors the
-- Maestro chat history is bootstrapped in Rust (`maestro_state::ensure_maestro_chat`),
-- NOT here: it already validates against migration 0001's
-- `CHECK (kind IN ('session','maestro'))` + `CHECK ((session_id IS NOT NULL)
-- OR kind='maestro')`, so no DDL is required for it.
--
-- No `CREATE INDEX` (a one-row singleton needs none), no CHECK-widen, no DROP —
-- purely additive and forward-only.

CREATE TABLE maestro_state (
    id              INTEGER PRIMARY KEY CHECK (id = 1),  -- singleton
    daily_in_today  INTEGER NOT NULL DEFAULT 0,
    daily_out_today INTEGER NOT NULL DEFAULT 0,
    budget_resets_at INTEGER NOT NULL,
    last_digest_at  INTEGER,
    enabled         INTEGER NOT NULL DEFAULT 1
);
