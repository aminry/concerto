# Task 410 — Daily history condensation (verbatim 24h + condensed older + `daily_summaries[:weekly]`) + `chat_messages.metadata` (migration `0016`)

| Field | Value |
|---|---|
| Phase | 4 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 403 |
| Touches subsystem(s) | 09 (Persistence), 08 (Maestro) |
| Smoke gate | unchanged |

## Goal
Keep the Maestro chat's token cost **flat regardless of session length** (`design/08 §3.7`) by condensing old history offline while the UI keeps the full unabridged log. Today there is **no condensation and no carrier for it**: `chat_messages` is locked by migration `0001` to exactly `(id, chat_id, role CHECK IN('user','assistant','system','tool'), content_json, created_at, parent_id, superseded_by)` with **no `metadata` column** (`crates/persist/src/chat_messages.rs:6` transcribes the frozen DDL; the only helpers are `insert(NewChatMessage)` at `:44` and `soft_delete_after` at `:78` — no day-range read, no summary insert, no summary list), the maestro `chats(kind='maestro', session_id NULL)` singleton is only just bootstrapped by `ensure_maestro_chat` (Task 403, `crates/persist/src/maestro_state.rs`), and **no `crates/core/src/maestro/` condensation code exists at all**. This task adds (a) migration **`0016_chat_messages_metadata.sql`** = the **additive** `ALTER TABLE chat_messages ADD COLUMN metadata TEXT` (NO CHECK-widen — additive-only; FROZEN per PHASE4_PLANNING §3/§4.6, D12), (b) the new `chat_messages.rs` accessors `list_in_day_range` / `insert_daily_summary` / `list_daily_summaries` (and the `metadata` field on `NewChatMessage`), and (c) a new `crates/core/src/maestro/condense.rs` module with the offline `condense_day` pass and the `assemble_input_window` window-builder, tagging each daily summary as a `chat_messages` row whose `metadata.role_extra = 'daily_summary'` (**NOT** folded into `content_json`; `design/08 §3.7/§4.1`). The agent's input window is **`daily_summaries[:weekly] + verbatim[last 24h] + user's latest`**; the UI still renders the raw history. After this task, **Task 414** (the gRPC + boot wiring + digest cadence) can drive the daily pass from a timer/boot and feed `assemble_input_window`'s output to the spawned Maestro agent; the condensation result is **agent-independent** persistence + a pure window-assembly fn, so it parallelizes the agent spine. What stays out: the real timer/scheduler that fires the pass, any live agent feeding, and the real-LLM summary *quality* of the one-paragraph daily summary (the LIVE path is `OneShotLlm`/`DeterministicOneShot`, the real Haiku-class call is Task 412's provider) — those are Tier-2/Tier-3 and not verified here.

## Inputs to read before starting
- `tasks/v1.0/PHASE4_PLANNING.md` §3 — **AUTHORITATIVE migration reservation.** `0016` = 410's `chat_messages += metadata TEXT`, an **additive `ALTER TABLE ADD COLUMN` (no CHECK-widen)**. **CHECK-widening is BANNED** here (`foreign_keys=ON` + per-migration transactions ⇒ a `DROP` cascade-deletes children); this migration touches no CHECK so it is safe. **Author check (do this first):** confirm the actual highest `crates/persist/migrations/NNNN_*.sql` on `main` is still **`0015`** (i.e. 403 landed first — 410 serializes after 403 on the migrations dir per §8.1). If a migration landed above `0015`, **shift `0016`→`0016+offset`** preserving order and **note it in your Handoff**; if `0015` is *absent* (403 not yet merged), STOP — 410 depends on 403.
- `tasks/v1.0/PHASE4_PLANNING.md` §1 (D12) — **AUTHORITATIVE.** "`chat_messages` has no `metadata` column today. 410 adds `metadata TEXT` via migration 0016 (additive `ALTER TABLE ADD COLUMN` — no CHECK-widen) and tags daily summaries `metadata.role_extra='daily_summary'` (`design/08 §3.7/§4.1`); it is not folded into `content_json`." This row is the entire migration + tagging contract.
- `tasks/v1.0/PHASE4_PLANNING.md` §4.6 — the `chats(kind='maestro')` singleton + `maestro_state` are FROZEN by **403**; 410 **CONSUMES the maestro chat id** that `ensure_maestro_chat` bootstraps — it does **not** re-lock it. Do not author `maestro_state`, `0015`, or the chat bootstrap here.
- `tasks/v1.0/PHASE4_PLANNING.md` §8.1 (410 write-set) — your write-set (`migrations/0016_*.sql`, `persist/src/chat_messages.rs`, `crates/core/src/maestro/condense.rs`) + the **hard seam you share: 403 (the migrations dir)** — the orchestrator serializes 403 before 410. The maestro `mod.rs` is the **soft seam** (add one `pub mod condense;` line in a distinct region; additive, auto-merges on rebase).
- `design/08_Maestro_Agent.md` §3.7 — the canonical condensation rule: recent (last 24h) verbatim; a daily offline pass condenses **24-48h-old** messages into a **one-paragraph** daily summary; the agent's input is `daily_summaries[:weekly] + verbatim[last 24h] + user's latest`; the user sees the **full unabridged** history in the UI; "keeps per-day cost roughly flat regardless of session length." §4.1 — "Daily summaries are stored as `chat_messages` with a `metadata.role_extra = 'daily_summary'` tag." §10 — the Tier-1 bench row: "Cost | Daily condensation keeps token cost roughly flat over 30 days | Bench" (synthetic clock).
- `crates/persist/src/chat_messages.rs` — the **accessor pattern to mirror EXACTLY** (free `pub async fn` over `&mut SqliteConnection` writes / `&SqlitePool` reads, `.map_err(|e| Error::Sqlx(Box::new(e)))`); the locked `0001` DDL transcribed in its header (`:6`); the existing `NewChatMessage` struct (`:32` — **you add a `metadata: Option<String>` field**) and `insert` (`:44` — its `INSERT` column list **gains `metadata`**) and `soft_delete_after` (`:78`). The doc comment at `:21` ("V1.0 may add a richer CRUD when the maestro chat surface lands") is *this* task's invitation.
- `crates/persist/migrations/0001_initial_schema.sql` (the `chat_messages` block, ~line 145+) — confirm the frozen column set + that `metadata` is genuinely absent; the `ALTER TABLE … ADD COLUMN metadata TEXT` is purely additive (NULL default; existing rows read `NULL`).
- `crates/persist/migrations/0004_schedules.sql` — the migration-file house style (header comment explaining the column + why; `INTEGER` unix-ms convention). Mirror the comment density.
- `tasks/v1.0/403-maestro-state-table.md` — the **sibling you depend on**: it ships `0015`, the `maestro_state` accessors, and `ensure_maestro_chat(conn, id, created_at)` which bootstraps the `kind='maestro'` chat id you write summaries against. Read its Public-interface block so you consume the chat id as frozen (PHASE4_PLANNING §4.6) and do not redefine it.
- `crates/core/src/llm/oneshot.rs` — `OneShotLlm::suggest(OneShotRequest{action, repo_id, prompt, context}) -> Result<String>`; `ActionKind::DigestSummary` (already reserved); `DeterministicOneShot` is the LIVE fallback (truncate/collapse). **CONSUME this as frozen by Task 312** for the one-paragraph daily summarizer — do not invent a new summarizer trait; the real Haiku/Sonnet call is Task 412's separate provider seam (PHASE4_PLANNING §4.3/§4.5).
- `crates/persist/tests/initial_schema.rs` — `EXPECTED_TABLES` + `insert_and_read_back_every_table`: `metadata` is a **new column on an existing table**, so confirm the schema test's column-shape assertions (if any) tolerate the added nullable column; no new table is added (no `EXPECTED_TABLES` edit), but a round-trip of a `metadata`-bearing row belongs in `chat_messages` tests.

## Scope — in
- **`crates/persist/migrations/0016_chat_messages_metadata.sql` (new):**
  - A header comment in the `0004` house style: this is an **additive** `ALTER TABLE chat_messages ADD COLUMN metadata TEXT` (nullable JSON text); existing rows read `NULL`; **no CHECK-widen, no `DROP`, no index** (it is a free-text tag column, not a query key). Note that it carries `{"role_extra":"daily_summary"}` for the condensation pass and is otherwise NULL for normal chat rows.
  - Exactly one statement: `ALTER TABLE chat_messages ADD COLUMN metadata TEXT;`.
- **`crates/persist/src/chat_messages.rs` (modified) — extend the existing accessors + add three:**
  - Add `pub metadata: Option<String>` to `NewChatMessage` (the nullable JSON tag; `None` for ordinary rows). Update `insert`'s `INSERT` column list + `VALUES` + `.bind(&row.metadata)` so the new column round-trips. `soft_delete_after` is untouched.
  - `list_in_day_range(pool: &SqlitePool, chat_id: &str, start_ms: i64, end_ms: i64) -> Result<Vec<ChatMessage>>` — the verbatim/condense window selector: `SELECT … WHERE chat_id = ? AND created_at >= ? AND created_at < ? AND superseded_by IS NULL ORDER BY created_at ASC`. (Read path; used both for the 24-48h condense slice and the last-24h verbatim slice.) Excludes superseded rows so the agent never re-reads rewound history.
  - `insert_daily_summary(conn: &mut SqliteConnection, chat_id: &str, id: &str, content_json: &str, created_at: i64) -> Result<String>` — a thin wrapper over `insert` that hard-codes `role = 'assistant'`, `parent_id = None`, `superseded_by = None`, and `metadata = Some(r#"{"role_extra":"daily_summary"}"#)` (the FROZEN tag). The summary text lives in `content_json`; the **tag is the `metadata` column, never `content_json`** (D12).
  - `list_daily_summaries(pool: &SqlitePool, chat_id: &str) -> Result<Vec<ChatMessage>>` — `SELECT … WHERE chat_id = ? AND metadata IS NOT NULL AND json_extract(metadata, '$.role_extra') = 'daily_summary' AND superseded_by IS NULL ORDER BY created_at ASC`. (Read path; the `assemble_input_window` `daily_summaries[:weekly]` source. SQLite ships JSON1; `json_extract` is available — if the build disables it, fall back to a `metadata LIKE '%"role_extra":"daily_summary"%'` filter and note in Handoff.)
  - Add a `pub struct ChatMessage { pub id, pub chat_id, pub role, pub content_json, pub created_at, pub parent_id: Option<String>, pub superseded_by: Option<String>, pub metadata: Option<String> }` read-back row struct (mirroring `NewChatMessage` + the read-only `id`) + a private `row_to_chat_message(row) -> ChatMessage` projector (mirroring `schedules.rs`'s `row_to_*` style). If the file has no read struct today, this is its first.
- **`crates/core/src/maestro/condense.rs` (new):**
  - `pub async fn condense_day(...)` — the **offline pass over a single day**: given the maestro `chat_id`, a `now_ms` (synthetic-clock-injectable), and an `&dyn OneShotLlm` (LIVE = `DeterministicOneShot`), select the **24-48h-old** slice via `list_in_day_range(now-48h, now-24h)`, skip if empty or if a `daily_summary` already exists for that day window (idempotent — re-running the pass does not double-summarize), build a digest prompt, call `OneShotLlm::suggest(OneShotRequest{ action: ActionKind::DigestSummary, .. })`, and persist the one-paragraph result via `insert_daily_summary` (its `created_at` set to the **day boundary** so it sorts before the verbatim window). Returns the inserted summary id or `None` (nothing to condense).
  - `pub async fn assemble_input_window(...) -> InputWindow` — the **pure window builder** for what the agent sees: `daily_summaries[:weekly]` (the last 7 `list_daily_summaries` rows) **+** `verbatim[last 24h]` (`list_in_day_range(now-24h, now)`) **+** the user's latest message (caller-passed). Returns a typed `InputWindow { summaries: Vec<ChatMessage>, verbatim: Vec<ChatMessage>, latest: String }` (or a rendered `String` if the caller prefers — pick one shape and FREEZE it for 414). This fn is the contract 414 feeds to the spawned Maestro agent's stdin; the **UI window is unaffected** (it reads the full history directly via the existing chat read path — out of scope here).
  - `[:weekly]` = the **7 most-recent** daily summaries (drop older — they were already absorbed by a coarser future pass; V1.0 keeps a flat 7-day window per `design/08 §3.7`). Document the constant.
  - A `pub mod condense;` line added to `crates/core/src/maestro/mod.rs` (the **soft seam** — a distinct additive region; 401 owns the initial `mod.rs`).
- **Tests (Tier 1):** in `crates/persist/tests/` (or `#[cfg(test)]` in `chat_messages.rs`) and a `#[cfg(test)]` in `condense.rs` with a **synthetic clock**:
  - migration `0016` applies on a fresh DB; a `metadata`-bearing row round-trips (`insert` with `metadata=Some(...)` → read back the JSON tag); a `metadata=None` row reads back `NULL`.
  - `list_in_day_range` selects only rows in `[start, end)`, ordered, and **excludes superseded** rows.
  - `insert_daily_summary` writes `role='assistant'` + `metadata.role_extra='daily_summary'` with the text in `content_json` (assert `content_json` carries no `role_extra`); `list_daily_summaries` returns exactly the tagged rows in `created_at` order.
  - `condense_day` over a fixture is **idempotent** (second run inserts no second summary for the same day).
  - **The flat-cost bench:** build a **30-day fixture** of synthetic chat (N messages/day) on a synthetic clock; run `condense_day` for each elapsed day; assert `assemble_input_window`'s **input size (summaries + verbatim-24h message count, or rendered char/approx-token length) stays within a bounded constant across all 30 days** (it does NOT grow with total history) AND the **last-24h verbatim slice is preserved** unsummarized. This is `design/08 §10`'s "keeps token cost roughly flat over 30 days" row.

## Scope — out
- **The timer / scheduler that *fires* `condense_day` daily + the boot wiring** — **Task 414** (it constructs the `MaestroHandle` in `boot.rs` and owns the digest/condensation cadence). This task ships the pass as a callable `async fn`; nothing in `crates/core/src/boot.rs` is touched, leaving a **call-site seam** 414 drives from a timer/midnight tick.
- **Feeding `assemble_input_window`'s output to the live Maestro agent stdin** — **Task 414** (`SendToMaestro` + the spawned agent from Task 402). This task freezes the window shape; it does not spawn or feed an agent, leaving the `InputWindow` return type as the consumed seam.
- **The real Haiku/Sonnet one-paragraph summarizer quality** — **Task 412**'s provider seam (PHASE4_PLANNING §4.3); the LIVE path here is `OneShotLlm`/`DeterministicOneShot` (deterministic truncate/collapse), reused (not modified) per §4.5. The double does NOT cover real-LLM summary *quality*.
- **Coarser-than-weekly rollups (monthly/quarterly condensation)** — out of V1.0 scope; `design/08 §3.7` specifies a flat 7-day window. This task drops summaries older than 7 days from the input window (they remain in the DB for the UI); a future task may add a second-level rollup. Leaves the `[:weekly]` constant as the only condense tier.
- **`maestro_state` / the `chats(kind='maestro')` bootstrap / migration `0015`** — **Task 403** (FROZEN, PHASE4_PLANNING §4.6). This task **consumes** the maestro chat id; it does not create it. Do not author `0015` or the chat bootstrap.
- **Real-world (Tier-3):** a real multi-week Maestro chat whose per-turn token cost is observed to stay flat under a live LLM backend across real calendar days is demonstrated at the **Phase-4 manual checklist** once 412/414 wire the live provider + cadence — not provable by this synthetic-clock persistence/pure-fn task.

## Public interface this task locks
- **SQL (FROZEN, `design/08 §3.7/§4.1` / PHASE4_PLANNING §3 + D12) — `crates/persist/migrations/0016_chat_messages_metadata.sql`:**

```sql
-- Additive ALTER (no CHECK-widen, no DROP, no index). Existing rows read NULL.
-- Carries {"role_extra":"daily_summary"} for the daily-condensation pass (Task 410);
-- NULL for ordinary chat rows. The tag lives here, NOT in content_json (D12).
ALTER TABLE chat_messages ADD COLUMN metadata TEXT;
```

- **Rust accessors (FROZEN, PHASE4_PLANNING §4.6 consume + D12) — `crates/persist/src/chat_messages.rs` (free async fns, mirroring `schedules.rs`):**

```rust
/// Insert-time shape — UNCHANGED except the new nullable metadata tag.
pub struct NewChatMessage {
    pub id: String,
    pub chat_id: String,
    pub role: String,                  // user|assistant|system|tool (0001 CHECK)
    pub content_json: String,
    pub created_at: i64,               // unix ms
    pub parent_id: Option<String>,
    pub superseded_by: Option<String>,
    pub metadata: Option<String>,      // NEW: nullable JSON tag, e.g. {"role_extra":"daily_summary"}
}

/// Read-back row (NEW struct; the file had only insert/soft_delete before).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub id: String,
    pub chat_id: String,
    pub role: String,
    pub content_json: String,
    pub created_at: i64,
    pub parent_id: Option<String>,
    pub superseded_by: Option<String>,
    pub metadata: Option<String>,
}

/// Verbatim/condense window selector: rows in [start_ms, end_ms), non-superseded,
/// ascending. Used for BOTH the 24-48h condense slice and the last-24h verbatim slice.
pub async fn list_in_day_range(
    pool: &SqlitePool, chat_id: &str, start_ms: i64, end_ms: i64,
) -> Result<Vec<ChatMessage>>;

/// Persist a one-paragraph daily summary as a chat_messages row tagged
/// metadata.role_extra='daily_summary' (role='assistant', no parent/supersede).
/// The summary text is content_json; the tag is the metadata column (D12).
pub async fn insert_daily_summary(
    conn: &mut SqliteConnection, chat_id: &str, id: &str, content_json: &str, created_at: i64,
) -> Result<String>;

/// List the daily summaries for a chat (metadata.role_extra='daily_summary'),
/// non-superseded, ascending. The assemble_input_window daily_summaries source.
pub async fn list_daily_summaries(
    pool: &SqlitePool, chat_id: &str,
) -> Result<Vec<ChatMessage>>;
```

- **Maestro condensation surface (FROZEN, `design/08 §3.7`) — `crates/core/src/maestro/condense.rs`:**

```rust
/// What the Maestro agent sees as input: weekly daily-summaries + verbatim-24h + latest.
/// The UI window is separate (it reads the full unabridged history).
#[derive(Debug, Clone)]
pub struct InputWindow {
    pub summaries: Vec<ChatMessage>,   // daily_summaries[:weekly] — the 7 most recent
    pub verbatim: Vec<ChatMessage>,    // verbatim[last 24h]
    pub latest: String,                // the user's newest message
}

/// Number of daily summaries retained in the agent input window (flat 7-day window).
pub const WEEKLY_SUMMARY_WINDOW: usize = 7;

/// Offline pass: condense the 24-48h-old slice of `chat_id` into one daily-summary
/// row via OneShotLlm (DeterministicOneShot LIVE). Idempotent per day; returns the
/// new summary id, or None if nothing to condense. `now_ms` is clock-injectable.
pub async fn condense_day(
    chat_id: &str, now_ms: i64, llm: &dyn OneShotLlm, /* + persist handle */
) -> Result<Option<String>>;

/// Pure window builder: daily_summaries[:weekly] + verbatim[last 24h] + latest.
pub async fn assemble_input_window(
    chat_id: &str, now_ms: i64, latest: String, /* + persist handle */
) -> Result<InputWindow>;
```

- **CONSUMES (do NOT re-lock):** the maestro `chats(kind='maestro', session_id NULL)` singleton + its chat id are **frozen by Task 403** (PHASE4_PLANNING §4.6); `OneShotLlm`/`OneShotRequest`/`ActionKind::DigestSummary`/`DeterministicOneShot` are **frozen by Task 312** (PHASE4_PLANNING §4.5) — reused, never modified; the `chat_messages` `(id, chat_id, role CHECK, content_json, created_at, parent_id, superseded_by)` columns are frozen by migration `0001` (Task 09) — this task only **adds** `metadata`, never alters the existing columns or their CHECK.

## Implementation notes
- **The tag is the `metadata` column, NEVER `content_json` (the load-bearing rule, D12).** A daily summary is a normal `chat_messages` row whose *text* is in `content_json` and whose *classification* is `metadata.role_extra='daily_summary'`. This keeps the summary renderable as ordinary chat in the UI while letting `list_daily_summaries` find it by tag. A reviewer should be able to `grep` `condense.rs` and find the summary text never carries `role_extra`.
- **Additive migration only — no CHECK-widen.** `ALTER TABLE … ADD COLUMN metadata TEXT` is the entire DDL; SQLite back-fills existing rows with `NULL`. Do **not** touch the `role` CHECK, do **not** `DROP`+recreate (that cascade-deletes children under `foreign_keys=ON`). This is why D12 reserves a plain additive migration, not the `0010` writable-schema rewrite.
- **Reuse, don't reinvent: clone the `chat_messages.rs`/`schedules.rs` accessor shape.** Same imports (`use concerto_error::{Error, Result}; use sqlx::{Row, SqliteConnection, SqlitePool};`), same `.map_err(|e| Error::Sqlx(Box::new(e)))`, same write-`&mut SqliteConnection`/read-`&SqlitePool` split, a private `row_to_chat_message` projector. `insert_daily_summary` wraps the existing `insert` — do not duplicate the `INSERT` SQL.
- **Reuse the LLM seam, don't add one.** The one-paragraph summarizer routes through `OneShotLlm::suggest` with `ActionKind::DigestSummary`; `DeterministicOneShot` is the LIVE V1.0 path (deterministic truncate/collapse so the Tier-1 bench is reproducible). The real Haiku/Sonnet call is **Task 412**'s provider — do not wire a real network call here; that would make the bench non-deterministic and break CI.
- **Idempotency is load-bearing for the bench.** `condense_day` must no-op if the 24-48h window is empty OR a `daily_summary` already covers that day — otherwise re-running the pass (414's timer may fire more than once, or a boot replays it) double-counts and the flat-cost invariant fails. Key the skip off `list_daily_summaries` containing a summary whose `created_at` falls on the target day boundary.
- **Synthetic clock, not wall clock.** `condense_day`/`assemble_input_window` take `now_ms` explicitly (do not call `SystemTime::now()` inside) so the 30-day bench can advance the clock deterministically (`design/08 §10` "Synthetic clock"). 414 passes the real clock at the call site.
- **Cross-platform / no new deps.** Pure `sqlx`-SQLite (the `metadata` column + JSON1 `json_extract`, already linked by `chats.settings_json` usage elsewhere) + a pure window-assembly fn; **no `#[cfg(unix)]`** (no agent supervisor in this task — the agent feeding is 414's), no new crate, so `cargo deny check` is unaffected.
- **Soft seam on `maestro/mod.rs`.** Add `pub mod condense;` in a distinct additive region (401 owns the initial skeleton); this auto-merges on rebase. The hard seam is the **migrations dir shared with 403** — 410 serializes after 403 (confirm `0015` exists; this task is `0016`).
- **Regen:** a new migration + new `pub` persistence API ⇒ `./scripts/regen-interfaces.sh` updates `docs/interfaces/schema.md` (the `chat_messages` row gains the `metadata` column). Per the 305/403 handoff precedent, `regen-interfaces.sh` captures struct/enum *type* definitions from `crates/*/src/api.rs` but **not** free `pub async fn`s, and `ChatMessage`/`NewChatMessage` live in `chat_messages.rs` (not `api.rs`) — so only `schema.md` changes; `rust-api.md` is unaffected. **Commit `schema.md`.** No proto change (no gRPC surface in this task).
- **Parallel build hint:** the disjoint fan-out sub-parts (DAG `fanout = "migration+metadata-accessors ∥ condensation-pass ∥ verbatim/condensed-window-assembly"`): **(1)** `0016_chat_messages_metadata.sql` + the `NewChatMessage.metadata` field + `insert` update + `list_in_day_range`/`insert_daily_summary`/`list_daily_summaries` + `ChatMessage` struct + `regen-interfaces.sh` (all `crates/persist`); **(2)** `condense_day` (the offline pass + idempotency + `OneShotLlm` call) in `condense.rs`; **(3)** `assemble_input_window` + `InputWindow` + `WEEKLY_SUMMARY_WINDOW` + the 30-day flat-cost bench. (2)+(3) depend on (1)'s accessors only at the read/write seam. Integrate into the single commit.

## Verification
**Tier 1.** The `rust` §5.3 command set.
1. `cargo check --workspace` → clean (the new `chat_messages` accessors + `ChatMessage`/`InputWindow` structs + the `maestro::condense` module compile).
2. `cargo clippy --workspace --all-targets -- -D warnings` → clean.
3. `cargo fmt --all -- --check` → clean.
4. `cargo test -p concerto-persist chat_messages` → proves: `0016` applies; `metadata`-bearing row round-trips (and `None` reads back NULL); `list_in_day_range` selects only `[start,end)` non-superseded rows in order; `insert_daily_summary` writes `role='assistant'` + `metadata.role_extra='daily_summary'` with text in `content_json` (no `role_extra` in `content_json`); `list_daily_summaries` returns exactly the tagged rows ascending.
5. `cargo test -p concerto-core condense` → proves: `condense_day` is idempotent (second run inserts no second summary for a day); the **30-day synthetic-clock bench** — `assemble_input_window`'s input size stays within a bounded constant across all 30 days (does NOT grow with total history) AND the last-24h verbatim slice is preserved unsummarized.
6. `cargo test -p concerto-persist --test initial_schema` → `migrations_are_idempotent_across_reopens` still green (forward-only guard unaffected — `0016` is purely additive); the `chat_messages` read-back tolerates the new nullable column.
7. `cargo test --workspace --no-fail-fast` → all pass.
8. `cargo deny check` → green (no new crates; pure `sqlx` + a migration).
9. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → fails first run (regen pending), then commit the regen (`schema.md`'s `chat_messages` gains `metadata`) and it passes.
10. `scripts/smoke.sh` → **unchanged** (410 touches no smoke capability; the live daily-condensation/digest cadence is turned on by 414, not here). Exits 0.

**Tier-1 scope + what it does NOT cover.** This is CI-self-verifiable persistence + a pure-fn window builder: the additive `0016` migration applies, the three new accessors round-trip deterministically, the `daily_summary` tag lives in `metadata` (never `content_json`), `condense_day` is idempotent, and the **synthetic-clock 30-day bench** proves the input window stays flat while the last-24h verbatim slice is preserved. It does **NOT** cover the real timer firing the pass (Task 414), feeding the window to a live agent (Task 414), or the real-LLM one-paragraph summary *quality* (Task 412's provider; the LIVE path here is the deterministic `OneShotLlm`). Those are demonstrated at the **Phase-4 Tier-3 manual checklist** line "confirm a multi-week Maestro chat keeps per-turn token cost flat under a live backend." This task adds **no** new Tier-3 line of its own beyond that deferral. **(Why Tier-1 here while 404/409 — which call the same `DeterministicOneShot` — are Tier-2:** in 410 the deterministic summarizer is the shipping V1.0 path and the **proven artifact is the window's flat token cost**, not the summary *text*, so no real-LLM double is needed; in 404/409 the summary/digest *content* IS the proven output, so its quality is judged against the real provider at the gate. This is a deliberate, correct tier split — not a downgrade.)

## Definition of Done
- [x] Author-check done: highest migration on `main` confirmed `0015` (403 landed first); else shifted `0016`→`0016+offset` + noted in Handoff
- [x] Migration `0016_chat_messages_metadata.sql` = additive `ALTER TABLE chat_messages ADD COLUMN metadata TEXT` — no CHECK-widen, no DROP, no index; existing rows read NULL
- [x] `NewChatMessage` gains `metadata: Option<String>`; `insert` round-trips the new column; `soft_delete_after` untouched
- [x] New `chat_messages.rs` accessors `list_in_day_range` / `insert_daily_summary` / `list_daily_summaries` + the `ChatMessage` read struct + `row_to_chat_message` projector, mirroring `schedules.rs`
- [x] Daily summary tagged `metadata.role_extra='daily_summary'` (the tag in `metadata`, the text in `content_json` — NEVER folded into `content_json`)
- [x] `crates/core/src/maestro/condense.rs`: `condense_day` (offline, idempotent, `OneShotLlm`/`DeterministicOneShot` LIVE, clock-injectable) + `assemble_input_window` (`daily_summaries[:weekly]` + `verbatim[last 24h]` + latest) + `InputWindow` + `WEEKLY_SUMMARY_WINDOW`; `pub mod condense;` added to maestro `mod.rs` (soft seam, distinct region)
- [x] Consumes the maestro chat id (Task 403, FROZEN PHASE4_PLANNING §4.6) — does NOT re-author `0015`/`maestro_state`/the chat bootstrap
- [x] Tests: metadata round-trip, day-range selection (excludes superseded), summary tag in `metadata` not `content_json`, `list_daily_summaries`, `condense_day` idempotency, the 30-day synthetic-clock flat-cost bench (input window bounded; verbatim-24h preserved)
- [x] All Verification commands pass on a clean checkout; smoke gate unchanged (green)
- [x] No TODO/FIXME/unimplemented!()/todo!() in new code (signature-frozen seams return a typed `Err`/`Ok(None)`, not the macro — documented in Handoff)
- [x] No files outside Outputs modified
- [x] Interfaces regenerated + committed (`schema.md`'s `chat_messages` gains `metadata`)
- [x] Single commit with the message below

## Outputs
- `crates/persist/migrations/0016_chat_messages_metadata.sql` (new — additive `ALTER TABLE chat_messages ADD COLUMN metadata TEXT`)
- `crates/persist/src/chat_messages.rs` (modified — `NewChatMessage.metadata` field + `insert` update; new `list_in_day_range` / `insert_daily_summary` / `list_daily_summaries` + `ChatMessage` struct + `row_to_chat_message`)
- `crates/core/src/maestro/condense.rs` (new — `condense_day` offline pass + `assemble_input_window` + `InputWindow` + `WEEKLY_SUMMARY_WINDOW`)
- `crates/core/src/maestro/mod.rs` (modified — `pub mod condense;` in a distinct additive region; soft seam)
- `docs/interfaces/schema.md` (regenerated — `chat_messages` gains the `metadata` column)

## Commit message
```
phase-4: daily history condensation + chat_messages.metadata (0016)

Adds migration 0016 (additive ALTER TABLE chat_messages ADD COLUMN
metadata TEXT — no CHECK-widen) + chat_messages accessors
(list_in_day_range / insert_daily_summary / list_daily_summaries) and a
new maestro/condense.rs: an offline condense_day pass that summarizes
24-48h-old messages into one daily-summary row tagged
metadata.role_extra='daily_summary' (text in content_json, tag in
metadata — never folded), plus assemble_input_window =
daily_summaries[:weekly] + verbatim[last 24h] + latest. Summarizer reuses
OneShotLlm/DeterministicOneShot (real LLM is 412's). A 30-day synthetic-
clock bench proves the agent input window stays flat while the UI keeps
the full unabridged history. Timer/boot wiring + live agent feed deferred
to 414. FROZEN per PHASE4_PLANNING §3/D12; consumes the maestro chat id
(403, §4.6).

Refs: tasks/v1.0/410-daily-condensation.md
```

## Handoff Notes (filled in when finishing)
- **Drift from plan:**
  - **`json_extract` available — no `LIKE` fallback.** `list_daily_summaries` uses `json_extract(metadata, '$.role_extra') = 'daily_summary'` (SQLite JSON1 is linked); the round-trip + tag-filter tests pass against it, so the documented `LIKE '%"role_extra":"daily_summary"%'` fallback was NOT needed.
  - **`InputWindow` is a typed struct, NOT a rendered `String`** — `InputWindow { summaries: Vec<ChatMessage>, verbatim: Vec<ChatMessage>, latest: String }`. 414 renders it for the agent stdin; freezing the struct (not a pre-rendered string) keeps the rendering policy 414's. A private `render_slice` builds the summarizer *input* context for `condense_day` only.
  - **Day-boundary math:** the condensed slice is the half-open `[now-48h, now-24h)` (`DAY_MS = 86_400_000`). The summary's `created_at` is pinned to the **start of that slice** (`now - 2*DAY_MS`); that boundary is also the idempotency key (`condense_day` no-ops when a `daily_summary` already sits at that exact `created_at`). The summary id is derived deterministically (`daily-summary-{chat_id}-{slice_start}`) so an uncommitted re-run stays stable. The summary therefore sorts *before* the last-24h verbatim window.
  - **Migration confirmed `0016`** — highest on base was `0015` (403 merged); authored `0016_chat_messages_metadata.sql`, no offset shift needed. Purely additive `ALTER TABLE chat_messages ADD COLUMN metadata TEXT` (no CHECK-widen, no DROP, no index); `migrations_are_idempotent_across_reopens` stays green.
  - **Forced mechanical edits outside the stated Outputs:** adding the non-defaulted `metadata` field to `NewChatMessage` forces a one-line `metadata: None` at its two existing constructors — `crates/core/src/agent_supervisor/checkpoint.rs` (V0.1 turn-marker insert) and `crates/core/tests/checkpoint_revert.rs`. These are compile-mandatory consequences of the frozen struct change, not scope expansion.
- **Open questions for next task:** Task **414** drives `condense_day(persist, chat_id, now_ms, llm)` from a timer/boot tick and feeds `assemble_input_window(...)`'s `InputWindow` to the spawned Maestro agent's stdin — built on the FROZEN `condense.rs` surface + the maestro chat id from 403's `ensure_maestro_chat`. Both fns take `&Persistence` + a synthetic-injectable `now_ms` (414 passes the real clock at the call site). Task **412**'s real Haiku/Sonnet provider replaces `DeterministicOneShot` behind the unchanged `OneShotLlm`/`ActionKind::DigestSummary` seam. Task **412** also owns the per-turn token accounting that the flat window feeds.
- **Deliberate debt:** the LIVE summarizer is `DeterministicOneShot` (collapse/echo) so the Tier-1 30-day bench is reproducible — real-LLM summary *quality* is 412's, not stubbed with a macro. `condense_day` returns `Ok(None)` (empty slice or already-summarized day), never `todo!()`/`unimplemented!()`. The flat 7-day `WEEKLY_SUMMARY_WINDOW` has no coarser rollup tier — older summaries persist for the UI but leave the agent window; a future task may add monthly/quarterly rollups.
- **Smoke-gate state:** **unchanged** — `scripts/smoke.sh` PASSED (exit 0, 126s); 410 touches no `scripts/smoke.d/*` or `scripts/smoke.manifest`; the live daily-condensation cadence is turned on by 414.
