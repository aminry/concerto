# Task 403 — `maestro_state` table (migration `0015`) + `chats(kind='maestro')` singleton bootstrap + budget accessor (the first daily-counter/budget table; FROZEN per PHASE4_PLANNING §4.6 / §3)

| Field | Value |
|---|---|
| Phase | 4 |
| Task type | rust |
| Verification tier | 1 |
| Size | small (≤4h) |
| Depends on | — |
| Touches subsystem(s) | 09 (Persistence), 08 (Maestro) |
| Smoke gate | unchanged |

## Goal
Lock the **persistence root of the Maestro budget + lifecycle state** — the singleton `maestro_state` table and the find-or-create of the singleton `chats(kind='maestro')` row — so that Phase-4's token-counting (Task 412), digest cadence (Task 414), and daily-history condensation (Task 410) have a typed, round-trippable home with **zero schema rework** when they wire. Today there is **no maestro state at all**: there is no `maestro_state` table (highest migration on `main` is `crates/persist/migrations/0014_pull_requests_merge_order.sql`), `schedules` (migration `0004`) deliberately deferred its budget/token columns (`tokens_in`/`tokens_out`/`daily_budget_tokens` — see `crates/persist/src/schedules.rs` doc comment) so **there is no daily-counter precedent to copy**, and while migration `0001` already provisions `chats(kind IN ('session','maestro'))` with the `CHECK ((session_id IS NOT NULL) OR kind='maestro')` carve-out (`crates/persist/migrations/0001_initial_schema.sql:151`), **no code ever inserts the maestro singleton row**. This task ships migration `0015_maestro_state.sql` creating `maestro_state(id, daily_in_today, daily_out_today, budget_resets_at, last_digest_at, enabled)` **EXACTLY** per `design/08 §4.1`, the typed free-async-fn accessors in a new `crates/persist/src/maestro_state.rs` (`get` singleton, `bump_daily_counters`, `reset_budget`, `set_last_digest`, `set_enabled`) over `&mut SqliteConnection` (writes) / `&SqlitePool` (reads), the `MaestroState`/`NewMaestroState` row structs in `crates/persist/src/api.rs`, a `pub mod maestro_state;` line in `crates/persist/src/lib.rs`, and an idempotent `ensure_maestro_chat`-style bootstrap that inserts the `kind='maestro', session_id NULL` chat row if absent (no schema change — the row already validates against the `0001` CHECK). These accessors + the `0015` schema are **FROZEN (PHASE4_PLANNING §4.6, D6)**. After this task, **Task 412** reads/writes the budget counters via these accessors (cumulative-across-backends counting, inert-on-exhaust, UTC-midnight/manual reset), **Task 414** reads `enabled`/`last_digest_at`, and **Task 410** writes daily-summary `chat_messages` against the maestro chat id this task bootstraps. What stays out of this task: the actual token *counting* logic, the in-memory `TokenBudget`/`MaestroState` runtime struct (`design/08 §4.2` — a different type), and any Maestro module code — those are Tier-2/Tier-1 work owned by 412/414 and are **not** verified here.

## Inputs to read before starting
- `tasks/v1.0/PHASE4_PLANNING.md` §4.6 — **AUTHORITATIVE.** "`maestro_state` + budget accessor + `chats(kind='maestro')` singleton — FROZEN by 403 (D6). The 0015 schema, the typed accessors (`get`-singleton, bump-daily-counters, reset-budget, set-last-digest, set-enabled), and the singleton-bootstrap. 412 consumes the budget; 410 consumes the maestro chat id; 414 reads `enabled`/digest state." This row is the entire task.
- `tasks/v1.0/PHASE4_PLANNING.md` §3 — **AUTHORITATIVE migration reservation.** `0015` = 403's `maestro_state`; the singleton `chats(kind='maestro')` bootstrap **needs no schema change** (row validates against `0001`); **CHECK-widening is BANNED** (neither `0015` nor the bootstrap touches a CHECK). **Author check (do this first):** confirm the actual highest `crates/persist/migrations/NNNN_*.sql` on `main` is still **`0014`**; if a migration landed above `0014`, **shift `0015`→`0015+offset`** preserving order and **note it in your Handoff** (the `0016`/410 row shifts with you).
- `tasks/v1.0/PHASE4_PLANNING.md` §1 (D6) — token accounting is net-new; `AgentEvent::ContextUsage{pct}` is **NOT** the carrier; this table is. Budget is **cumulative across backends** (`design/08 §3.9`).
- `tasks/v1.0/PHASE4_PLANNING.md` §8.1 (403 write-set) — your write-set + the hard seam you share: **410** (the migrations dir + `api.rs`/`lib.rs`). Migration `0016` (`chat_messages.metadata`) is **410's** — **do NOT author it here**; you collide on the migrations dir, so the orchestrator serializes 403 before 410.
- `design/08_Maestro_Agent.md` §4.1 — the canonical `CREATE TABLE maestro_state (...)` block. Transcribe the columns/constraints **verbatim**. (§4.2's `MaestroState`/`TokenBudget` in-memory struct is a **different**, non-persisted type — NOT yours.)
- `crates/persist/migrations/0001_initial_schema.sql:151` — the `chats` table: `kind TEXT NOT NULL CHECK (kind IN ('session','maestro'))` + `CHECK ((session_id IS NOT NULL) OR kind='maestro')`. The maestro singleton is the `kind='maestro', session_id NULL` row — confirm it validates **without** a schema change.
- `crates/persist/migrations/0004_schedules.sql` — the migration-file house style (header comment explaining columns + constraints; `CHECK` syntax; `INTEGER` unix-ms). The doc comment notes `tokens_in`/`tokens_out` were deferred — that is **why** `maestro_state` is the first daily-counter table.
- `crates/persist/src/schedules.rs` — the **accessor pattern to mirror EXACTLY**: free `pub async fn` over `&mut SqliteConnection` (writes) / `&SqlitePool` (reads), `.map_err(|e| Error::Sqlx(Box::new(e)))`, a private `row_to_*` projector. Copy this shape.
- `crates/persist/src/api.rs` (`NewChat` ~line 568, `NewSchedule`/`Schedule` ~line 687) — where the `MaestroState`/`NewMaestroState` row structs go + the `NewChat` shape the bootstrap reuses (`{id, session_id: Option<String>, kind, created_at}`).
- `crates/persist/src/lib.rs` — declare `pub mod maestro_state;` (alongside `pub mod schedules;`) and re-export `MaestroState`/`NewMaestroState` from the `pub use api::{...}` block.
- `crates/persist/tests/initial_schema.rs` — `EXPECTED_TABLES` (line 38) + `insert_and_read_back_every_table` (line 116) + the read-back `counts` vec (line 336): add `maestro_state` to all three so the schema test covers it.

## Scope — in
- **`crates/persist/migrations/0015_maestro_state.sql`** (new):
  - A header comment in the `0004` house style: this is the **first daily-counter/budget table**; `schedules` deferred its token columns so there is no precedent; the singleton pattern (`id INTEGER PRIMARY KEY CHECK (id = 1)`) means exactly one row; `budget_resets_at`/`last_digest_at` are unix-ms `INTEGER`; `enabled` is a `0/1` boolean. Note that the `chats(kind='maestro')` singleton is bootstrapped in Rust (no DDL here — it validates against `0001`).
  - The table transcribed **verbatim** from `design/08 §4.1` (see Public interface). **No** `CREATE INDEX` (a one-row singleton needs none). **No** CHECK-widen, **no** `DROP`.
- **`crates/persist/src/maestro_state.rs`** (new) — free async fns mirroring `schedules.rs`:
  - `get(pool: &SqlitePool) -> Result<Option<MaestroState>>` — fetch the `id=1` singleton (read).
  - `bump_daily_counters(conn: &mut SqliteConnection, in_delta: i64, out_delta: i64) -> Result<()>` — `UPDATE maestro_state SET daily_in_today = daily_in_today + ?, daily_out_today = daily_out_today + ? WHERE id = 1` (412's cumulative-across-backends counter; additive so concurrent bumps don't clobber).
  - `reset_budget(conn: &mut SqliteConnection, budget_resets_at: i64) -> Result<()>` — zero both counters + set the next `budget_resets_at` (the UTC-midnight/manual reset).
  - `set_last_digest(conn: &mut SqliteConnection, last_digest_at: i64) -> Result<()>` — patch `last_digest_at` (414's digest cadence).
  - `set_enabled(conn: &mut SqliteConnection, enabled: bool) -> Result<()>` — patch `enabled` (414's `set_enabled`; `enterpriseDataPrivacy`-disable).
  - An **upsert/bootstrap** for the singleton — `ensure_initialized(conn: &mut SqliteConnection, budget_resets_at: i64) -> Result<()>` doing `INSERT OR IGNORE INTO maestro_state (id, budget_resets_at) VALUES (1, ?)` (defaults fill `daily_in_today=0`/`daily_out_today=0`/`enabled=1`/`last_digest_at=NULL`). Idempotent: re-running on an existing row is a no-op (it does **not** clobber live counters). `get` returning `None` means "never initialized" → caller bootstraps.
  - A private `row_to_maestro_state(row) -> MaestroState` projector (`enabled` mapped `i64 != 0 → bool`).
- **`crates/persist/src/maestro_state.rs` — the `chats(kind='maestro')` singleton bootstrap:**
  - `ensure_maestro_chat(conn: &mut SqliteConnection, id: &str, created_at: i64) -> Result<()>` — insert the `kind='maestro', session_id NULL` chat row **only if absent** (`INSERT … WHERE NOT EXISTS (SELECT 1 FROM chats WHERE kind='maestro')`, or `INSERT OR IGNORE` keyed on the caller-supplied `id`). Reuses the `NewChat` shape semantics (`session_id: None`, `kind: "maestro"`). **No schema change** — the row validates against `0001`'s CHECK. Returns the maestro chat id (or accepts it) so 410 can attach daily summaries.
- **`crates/persist/src/api.rs`** (modified) — add `pub struct MaestroState { pub id: i64, pub daily_in_today: i64, pub daily_out_today: i64, pub budget_resets_at: i64, pub last_digest_at: Option<i64>, pub enabled: bool }` (+ a `NewMaestroState` if the bootstrap signature wants one; the upsert above can take scalars directly — keep it minimal). Mirror the `Schedule` derive set (`#[derive(Debug, Clone, PartialEq, Eq)]`).
- **`crates/persist/src/lib.rs`** (modified) — `pub mod maestro_state;` + add `MaestroState` (and `NewMaestroState` if present) to the `pub use api::{...}` re-export block.
- **`crates/persist/tests/initial_schema.rs`** (modified) — add `"maestro_state"` to `EXPECTED_TABLES`; in `insert_and_read_back_every_table` insert the singleton (`INSERT INTO maestro_state (id, budget_resets_at) VALUES (1, ?)`) and add `("maestro_state", ...)` to the read-back assertion (note: its PK is `INTEGER id=1`, not a `TEXT` id — read it back via a dedicated `SELECT id FROM maestro_state WHERE id = 1` scalar rather than forcing it into the `String`-id `counts` vec). Assert the `CHECK(id = 1)` rejects `id=2`.
- **Tests (Tier 1):** in `crates/persist/tests/` (or `#[cfg(test)]` in `maestro_state.rs`):
  - migration applies on a fresh DB (the schema test's `every_expected_table_exists` now covers `maestro_state`).
  - `ensure_initialized` then `get` → singleton with `daily_in_today=0`, `enabled=true`; a second `ensure_initialized` is a no-op (counters/`enabled` unchanged after a `bump`/`set_enabled`).
  - `bump_daily_counters(100, 20)` twice → `daily_in_today=200`, `daily_out_today=40` (additive, cumulative).
  - `reset_budget(new_ts)` → both counters `0`, `budget_resets_at=new_ts`.
  - `set_last_digest` / `set_enabled(false)` round-trip via `get`.
  - `CHECK(id = 1)` rejects an insert with `id=2`.
  - `ensure_maestro_chat` twice → exactly **one** `chats` row with `kind='maestro'`; the row has `session_id IS NULL` and validates (no FK/CHECK error).

## Scope — out
- **Token counting / parsing the CLI or Direct-API usage** — **Task 412** (it consumes `bump_daily_counters`/`reset_budget`, decides the 200K/50K caps, and implements inert-on-exhaust + the UTC-midnight reset clock). This task only freezes the *storage* + accessors; it never counts a token.
- **The in-memory `MaestroState`/`TokenBudget` runtime struct** (`design/08 §4.2`: `summary_cache`, `daily_budget: TokenBudget`, `pending_routings`) — that lives in the **maestro module** (401/412), is **not persisted**, and is a distinct type from the persisted row struct named here. Do not build it.
- **`chat_messages.metadata` column + migration `0016`** — **Task 410** (the daily-condensation pass + `role_extra='daily_summary'` tagging). **You collide on the migrations dir**, so the orchestrator serializes 403 before 410; do NOT author `0016` here.
- **Writing/reading maestro `chat_messages` rows** (the actual chat history) — **Task 410/414** against the chat id this task bootstraps. 403 only ensures the parent `chats` row exists.
- **Calling `ensure_initialized`/`ensure_maestro_chat` at Core boot** — the boot wiring (`crates/core/src/boot.rs`) is **414**'s (it constructs the `MaestroHandle`). 403 ships the accessor; nothing in `crates/core` is touched here.
- **Real-world (Tier-3):** budget-exhaust-goes-inert-while-routing-still-works and the digest-after-absence cadence are demonstrated at the **Phase-4 manual checklist** ("confirm budget-exhaust goes inert while routing still works") once 412/414 wire these accessors live — not provable by this persistence-only task.

## Public interface this task locks
- **SQL (FROZEN, `design/08 §4.1` / PHASE4_PLANNING §3 + §4.6) — `crates/persist/migrations/0015_maestro_state.sql`:**

```sql
CREATE TABLE maestro_state (
    id              INTEGER PRIMARY KEY CHECK (id = 1),  -- singleton
    daily_in_today  INTEGER NOT NULL DEFAULT 0,
    daily_out_today INTEGER NOT NULL DEFAULT 0,
    budget_resets_at INTEGER NOT NULL,
    last_digest_at  INTEGER,
    enabled         INTEGER NOT NULL DEFAULT 1
);
```

- **Rust accessors (FROZEN, PHASE4_PLANNING §4.6) — `crates/persist/src/maestro_state.rs` (free async fns, mirroring `schedules.rs`):**

```rust
/// Fetch the `id = 1` singleton (read path). `None` ⇒ never initialized.
pub async fn get(pool: &SqlitePool) -> Result<Option<MaestroState>>;

/// Idempotently create the singleton with defaults if absent (INSERT OR
/// IGNORE on id = 1). Never clobbers live counters. Call before first use.
pub async fn ensure_initialized(conn: &mut SqliteConnection, budget_resets_at: i64) -> Result<()>;

/// Additive cumulative-across-backends counter bump (Task 412).
pub async fn bump_daily_counters(conn: &mut SqliteConnection, in_delta: i64, out_delta: i64) -> Result<()>;

/// Zero both counters + set the next reset instant (UTC-midnight / manual, Task 412).
pub async fn reset_budget(conn: &mut SqliteConnection, budget_resets_at: i64) -> Result<()>;

/// Patch the digest-cadence cursor (Task 414).
pub async fn set_last_digest(conn: &mut SqliteConnection, last_digest_at: i64) -> Result<()>;

/// Enable/disable the Maestro (Task 414 set_enabled / enterpriseDataPrivacy gate).
pub async fn set_enabled(conn: &mut SqliteConnection, enabled: bool) -> Result<()>;

/// Bootstrap the singleton `chats(kind='maestro', session_id NULL)` row if
/// absent. No schema change — validates against the 0001 CHECK. Idempotent.
pub async fn ensure_maestro_chat(conn: &mut SqliteConnection, id: &str, created_at: i64) -> Result<()>;
```

- **Rust row struct (FROZEN, PHASE4_PLANNING §4.6) — `crates/persist/src/api.rs`:**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaestroState {
    pub id: i64,                       // always 1
    pub daily_in_today: i64,
    pub daily_out_today: i64,
    pub budget_resets_at: i64,         // unix ms
    pub last_digest_at: Option<i64>,   // unix ms; None until first digest
    pub enabled: bool,                 // INTEGER 0/1 mapped to bool
}
```

- **CONSUMES (do NOT re-lock):** the `chats` table + its `CHECK (kind IN ('session','maestro'))` / `CHECK ((session_id IS NOT NULL) OR kind='maestro')` are frozen by migration `0001` (Task 09). The maestro singleton row is an **insert**, not a schema change. `NewChat` (`api.rs`) is the existing chat insert shape this task's bootstrap reuses.

## Implementation notes
- **Transcribe the `design/08 §4.1` DDL byte-for-byte.** `id INTEGER PRIMARY KEY CHECK (id = 1)` is the singleton enforcement — do not add a separate sentinel column or a UNIQUE index; the PK + CHECK is the entire mechanism. This is the **load-bearing decision**: a one-row table where the row id is constrained to `1`.
- **Reuse, don't reinvent: clone `schedules.rs` line-for-line.** Same imports (`use concerto_error::{Error, Result}; use sqlx::{Row, SqliteConnection, SqlitePool};`), same `.map_err(|e| Error::Sqlx(Box::new(e)))`, same write-takes-`&mut SqliteConnection` / read-takes-`&SqlitePool` split, same private `row_to_*` projector. The accessors are deliberately thin — counting/budget *policy* is 412's.
- **`bump_daily_counters` is additive in SQL** (`SET x = x + ?`), not read-modify-write in Rust — so 412's per-turn bumps stay correct under the writer mutex without a select-then-update race. **`ensure_initialized` uses `INSERT OR IGNORE`** so it never resets a live row (idempotent bootstrap, called once per boot by 414).
- **The `chats(kind='maestro')` bootstrap is the singleton-by-CHECK pattern again, at the row level.** Insert only if no `kind='maestro'` row exists (or `INSERT OR IGNORE` on the caller's stable id). `session_id` MUST be `NULL` — the `0001` CHECK `((session_id IS NOT NULL) OR kind='maestro')` permits exactly this. The schema test at `initial_schema.rs:210` already inserts such a row in setup, proving it validates; this task makes the insert a reusable, idempotent accessor.
- **Two-site test update.** Adding a table means updating `crates/persist/tests/initial_schema.rs` in **two** spots (the `EXPECTED_TABLES` const + `insert_and_read_back_every_table`). `maestro_state`'s PK is an `INTEGER`, not the `TEXT` id every other table uses — its read-back is a `SELECT id FROM maestro_state WHERE id = 1` scalar, **not** an entry in the `String`-keyed `counts` vec (which would type-mismatch). Also assert `CHECK(id=2)` rejection here.
- **Cross-platform / no new deps.** Pure `sqlx`-SQLite + an additive forward-only migration; no `#[cfg(unix)]` (no agent supervisor involved — this is `crates/persist`, not `crates/core`), no new crate, so `cargo deny check` is unaffected.
- **Regen:** a new migration + new `pub` Rust persistence API ⇒ `./scripts/regen-interfaces.sh` updates `docs/interfaces/schema.md` (gains `maestro_state`) and `docs/interfaces/rust-api.md` (gains the `MaestroState` struct). **Commit both.** (Per the 305 handoff, `regen-interfaces.sh` captures struct/enum *type* definitions from `crates/*/src/api.rs` but **not** free `pub async fn`s — so the `maestro_state.rs` accessors will not appear in `rust-api.md`; only the `MaestroState` struct will. This matches established behavior — not drift.)
- **Parallel build hint:** the disjoint fan-out sub-parts (DAG `fanout = "migration+accessors ∥ singleton-bootstrap ∥ schema-test+regen"`): **(1)** `0015_maestro_state.sql` + the five budget accessors + `MaestroState` struct + `lib.rs` wiring; **(2)** the `ensure_maestro_chat` singleton-chat bootstrap (independent of the `maestro_state` accessors); **(3)** the `initial_schema.rs` `EXPECTED_TABLES`/insert/read-back updates + `regen-interfaces.sh`. Integrate into the single commit.

## Verification
**Tier 1.** The `rust` §5.3 command set.
1. `cargo check --workspace` → clean (the new module + struct + re-exports compile).
2. `cargo clippy --workspace --all-targets -- -D warnings` → clean.
3. `cargo fmt --all -- --check` → clean.
4. `cargo test -p concerto-persist maestro_state` → proves: `ensure_initialized`+`get` → defaults singleton; second `ensure_initialized` no-op; `bump_daily_counters(100,20)` twice → `(200,40)`; `reset_budget` zeroes + sets `budget_resets_at`; `set_last_digest`/`set_enabled(false)` round-trip; `CHECK(id=1)` rejects `id=2`; `ensure_maestro_chat` twice → exactly one `kind='maestro'`/`session_id NULL` row.
5. `cargo test -p concerto-persist --test initial_schema` → `every_expected_table_exists` now passes with `maestro_state` in `EXPECTED_TABLES`; `insert_and_read_back_every_table` inserts + reads back the singleton; `migrations_are_idempotent_across_reopens` still green (forward-only guard unaffected — `0015` is purely additive).
6. `cargo test --workspace --no-fail-fast` → all pass.
7. `cargo deny check` → green (no new crates; pure `sqlx` + a migration).
8. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → fails first run (regen pending), then commit the regen (`schema.md` gains `maestro_state`; `rust-api.md` gains the `MaestroState` struct) and it passes.
9. `scripts/smoke.sh` → **unchanged** (403 touches no smoke capability; the maestro digest/budget smoke check is turned on by 414/412, not here).

**Tier-1 scope + what it does NOT cover.** This is pure CI-self-verifiable persistence: the migration applies on a fresh DB, the accessors round-trip deterministically, the `id=1` singleton CHECK + the `kind='maestro'` singleton-chat bootstrap are proven, and the forward-only downgrade guard (`migrations_are_idempotent_across_reopens`) is unaffected by the additive `0015`. It does **NOT** cover the live token *counting* (Task 412 parses CLI/Direct-API usage into `bump_daily_counters`), inert-on-exhaust behavior, the UTC-midnight reset clock, or the digest cadence reading `last_digest_at` (Task 414) — those are wired later and demonstrated at the **Phase-4 Tier-3 manual checklist** line "confirm budget-exhaust goes inert while routing still works." This task adds **no** new Tier-3 line of its own.

## Definition of Done
- [x] Migration `0015_maestro_state.sql` creates `maestro_state` **verbatim** per `design/08 §4.1` (`id INTEGER PRIMARY KEY CHECK (id = 1)` singleton; `daily_in_today`/`daily_out_today` default 0; `budget_resets_at` NOT NULL; nullable `last_digest_at`; `enabled` default 1) — no CHECK-widen, no DROP, no index
- [x] Author-check done: highest migration on `main` confirmed `0014` (else shifted + noted in Handoff)
- [x] `crates/persist/src/maestro_state.rs`: `get`, `ensure_initialized`, `bump_daily_counters`, `reset_budget`, `set_last_digest`, `set_enabled`, `ensure_maestro_chat` as free async fns mirroring `schedules.rs` (writes `&mut SqliteConnection`, reads `&SqlitePool`)
- [x] `MaestroState` row struct in `api.rs`; `pub mod maestro_state;` + re-export in `lib.rs`
- [x] `chats(kind='maestro', session_id NULL)` singleton bootstrap idempotent — no schema change (validates against the `0001` CHECK)
- [x] `crates/persist/tests/initial_schema.rs`: `maestro_state` added to `EXPECTED_TABLES` + insert/read-back; `CHECK(id=2)` rejection asserted
- [x] Tests: singleton round-trip, additive cumulative bump, reset, last-digest/enabled round-trip, `CHECK(id=1)`, one-row maestro-chat bootstrap
- [x] All Verification commands pass on a clean checkout; smoke gate unchanged
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (signature-frozen seams return a typed `Err`/`Status`, not the macro — documented in Handoff)
- [x] No files outside Outputs modified
- [x] Interfaces regenerated + committed (`schema.md` + `rust-api.md`)
- [x] Single commit with the message below

## Outputs
- `crates/persist/migrations/0015_maestro_state.sql` (new — the `maestro_state` singleton table per `design/08 §4.1`)
- `crates/persist/src/maestro_state.rs` (new — the five budget accessors + `ensure_initialized` + `ensure_maestro_chat` singleton bootstrap + `row_to_maestro_state`)
- `crates/persist/src/api.rs` (modified — `MaestroState` row struct, mirroring the `Schedule` derive set)
- `crates/persist/src/lib.rs` (modified — `pub mod maestro_state;` + `MaestroState` in the `pub use api::{...}` re-export)
- `crates/persist/tests/initial_schema.rs` (modified — `maestro_state` in `EXPECTED_TABLES` + singleton insert/read-back + `CHECK(id=2)` rejection)
- `docs/interfaces/schema.md` (regenerated — gains `maestro_state`)
- `docs/interfaces/rust-api.md` (regenerated — gains the `MaestroState` struct)

## Commit message
```
phase-4: maestro_state table (0015) + maestro-chat singleton + budget accessor

Adds migration 0015_maestro_state.sql (the first daily-counter/budget
table; schedules deferred its token columns so there's no precedent) as a
CHECK(id=1) singleton per design/08 §4.1, with typed free-async-fn
accessors (get/ensure_initialized/bump_daily_counters/reset_budget/
set_last_digest/set_enabled) mirroring schedules.rs, plus an idempotent
ensure_maestro_chat bootstrap for the chats(kind='maestro') singleton (no
schema change — validates against the 0001 CHECK). Schema test covers the
new table. FROZEN per PHASE4_PLANNING §4.6 (D6); Task 412 consumes the
budget, 414 reads enabled/last_digest, 410 attaches daily summaries to the
bootstrapped maestro chat. Counting/inert-on-exhaust deferred to 412.

Refs: tasks/v1.0/403-maestro-state-table.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan:** None. Author-check confirmed the highest migration on base (`origin/main`) is `0014_pull_requests_merge_order.sql`, so **no shift** — the table landed at **`0015_maestro_state.sql`** exactly as reserved (PHASE4_PLANNING §3). The `0016`/410 row also stays at `0016`. The DDL was transcribed byte-for-byte from `design/08 §4.1` (matches the task's frozen Public-interface block verbatim). One non-drift implementation choice worth flagging: `ensure_maestro_chat` uses the `INSERT … SELECT … WHERE NOT EXISTS (SELECT 1 FROM chats WHERE kind='maestro')` form (not `INSERT OR IGNORE` keyed on the caller id) so the singleton invariant holds **regardless of the caller-supplied id** — a second call with a *different* id is still a no-op (proven by `ensure_maestro_chat_is_a_singleton`). The caller-supplied `id` is therefore honored only on the first bootstrap.
- **Open questions for next task:** None blocking. For consumers: Task **412** consumes `bump_daily_counters`/`reset_budget`/`get` + the FROZEN `MaestroState` struct for the 200K/50K cumulative-across-backends budget + inert-on-exhaust + UTC-midnight reset (note: `budget_resets_at`/`last_digest_at` are stored as plain unix-ms `i64`; the reset *clock* + cap *policy* are 412's, not encoded here). Task **414** consumes `set_enabled`/`set_last_digest` + reads `enabled`, and owns calling `ensure_initialized`/`ensure_maestro_chat` at Core boot (no `crates/core` wiring done here per Scope—out). Task **410** writes daily-summary `chat_messages` against the maestro chat id `ensure_maestro_chat` bootstraps, and lands migration `0016` for `chat_messages.metadata` — it **serializes after 403 on the migrations dir + `api.rs`/`lib.rs`** (the hard seam), so 410 must rebase onto this commit. `get` returning `None` ⇒ never initialized; consumers must `ensure_initialized` first.
- **Deliberate debt:** None. The accessors are thin storage seams — no `todo!()`/`unimplemented!()`/`TODO`/`FIXME` in the new SQL or Rust (verified). Budget *policy*, the reset clock, and token counting are 412's; digest cadence is 414's — intentionally out of scope, not stubbed.
- **Smoke-gate state:** **Unchanged.** 403 touched no `scripts/smoke.d/*` / `scripts/smoke.manifest` / `scripts/smoke.sh`; the maestro budget/digest smoke check is turned on later by 412/414. Regen of `docs/interfaces/{schema,rust-api}.md` is idempotent: `schema.md` gained the `maestro_state` table block; `rust-api.md` gained the `MaestroState` struct only (free `pub async fn` accessors are not captured by `regen-interfaces.sh` — established behavior per the 305 handoff, not drift).
