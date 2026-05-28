# Task 08 — SQLite Migration Runner

| Field | Value |
|---|---|
| Phase | 1 |
| Size | small (≤4h) |
| Depends on | 01, 02, 04, 05 |
| Touches subsystem(s) | 09 (Persistence) |
| Smoke gate | unchanged |

## Goal
Stand up the SQLite migration runner inside `crates/persist`. After this task, calling `Persistence::open(path)` opens a SQLite DB in WAL mode, runs any pending migrations from `crates/persist/migrations/`, and returns a typed handle. No migrations exist yet — Task 09 adds the first one.

## Inputs to read before starting
- `design/09_Persistence.md` §3.2 (migration tool: `sqlx::migrate!` with embedded SQL files), §3.3 (WAL config), §6.2 (migration runner: runs on PersistenceActor start, forward-only, single transaction per file), §8 (failure modes).
- `design/00_Architecture_Overview.md` §6.2 (SQLite via sqlx; WAL mandatory).
- `tasks/07-first-proto-messages.md` → "Handoff Notes".

## Scope — in
- Add `sqlx` (with features `runtime-tokio`, `sqlite`, `macros`, `migrate`) to `crates/persist/Cargo.toml`.
- Implement `crates/persist/src/lib.rs` with:
  ```rust
  pub struct Persistence {
      writer: Arc<Mutex<SqliteConnection>>,
      readers: SqlitePool,                     // multi-reader pool, query_only
  }
  
  pub struct PersistenceConfig {
      pub db_path: PathBuf,
      pub max_readers: u32,
  }
  
  impl Persistence {
      pub async fn open(config: PersistenceConfig) -> Result<Self>;
      pub async fn writer(&self) -> impl AsyncDeref<SqliteConnection>;  // returns guard
      pub fn readers(&self) -> &SqlitePool;
      pub async fn shutdown(self) -> Result<()>;
  }
  ```
- The `open` function:
  1. Creates the DB file's parent directory if missing.
  2. Opens the DB with `SqliteConnectOptions` setting `journal_mode = Wal`, `synchronous = Normal`, `busy_timeout = 5000ms`, `foreign_keys = on`.
  3. Runs `sqlx::migrate!("./migrations").run(&pool)` to apply pending migrations.
  4. Runs `PRAGMA quick_check;` after migrations; aborts if the result is anything other than `"ok"`.
  5. Sets up a separate read-only pool with `PRAGMA query_only = ON` on each connection.
  6. Returns the handle.
- Create `crates/persist/migrations/` directory with a placeholder `.gitkeep` (no migrations yet).
- Add `crates/persist/src/api.rs` re-exporting `Persistence`, `PersistenceConfig`.
- Add unit tests for: open-then-shutdown round-trip on a tempdir, opening twice fails or queues (verify WAL semantics), `quick_check` failure path via inserting a malformed file.

## Scope — out
- No actual schema migrations (Task 09).
- No write-queue / serializer (Task 09 establishes the schema; the writer pattern lives there or in a follow-up task).
- No backup / export (V1.0).
- No audit log integration (Task 44).

## Public interface this task locks
- Rust: `crates/persist/src/api.rs` — `pub struct Persistence`, `pub struct PersistenceConfig`, `pub async fn Persistence::open(...) -> Result<Self>`, `pub async fn Persistence::shutdown(self) -> Result<()>`.
- Migration directory: `crates/persist/migrations/NNNN_<name>.sql`. Naming forever forward-only, numerically ordered.
- WAL + busy_timeout=5000 + foreign_keys=on are non-negotiable.

## Implementation notes
- `sqlx::migrate!` is a build-time macro that embeds migrations into the binary. The directory path is relative to `crates/persist/`.
- Use `tokio::sync::Mutex` (not `std::sync::Mutex`) for the writer guard so it can be held across await points.
- For the writer, the long-term goal is a single dedicated task draining an mpsc queue (per `design/09 §6.1`). For Task 08, a `Mutex<SqliteConnection>` is acceptable — the queue pattern arrives when concurrent writes become a real concern (Task 19+).
- Wire the persistence open into `crates/core/src/main.rs` startup AFTER `logging::init()`. If `open` fails, log the error and exit with non-zero status — do not panic.
- Use `dirs::data_dir()` plus `concerto/concerto.db` as the default `db_path`, with override via `CONCERTO_DB_PATH` env var (for tests).

## Verification
1. `cargo build -p concerto-persist` → succeeds.
2. `cargo test -p concerto-persist` → all unit tests pass.
3. `cargo clippy -p concerto-persist -- -D warnings` → clean.
4. `cargo run --bin concerto-core` boots; `~/concerto/concerto.db` (or `$CONCERTO_DB_PATH`) is created; SIGTERM cleanly closes.
5. Inspect the created DB: `sqlite3 ~/concerto/concerto.db 'PRAGMA journal_mode;'` returns `wal`.
6. `sqlite3 ~/concerto/concerto.db 'PRAGMA foreign_keys;'` returns `1`.
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/rust-api.md` → updated.
8. `cargo deny check` → clean.

## Definition of Done
- [ ] Verification commands pass.
- [ ] `docs/interfaces/rust-api.md` reflects the new `Persistence` API.
- [ ] No `TODO` / `FIXME` / `todo!()` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/persist/Cargo.toml` (modified — sqlx, tokio)
- `crates/persist/src/lib.rs` (modified)
- `crates/persist/src/api.rs` (new)
- `crates/persist/migrations/.gitkeep` (new)
- `crates/persist/tests/migration_runner.rs` (new)
- `crates/core/Cargo.toml` (modified — depends on concerto-persist)
- `crates/core/src/main.rs` (modified — opens persistence on startup)
- `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-1: sqlite migration runner

crates/persist exposes Persistence::open which opens SQLite with WAL,
busy_timeout, foreign_keys, runs sqlx::migrate!, and PRAGMA
quick_check. No migrations yet — Task 09 adds the first.

Refs: tasks/08-sqlite-migration-runner.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** writer is a `Mutex<SqliteConnection>` for now; will need a real writer queue per design/09 §6.1 when concurrent writes appear (target task: ~20).
- **Smoke-gate state:** unchanged.
