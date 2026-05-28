# Task 09 — Initial DB Schema Migration

| Field | Value |
|---|---|
| Phase | 1 |
| Size | small (≤4h) |
| Depends on | 08 |
| Touches subsystem(s) | 09 (Persistence) |
| Smoke gate | unchanged |

## Goal
Add `migrations/0001_initial_schema.sql` defining the core entity tables: `projects`, `repositories`, `workspaces`, `workspace_repos`, `workareas`, `workarea_repos`, `chats`, `chat_messages`, `sessions`, `checkpoints`, `tool_approvals`, `devices`. After this task, fresh databases come up with the full V0.1 entity schema (minus tables like `schedules`, `skills_index`, `suggestion_learn`, `pull_requests` which arrive in their respective Phase 3 tasks).

## Inputs to read before starting
- `design/09_Persistence.md` §4.1 (core entities — the 3-level hierarchy + tables), §4.2 (sessions, checkpoints, tool_approvals), §4.4 (identity, devices).
- `design/09_Persistence.md` §3.1 (schema philosophy — normalize relations, JSON-blob agent stuff).
- `tasks/08-sqlite-migration-runner.md` → "Handoff Notes".

## Scope — in
Add `crates/persist/migrations/0001_initial_schema.sql` containing CREATE TABLE statements and indexes EXACTLY as defined in `design/09 §4.1`, `§4.2`, `§4.4`. Specifically:

- `projects`
- `repositories`
- `workspaces`
- `workspace_repos`
- `workareas`
- `workarea_repos`
- `chats`
- `chat_messages`
- `sessions`
- `checkpoints`
- `tool_approvals`
- `devices`

Include all indexes named in the design doc: `idx_workareas_status`, `idx_workareas_workspace`, `idx_chat_messages_chat`, `idx_sessions_workarea`, `idx_sessions_status`, `idx_sessions_yolo`, `idx_checkpoints_workarea`, `idx_devices_active`.

The migration file is a single SQL file, top-to-bottom, wrapped in `BEGIN; ... COMMIT;`.

Add an integration test at `crates/persist/tests/initial_schema.rs`:
- Opens a tempdir DB.
- Verifies every expected table exists (`SELECT name FROM sqlite_master WHERE type='table'`).
- Verifies every expected index exists.
- Inserts a representative row into each table (with sensible test fixtures) and reads it back.
- Verifies foreign key constraints fire (insert a `workareas` row with non-existent `workspace_id` → expect error).

Update `docs/interfaces/schema.md` via `./scripts/regen-interfaces.sh`.

## Scope — out
- Tables for `schedules`, `schedule_runs`, `skills_index`, `suggestion_learn`, `todos`, `pull_requests` — those land in their owning Phase 3 tasks (38, 39, 40, 45).
- Read/write helper functions per entity (Tasks 19, 20, 23 add these for the entities they need).
- Audit-log table (audit is JSONL on disk per `design/09 §3.5`).

## Public interface this task locks
- SQL: `crates/persist/migrations/0001_initial_schema.sql` is the FIRST migration. Its contents are FROZEN — no edits after merge. Schema changes go in subsequent `0002_*.sql`, `0003_*.sql` files.
- Column names exactly match `design/09 §4`. No renames without a revision task.

## Implementation notes
- Use `INTEGER` for unix-epoch-ms timestamps as the design doc specifies — not `TEXT`/`DATETIME`.
- UUIDv7 primary keys are TEXT — the application generates them; SQLite doesn't enforce the format.
- The `settings_json` columns store JSON as TEXT; do not use `JSON1`-backed `JSON` type alias (sqlx handles `TEXT` cleanly).
- Foreign keys with `ON DELETE CASCADE` per the design doc — required because `foreign_keys = on` (set in Task 08).
- The `CHECK ( (session_id IS NOT NULL) OR kind = 'maestro' )` constraint on `chats` is literal SQL — preserve it.

## Verification
1. `cargo build -p concerto-persist` → succeeds (migration file is embedded at build time).
2. `cargo test -p concerto-persist initial_schema` → all assertions pass.
3. `cargo run --bin concerto-core` boots; verify schema exists:
   ```
   sqlite3 ~/concerto/concerto.db ".tables"
   ```
   Expected output: at minimum the 12 tables listed in Scope — in. (Plus `_sqlx_migrations`.)
4. Foreign-key enforcement test passes (one of the integration test cases).
5. `./scripts/regen-interfaces.sh && git diff docs/interfaces/schema.md` → updated and committed.
6. Re-running `cargo run --bin concerto-core` on the existing DB is a no-op (migration already applied; idempotent).
7. `cargo clippy -p concerto-persist -- -D warnings` → clean.

## Definition of Done
- [ ] Verification commands pass.
- [ ] `docs/interfaces/schema.md` regenerated.
- [ ] No `TODO` / `FIXME` in SQL.
- [ ] Idempotent rerun verified.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/persist/migrations/0001_initial_schema.sql` (new)
- `crates/persist/tests/initial_schema.rs` (new)
- `docs/interfaces/schema.md` (regenerated)

## Commit message
```
phase-1: initial DB schema (0001)

Defines projects, repositories, workspaces, workspace_repos,
workareas, workarea_repos, chats, chat_messages, sessions,
checkpoints, tool_approvals, devices per design/09 §4. Schedules,
skills, todos, PRs land in Phase 3 tasks.

Refs: tasks/09-initial-db-schema.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** —
- **Smoke-gate state:** unchanged.
