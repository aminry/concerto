-- 0004_schedules.sql — Concerto V0.1 /loop primitive (Task 38).
--
-- Adds `schedules` and `schedule_runs` per `design/09 §4.3`, scoped down
-- for V0.1: only `kind='loop'` is supported on this surface. The cron-
-- based persistent scheduled tasks, cloud-task sync, promotion, and the
-- budget/jitter columns from design/09 §4.3 are V1.0 (Task 38 §"Scope —
-- out") and are intentionally omitted here so they can land in a future
-- numbered migration once the V1.0 surface is finalized.
--
-- Columns:
--
--   * `kind` is `CHECK (kind = 'loop')` in V0.1. V1.0 widens this to
--     `('loop','scheduled_task')` via a later migration; nothing on the
--     V0.1 read path reads beyond `'loop'`.
--   * `workarea_id` is `NOT NULL` because V0.1 loops are session-scoped
--     (`design/05 §3.1`). V1.0 scheduled_tasks may run unscoped; that
--     change rides along with the `kind` CHECK widening.
--   * `interval_seconds` is enforced 30..=604800 (7 days) at the Rust
--     layer (`SchedulerHandle::create_schedule`) — duplicating the
--     bounds at the SQL layer would silently catch driver bugs but adds
--     no semantic value vs the Rust-side validation. The frozen bounds
--     are documented in `tasks/38-scheduler-loop.md §"Public interface
--     this task locks"`.
--   * `expires_at` defaults to `now + 3 days` per design/05 §1; the
--     default is computed by the Rust layer (no SQLite expression here
--     so the value is observable in audit logs).
--   * `paused`, `agent_kind`, and `prompt` mirror the design/09 §4.3
--     schema directly. The `agent_kind` CHECK matches the
--     `sessions.agent_kind` CHECK set so a schedule cannot fire a
--     session with an unrepresentable kind.
--
-- `schedule_runs` records each fire's lifecycle. The columns are a
-- subset of design/09 §4.3 sized for V0.1:
--
--   * `session_id` is nullable because a fire that fails before the
--     supervisor's `start_session` returns has no session to point to
--     yet — `terminal_state = 'failed'` is set without a session FK.
--   * `terminal_state` is `NULL` while the run is in flight, then one
--     of `('completed','failed','crashed')` once the session emits its
--     terminal event (`TurnComplete` → completed; `Exited` with
--     non-zero / signal → crashed; any pre-handshake failure →
--     failed). V0.1 does not track tokens (`tokens_in`/`tokens_out`
--     columns from design/09 §4.3) — that's V1.0 budget territory.

CREATE TABLE schedules (
    id                  TEXT PRIMARY KEY,
    workarea_id         TEXT NOT NULL REFERENCES workareas(id) ON DELETE CASCADE,
    kind                TEXT NOT NULL CHECK (kind = 'loop'),
    interval_seconds    INTEGER NOT NULL,
    expires_at          INTEGER NOT NULL,
    last_run_at         INTEGER,
    paused              INTEGER NOT NULL DEFAULT 0 CHECK (paused IN (0,1)),
    prompt              TEXT NOT NULL,
    agent_kind          TEXT NOT NULL DEFAULT 'claude'
        CHECK (agent_kind IN ('claude','codex','gemini','maestro')),
    created_at          INTEGER NOT NULL
);

CREATE INDEX idx_schedules_workarea ON schedules(workarea_id);
CREATE INDEX idx_schedules_active
    ON schedules(expires_at)
    WHERE paused = 0;

CREATE TABLE schedule_runs (
    id                  TEXT PRIMARY KEY,
    schedule_id         TEXT NOT NULL REFERENCES schedules(id) ON DELETE CASCADE,
    session_id          TEXT REFERENCES sessions(id),
    started_at          INTEGER NOT NULL,
    ended_at            INTEGER,
    terminal_state      TEXT
        CHECK (terminal_state IS NULL
               OR terminal_state IN ('completed','failed','crashed'))
);

CREATE INDEX idx_schedule_runs_schedule
    ON schedule_runs(schedule_id, started_at);
CREATE INDEX idx_schedule_runs_inflight
    ON schedule_runs(schedule_id)
    WHERE ended_at IS NULL;
