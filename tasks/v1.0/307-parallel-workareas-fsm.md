# Task 307 — Multiple Workareas per Workspace + the Full Workarea FSM (`finished` + `partial`)

| Field | Value |
|---|---|
| Phase | 3 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 306 |
| Touches subsystem(s) | 03 (Workspace/Session Manager), 09 (Persistence), 04 (Agent Supervisor — event source) |
| Smoke gate | unchanged |

## Goal
Drive the workarea through its **full status FSM** wired to live session events, and unblock **parallel workareas** (multiple on-disk attempts per workspace, e.g. `bach` + `mozart`). Multiple workareas per workspace already work structurally — composer allocation is `UNIQUE(workspace_id, composer_name)` and 306 removed the single-repo guards — but the FSM is inert: `crates/core/src/workspace_manager/fsm.rs` already defines the pure `transition(state, event)` graph **including `Finished`**, yet nothing calls it from session events, and the DB cannot store two of its states because the migration-0001 `workareas.status` CHECK omits **both** `finished` and `partial`. This task: (1) adds migration **0010** — a **recreate-table** migration widening the `workareas.status` CHECK to add `finished` + `partial` (SQLite cannot `ALTER` a CHECK); (2) adds `partial` to the `fsm.rs` `WorkareaState` enum + transition table (`partial` = a multi-repo workarea where ≥1 repo's `git worktree add` failed, per `design/03 §8`) and lets 306's create-loop mark a workarea `partial` instead of aborting the whole create on a single-repo failure; (3) wires `WorkareaManager::transition_workarea(id, event)` to load → `fsm::transition` → persist → broadcast → audit, driven by the Agent Supervisor's `Session*` events; (4) implements `pause_workarea` / `resume_workarea` (hard pause, `design/03 R-9`). After this task a session starting moves its workarea `active → running`, a tool-approval pause moves it `running → awaiting`, a clean finish moves it `→ finished`, and a partial multi-repo create persists as `partial`.

## Inputs to read before starting
- `design/03_Workspace_Session_Manager.md` §3.1 (the FSM diagram + "a workarea's effective status is derived from the union of its sessions' states"), §3.7 (archive/restore — already built by Task 31; restore resets to `active`), §6.5 (crash adoption on boot: probe `worktree_root`, missing/partial → `crashed`), §8 (the error table: **`git worktree add` fails for one of N repos → mark workarea `partial`**, retry the failing repo or abandon; setup-script failure keeps the workarea `active`).
- `design/04_Agent_Supervisor.md` §4.2 (the `AgentEvent` shapes the supervisor publishes) — these map to `WorkareaEvent::Session{Started,Awaiting,Resumed,Finished,Crashed}`; confirm the exact event names the supervisor's broadcast emits today before wiring.
- `crates/core/src/workspace_manager/fsm.rs` — the **already-present** pure FSM: `WorkareaState` (has `Finished`, lacks `Partial`), `WorkareaEvent`, `transition()` (the FROZEN `(state,event)->Result<state>` shape), `INVALID_TRANSITION_WIRE_CODE`, `as_sql`/`from_sql`, and the `ALL` arrays the table test iterates. **You add `Partial` to the enum + `as_sql`/`from_sql`/`ALL`, and the transitions into/out of it; you do not change the existing `transition` signature** (it is the locked surface). Note `as_sql`/`from_sql` already round-trip `Finished` — the only DB gap is the CHECK.
- `crates/persist/migrations/0001_initial_schema.sql` lines 109–127 — the `workareas` table: the CHECK is `status IN ('created','active','running','awaiting','paused','archived','crashed')` (line 116; **no `finished`, no `partial`**), the two indexes (`idx_workareas_status`, `idx_workareas_workspace`), the FK to `workspaces`, and `UNIQUE(workspace_id, composer_name)`. The recreate migration must reproduce **all** of these.
- `crates/persist/migrations/0002_workareas_settings_json.sql` — note the `settings_json` column added there; the recreated table must include it (the recreate copies **every** current column: `id, workspace_id, composer_name, branch_name, worktree_root, status, permission_mode, bypass_destructive_guard, created_at, archived_at, last_activity_at, settings_json`). Confirm the live column set by reading both 0001 + 0002 before writing the recreate.
- `crates/persist/src/workareas.rs` — `update_status` (line 104; the writer `transition_workarea` calls), `insert` (status starts `'created'`), `set_settings_json`, `list_all_non_archived` (line 234; used by crash adoption). The recreate-table migration is SQL-only; this file needs no signature change but its module-header CHECK-comment (line 12) must be updated to list the widened value set.
- `crates/core/src/workspace_manager/archive.rs` — `adopt_crashed_workareas` (the boot sweep; align it to emit the `AdoptCrashed` FSM event rather than a raw `update_status`).
- `crates/core/src/workspace_manager/workarea.rs` — `create_workarea` (306 made it loop all repos): the per-repo `git worktree add` failure point is where `partial` gets stamped; the existing `update_status(&mut tx, &id, "active")` at the end becomes conditional (`active` if all repos succeeded, `partial` if ≥1 failed).
- `crates/proto/proto/concerto/v1/workareas.proto` lines 28–40 — the `Workarea` message; line 34 carries the status doc-comment `// status ∈ { created | active | running | awaiting | paused | archived | crashed }`. **Update only that comment** to add `finished | partial` (NO field-number change; `status` stays a `string` at field 6, per the message's "FROZEN as of Task 20" + the design's "status may be promoted to an enum in a later revision" note — keep it a string).
- `crates/core/src/boot.rs` lines 530–599 — how `WorkareaManagerActor` + the Agent Supervisor handle are constructed and wired (`with_agent_supervisor`); the FSM-driving subscription to `session.events` is wired here.
- `tasks/v1.0/PHASE3_PLANNING.md` §3 (migration reservation: **0010 = this task**, recreate `workareas` to widen the status CHECK; confirm 0009 from Task 306 landed and 0010 is the next free number — if Phase-2 shifted the block, shift accordingly and note it) + §2 row 307 ("**both** `finished` + `partial` added; `partial` = a multi-repo workarea where ≥1 repo's `git worktree add` failed").
- `tasks/v1.0/306-multi-repo-workspaces.md` → "Handoff Notes" — the multi-repo create loop + the whole-create-rollback-on-failure behavior this task softens into `partial`.

## Scope — in
- **Migration 0010** (`crates/persist/migrations/0010_workareas_status_finished_partial.sql`): a **recreate-table** migration (SQLite cannot `ALTER` a CHECK constraint). Inside one transaction: `PRAGMA foreign_keys=OFF;` (or rely on the migration runner's wrapping tx), `CREATE TABLE workareas_new (… status TEXT NOT NULL CHECK (status IN ('created','active','running','awaiting','paused','finished','partial','archived','crashed')) …)` reproducing **every** column + the FK + `UNIQUE(workspace_id, composer_name)`, `INSERT INTO workareas_new SELECT … FROM workareas;`, `DROP TABLE workareas;`, `ALTER TABLE workareas_new RENAME TO workareas;`, then recreate `idx_workareas_status` + `idx_workareas_workspace`. Header comment cites this task + the two new values.
- **`fsm.rs`**: add `WorkareaState::Partial` to the enum, `as_sql` (`"partial"`), `from_sql`, and the `ALL` array (becomes length 9). Add transitions: `Created → Partial` is NOT a state event (it's set inside create like `Active`); from `Partial` allow `SessionStarted → Running`, `Archive → Archived`, `AdoptCrashed → Crashed`, and a **`SessionResumed`/retry-success → Active`** path if the failing repo is later materialized (or keep `Partial` terminal-until-retry and document it). The `Finished` transitions already exist — verify the table test now exercises them. Keep the `transition` fn signature + `INVALID_TRANSITION_WIRE_CODE` unchanged.
- **`WorkareaManager::transition_workarea(id, WorkareaEvent) -> Result<Workarea>`** (the handle method `design/03 §5.1` names — note the design lists it as `transition_workarea(id, Status)`; implement it taking the **event** and computing the new status via `fsm::transition`, which is the safer contract). Body: load current status → `WorkareaState::from_sql` → `fsm::transition(state, event)` (map `Err(Validation(INVALID_TRANSITION_WIRE_CODE …))` → `FAILED_PRECONDITION`, never a crash) → `workareas::update_status(new.as_sql())` → broadcast `WorkareaEvent::StatusChanged` on `workarea.events` → audit-log the transition.
- **Drive it from session events**: subscribe the Workarea Manager to the Agent Supervisor's session-event stream (wired in `boot.rs`); map `SessionStarted → SessionStarted` (`active/finished/crashed → running`), `SessionAwaiting → awaiting`, `SessionResumed → running`, `SessionFinished → finished` (only when **all** sessions on the workarea have ended — derive from the union, `design/03 §3.1`), `SessionCrashed → crashed`.
- **`pause_workarea` / `resume_workarea`**: pause stops all sessions on the workarea (via the Agent Supervisor) then `transition_workarea(Pause)` → `paused` (hard pause, `R-9`); resume `transition_workarea(Resume)` → `active` (cold-resume of sessions is the user's next action, not auto).
- **Stamp `partial` in create** (`workarea.rs`): when 306's per-repo loop has ≥1 `git worktree add` failure, instead of aborting the whole create, persist the successfully-materialized repos' `workarea_repos` rows and set the workarea status to `partial` (not `active`), recording the failing repo ids in `workarea.events` so the UI/retry can target them.
- **Crash adoption** (`archive.rs`): route the boot sweep's crash marking through `transition_workarea(AdoptCrashed)` so it audits + broadcasts like every other transition.
- Tests (Tier 1): extend `crates/core/tests/fsm_table.rs` to cover every `(state, event)` pair including `Partial` + the persistence round-trip of `finished` + `partial` (insert → `update_status("finished")` → read back, proving the widened CHECK); two parallel workareas on one workspace (distinct composers, independent status); a session-event sequence driving `active → running → awaiting → running → finished`; a 2-repo create with one repo's worktree-add stubbed to fail → workarea persists `partial`.

## Scope — out
- **Multiple sessions per workarea + the per-workarea edit mutex** — Task 308 (this task drives the FSM from **session events** but does not own the multi-session cardinality or the write-serialization mutex).
- **`exclude_from_maestro` toggle** — Task 311 (a `settings_json` key + RPC, no FSM involvement).
- **Branch-rename hook + `suggest_workarea_branch_name`** — Task 312.
- **Promoting `workareas.status` from `string` to a proto enum** — explicitly deferred by the `Workarea` message's "status may be promoted to an enum in a later revision" note; keep it a string + widen the comment only.
- **Retrying the failed repo of a `partial` workarea** (the actual re-`worktree add`) — surface the seam + the failing-repo event here; the user-driven retry RPC can be a thin follow-on (note it in Handoff if not built).
- **Desktop parallel-workareas + multi-agent tabs UI** — Task 323.

## Public interface this task locks
- **Migration 0010 — widened `workareas.status` CHECK (FROZEN):** the full value set is now `created | active | running | awaiting | paused | finished | partial | archived | crashed`. The recreate-table preserves every column, the FK to `workspaces(id)`, `UNIQUE(workspace_id, composer_name)`, and both status/workspace indexes.
- **`fsm.rs` `WorkareaState::Partial` (FROZEN):** SQL form `"partial"`; semantics = a multi-repo workarea with ≥1 repo whose `git worktree add` failed (`design/03 §8`). `WorkareaState::ALL` is now length 9; the `transition` fn signature + `INVALID_TRANSITION_WIRE_CODE` are unchanged (still the locked surface from Task 31).
- **`WorkareaManager::transition_workarea(WorkareaId, WorkareaEvent) -> Result<Workarea>` (FROZEN):** the single funnel for every status change — loads, applies `fsm::transition`, persists via `update_status`, broadcasts `StatusChanged` on `workarea.events`, audits. Illegal transitions → `FAILED_PRECONDITION` carrying `INVALID_TRANSITION_WIRE_CODE`.
- **`Workarea` proto `status` (UNCHANGED field, widened doc-comment):** still `string status = 6`; the comment now enumerates the two new values. No new field number; no new RPC (status flows on the existing `workarea.events` stream).

## Implementation notes
- **Recreate-table is the migration trap.** SQLite has no `ALTER TABLE … ALTER CONSTRAINT`. Copy **every** live column (read 0001 **and** 0002 — `settings_json` is easy to miss), preserve the FK + UNIQUE + both indexes, and run it all inside the migration's transaction. A column you drop here silently loses data on existing installs — enumerate them explicitly, don't `SELECT *` into a differently-shaped table.
- **`transition_workarea` must never panic on a bad transition.** A `running` workarea getting a stray `SessionStarted` is a no-op-or-rejection, not a crash. Map the FSM's `Err(Validation(INVALID_TRANSITION_WIRE_CODE))` to a typed `FAILED_PRECONDITION` and log at debug; the union-of-sessions derivation (`§3.1`) means you sometimes recompute the same state (idempotent re-apply is fine).
- **Union-of-sessions for `finished`.** `SessionFinished` should only transition the workarea to `finished` when **no** other session on the workarea is still live (`sessions WHERE workarea_id=? AND ended_at IS NULL` is empty — reuse `sessions::list_live_ids_by_workarea`). With multi-session (308) this matters; with one session it reduces to today's behavior.
- **Reuse, don't rebuild, the FSM.** `fsm::transition` + `as_sql`/`from_sql` + the `ALL` arrays + the table test are the Task-31 contract. You are extending the enum and wiring callers — resist rewriting the graph.
- **`partial` is reachable only via create.** No `Session*` event produces it; it is stamped in `create_workarea` like `active`. The table test should assert no `(state, SessionEvent) → Partial` edge exists.
- **Cross-platform.** Recreate-table SQL is portable; the session-event wiring is pure Rust. Builds on the Windows/Linux CI lanes (Task 113).
- **Regen.** Migration 0010 ⇒ `./scripts/regen-interfaces.sh` updates `docs/interfaces/schema.md`; the widened proto comment updates `docs/interfaces/proto.md`. Commit both.

## Verification
Tier 1.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core fsm` + `cargo test -p concerto-core workarea` → the widened `fsm_table.rs` covers all 9×10 `(state,event)` pairs; `finished` + `partial` persistence round-trips; the session-event-driven sequence; the `partial`-on-partial-create test; two parallel workareas isolated.
4. `cargo test -p concerto-persist workareas` → the recreate migration preserves columns/FK/UNIQUE/indexes on a fixture DB seeded before the migration runs.
5. `cargo test --workspace --no-fail-fast` → all pass.
6. `cargo deny check` → green (no new deps).
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit (`schema.md` widened CHECK; `proto.md` widened status comment).
8. `scripts/smoke.sh` → **unchanged gate** (the existing `workspace-workarea` + `echo-session` checks still pass; the FSM wiring must not regress the single-session happy path).

**Tier-1 scope.** The FSM is pure + table-testable; session-event wiring is provable with a stubbed/echo agent. The Tier-3 reality this gestures at — crash-injection at every create step recovering cleanly (`design/03 §10` crash row) — is corroborated at the Phase-3 manual checklist, not gated here.

## Definition of Done
- [x] Migration 0010 widens the `workareas.status` CHECK to add `finished` + `partial`; preserves every column + FK + UNIQUE + both indexes; persistence test proves `finished` + `partial` round-trip (NOTE: implemented as an in-place `PRAGMA writable_schema` CHECK rewrite instead of the prescribed DROP/RENAME recreate-table — the DROP would cascade-delete every child row under the FKs-ON migration runner; see Handoff *Drift*)
- [x] `fsm.rs` gains `WorkareaState::Partial` (+ `as_sql`/`from_sql`/`ALL`) and its transitions; `transition` signature + `INVALID_TRANSITION_WIRE_CODE` unchanged; table test exercises all 90 pairs
- [x] `transition_workarea` funnels every status change (load → fsm → persist → broadcast → audit); illegal → `FAILED_PRECONDITION`
- [x] Session events drive `active→running→awaiting→running→finished` (`finished` only when no live session remains); `pause_workarea`/`resume_workarea` (hard pause)
- [x] `create_workarea` stamps `partial` (not `active`) when ≥1 repo's worktree-add fails, recording failing repo ids on `workarea.events`; crash adoption routes through `transition_workarea(AdoptCrashed)`
- [x] `Workarea` proto status comment widened (no field/number change); no new proto field
- [x] No `TODO`/`FIXME`/`unimplemented!()`/`todo!()` in new code (deliberate seams in Handoff)
- [x] No files outside Outputs modified (one mechanical-drift addition: `crates/core/src/handlers/streams.rs` — see Handoff *Drift*)
- [x] Interfaces regenerated + committed (`schema.md`, `proto.md`)
- [x] Smoke gate green (unchanged)
- [x] Single commit with the message below

## Outputs
- `crates/persist/migrations/0010_workareas_status_finished_partial.sql` (new — in-place `writable_schema` CHECK widen; see Handoff *Drift* re: not DROP/RENAME)
- `crates/persist/src/workareas.rs` (modified — module-header CHECK comment widened; no signature change)
- `crates/core/src/workspace_manager/fsm.rs` (modified — `Partial` + transitions)
- `crates/core/src/workspace_manager/workarea.rs` (modified — `transition_workarea`, `apply_session_event`, `spawn_session_fsm_pump`, `pause`/`resume`, `partial` stamping, `StatusChanged`/`PartialCreate` events, crash-adoption funnel)
- `crates/core/src/workspace_manager/archive.rs` (modified — `list_missing_worktree_workareas` probe; crash adoption marking moved to the FSM funnel)
- `crates/core/src/boot.rs` (modified — wire the session-event subscription that drives the FSM)
- `crates/core/src/handlers/streams.rs` (modified — DRIFT: two new `WorkareaEvent` arms in `map_workarea_event`, forced by the widened broadcast enum; no wire/proto change)
- `crates/proto/proto/concerto/v1/workareas.proto` (modified — widened `status` doc-comment only)
- `crates/core/tests/fsm_table.rs` (modified — full 90-pair table + `Partial` + no-session-event-produces-partial assertion)
- `crates/core/tests/parallel_workareas_fsm.rs` (new — in-process FSM funnel / session-event / parallel-workarea / pause-resume tests)
- `crates/core/tests/workarea_lifecycle.rs` (modified — `partial_create_when_one_repo_worktree_add_fails` subprocess test)
- `crates/persist/tests/workareas_status_widen.rs` (new — migration-0010 widen + data/child-FK preservation tests)
- `docs/interfaces/schema.md` + `docs/interfaces/proto.md` (regenerated)

## Commit message
```
phase-3: parallel workareas + full workarea FSM (finished + partial)

Wires fsm::transition into the Workarea Manager (transition_workarea
funnel) driven by Agent Supervisor session events, adds pause/resume,
and widens workareas.status via recreate-table migration 0010 to add
finished + partial. partial = a multi-repo workarea where >=1 repo's
worktree add failed. Two parallel workareas per workspace now run
independently.

Refs: tasks/v1.0/307-parallel-workareas-fsm.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan:**
  - **Migration 0010 is an in-place `PRAGMA writable_schema` CHECK rewrite, NOT the prescribed DROP/RENAME recreate-table.** The textbook recreate is *unsafe* here: the migration runner connection has `foreign_keys = ON` (`crates/persist/src/api.rs base_connect_options`) and sqlx-sqlite's migrator **always** wraps each migration in its own transaction and **ignores the `-- no-transaction` directive on SQLite** (verified in `sqlx-sqlite-0.8.6/src/migrate.rs::apply`). `PRAGMA foreign_keys` is a no-op inside a transaction, so it can't be disabled for the DROP — and with FKs on, `DROP TABLE workareas` performs an implicit DELETE that fires `ON DELETE CASCADE` on every child (`workarea_repos`, `sessions`, `checkpoints`, `pull_requests`, `tool_approvals`), silently destroying their rows on any populated install. Neither `PRAGMA defer_foreign_keys` nor `PRAGMA legacy_alter_table` suppresses that cascade (verified empirically on SQLite 3.51). The `writable_schema` edit reaches the **identical FROZEN end-state** — same columns, FK, `UNIQUE(workspace_id, composer_name)`, both indexes, widened CHECK — without dropping the table, so no child cascade can fire. `workareas_status_widen.rs::migration_0010_preserves_seeded_rows_and_child_fk` proves a seeded row + child `workarea_repos` row survive (FKs ON at connect). The frozen public interface (the widened value set) is unchanged; only the *mechanic* differs.
  - **`crates/core/src/handlers/streams.rs` edited (outside the original Outputs).** Mechanical drift: `map_workarea_event` is an exhaustive `match` on the `WorkareaEvent` broadcast enum, which gained `StatusChanged` + `PartialCreate`. Added two arms mapping them onto the existing `ProtoWorkareaEvent { workarea_id, kind }` wire shape (`kind = "status:<to>"` / `"partial"`) — no proto/field change. Added to Outputs.
  - **`schema.md`'s 0010 section renders empty.** The regen only extracts `CREATE TABLE`/`CREATE INDEX` statements; 0010 has neither (it `UPDATE`s `sqlite_master`), so the generated section is a header with no body — the deterministic, correct output for a non-CREATE migration. The widened CHECK is documented in the migration file + the persist module-header comment + the proto comment.
- **Open questions for next task:**
  - **Retry of a `partial` workarea's failed repo is a seam, not built (Scope — out).** `create_workarea` records the failing `repository_id`s on `WorkareaEvent::PartialCreate` and the FSM allows `Partial --SessionResumed--> Active` as the retry-success promotion. The actual re-`worktree add` + `workarea_repos` insert + the `Workareas`-service RPC that drives it is a thin follow-on (a good home is alongside 308's multi-session work or a dedicated retry RPC). Today the partial workarea is fully usable for the materialized repos (a session can start → `running`).
  - **The cone seed for a partial create only covers materialized repos.** Failed repos get no `workarea_repos` row at all, so 302/305's cone resolver simply won't see them until a retry inserts the row. That's the intended shape; flagging it for the retry author.
  - **Task 308 (multi-session)** consumes the union-of-sessions `finished` logic already implemented here (`apply_session_event` only transitions to `finished` when `list_live_ids_by_workarea` is empty). 308 owns the per-workarea edit mutex + multi-session cardinality; the FSM funnel + session pump are ready for >1 session per workarea.
- **Deliberate debt:** — (none; no `TODO`/`FIXME`/`todo!()`/`unimplemented!()` in new code).
- **Smoke-gate state:** unchanged + green. `scripts/smoke.sh` PASSED end-to-end; `workspace-workarea` + `echo-session` (the single-session happy path the FSM wiring must not regress) both PASS, confirming `active → running → finished` via live session events does not break the existing flow.
