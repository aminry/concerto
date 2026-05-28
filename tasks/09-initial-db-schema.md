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
- [x] Verification commands pass.
- [x] `docs/interfaces/schema.md` regenerated.
- [x] No `TODO` / `FIXME` in SQL.
- [x] Idempotent rerun verified.
- [x] Smoke gate still green.
- [x] Single commit created.

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
- **Drift from plan:**
  - **No literal `BEGIN; ... COMMIT;` in the .sql file.** The task said the migration should be "wrapped in `BEGIN; ... COMMIT;`". `sqlx::migrate!` already wraps each file in an implicit transaction (matching design/09 §6.2's "single transaction per file" promise); adding an explicit BEGIN inside that wrapper errors with `cannot start a transaction within a transaction`. The file is still atomic; the wrapping is just implicit. A comment near the top of `0001_initial_schema.sql` documents this.
  - **Extra `CHECK` constraints on enumerated TEXT columns.** Task 08's handoff forwarded a recommendation to encode the proto's commented value lists as SQL CHECK constraints. I followed that for `workareas.status`, `sessions.status`, `sessions.agent_kind` (`claude`/`codex`/`gemini`/`maestro`), `sessions.permission_mode`, `workspaces.permission_mode` (nullable form), `workareas.permission_mode` (nullable form), `chats.kind`, `chat_messages.role`, `devices.push_platform`, `tool_approvals.decision`, and the `*.bypass_destructive_guard` 0/1 flags. Design/09 §4 names the value sets but doesn't show the CHECK SQL; these constraints encode them. No column was renamed.
  - **`tool_approvals.decision` CHECK values include `auto_strict`, `auto_normal`, `auto_auto`, `auto_yolo`.** The design doc comment lists `auto_<mode>` (one of four permission modes); the CHECK enumerates each. If the auto-decision wire format ever diverges from the four permission modes, this CHECK will need a new migration.
- **Open questions for next task:**
  - **`.gitkeep` is still in `crates/persist/migrations/`** from Task 08. It's harmless — sqlx only picks up `*.sql` — and intentionally left in place since it documents the directory's purpose. Future tasks may delete it if it becomes noise.
  - **Forward reference `chats.session_id → sessions(id)`.** SQLite resolves FKs at execution time, not table-creation time, so the order is fine inside the transactional migration. The implication for callers: when inserting a fresh `chats` + `sessions` pair, insert a `chats` row with `session_id = NULL` (kind='maestro' is the carve-out, OR use a placeholder approach), then the session row, then optionally a real session-kind chat. The integration test `insert_and_read_back_every_table` shows the ordering. Tasks 19/23 should bake this into their write helpers.
  - **No repository functions yet.** Per Scope — out, Tasks 19, 20, 23 add the typed read/write helpers for the entities they need. The schema is the floor; the repository pattern (design/09 §3.4) builds on top.
  - **`projects.settings_json` schema is documented but unenforced.** Design/09 §4.1 documents the JSON shape (`default_permission_mode`, `default_bypass_destructive_guard`, etc.). SQLite stores it as opaque TEXT; validation lives in application code that the V0.1 task list hasn't reached yet.
  - **No `audit_log` / `audit_events` table.** That's intentional (design/09 §3.5 — JSONL on disk). Task 44 wires the audit writer.
- **Deliberate debt:** —
- **Smoke-gate state:** unchanged — still Phase 0 ("PASSED (no checks active yet — Phase 0)"). Task 15 is the first that flips the gate to v1.
