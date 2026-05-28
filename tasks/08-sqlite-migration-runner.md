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
- [x] Verification commands pass.
- [x] `docs/interfaces/rust-api.md` reflects the new `Persistence` API.
- [x] No `TODO` / `FIXME` / `todo!()` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

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
- **Drift from plan:**
  - **`dirs::data_dir()` → `home::home_dir()` + `concerto/concerto.db`.** The task's Implementation notes said to use `dirs::data_dir()`, but `dirs` is not in the workspace — Task 05 removed it because it transitively brings MPL-2.0 `option-ext`, which `deny.toml` (per design/00 §6.11) refuses. Substituted `home::home_dir()` and the literal `~/concerto/concerto.db` path, matching the pattern already used in `crates/core/src/logging.rs` for `~/concerto/logs/`. Exposed via `PersistenceConfig::default_for_user()`.
  - **`impl AsyncDeref<SqliteConnection>` → concrete `WriterGuard<'a>`.** The task's API sketch returned `impl AsyncDeref<SqliteConnection>`, but `AsyncDeref` is not a stable trait in Rust. Substituted a concrete newtype `WriterGuard<'a>` that wraps `tokio::sync::MutexGuard<'_, SqliteConnection>` and implements `Deref` / `DerefMut`. Call-site syntax stays `let mut w = persist.writer().await; sqlx::query(...).execute(&mut *w).await?;`. When Task ~20 introduces the dedicated writer task + mpsc queue per design/09 §6.1, `WriterGuard` becomes the receipt the caller awaits on; the public contract that callers get exclusive write access through `Persistence::writer()` is preserved.
  - **`signal` Tokio feature added** to `crates/core` so the binary can wait on SIGTERM/SIGINT for a graceful shutdown. The task said "SIGTERM cleanly closes" in Verification step #4 but didn't list the wire-up; this is the minimum to satisfy it. The runtime supervisor task (Task 11) will take this signal handler over.
  - **`crates/core/src/main.rs` switched from `fn main() -> Result<()>` to a tokio-runtime-driven `fn main() -> ExitCode`.** Required because `Persistence::open` is async. Errors bubble out of `run()` and the binary returns `ExitCode::from(1)` rather than panicking — explicit per Implementation note #4.
- **Open questions for next task:**
  - **Migration directory has a `.gitkeep` placeholder.** `sqlx::migrate!("./migrations")` succeeds against an empty directory (it just records zero migrations). Task 09 should drop its first migration `0001_initial_schema.sql` into `crates/persist/migrations/` and run; no plumbing changes needed.
  - **Forwarded from Task 07:** `optional PermissionMode` semantically means "inherit from parent" per design/03 §3.2, distinct from `PERMISSION_MODE_UNSPECIFIED` and from `PERMISSION_MODE_STRICT`. Task 09's schema should encode this — likely with a nullable `permission_mode` TEXT column, where NULL means "inherit". The proto's `optional` field maps naturally to NULL.
  - **Forwarded from Task 07:** proto `status` fields on `Workspace` / `Workarea` / `Session` are typed `string` with allowed-value lists in proto comments. Task 09 likely wants `TEXT NOT NULL` with a `CHECK` constraint enumerating the values, matching what the proto comments declare (`workareas.status ∈ { created | active | running | awaiting | paused | archived | crashed }` etc.). The validator in the gRPC server middleware (Task 13) reads the same list.
  - **Forwarded from Task 07:** `sessions.agent_kind` accepts `gemini` for forward compatibility, but V0.1 wires only claude + codex per `tasks/README.md §2`. Task 09's CHECK constraint should include `gemini` even though no agent-supervisor code emits it yet, so the schema doesn't need a migration when gemini lands post-V0.1.
  - **`PRAGMA foreign_keys` is per-connection.** Running `sqlite3 ~/concerto/concerto.db 'PRAGMA foreign_keys;'` from the sqlite3 CLI returns `0` because each new connection starts with FK off; our connections turn it on via `SqliteConnectOptions::foreign_keys(true)`. Task 09 + downstream tests should obtain connections via the `Persistence` handle, never by opening the DB file directly. The unit test `pragmas_match_design_doc` asserts the in-process value is correct.
  - **`Persistence` is not `Clone`** by design — only one runtime actor owns persistence; sub-systems borrow `&Persistence`. If Task 11 / Task 19 want to hand a clonable handle around, wrap in `Arc<Persistence>` at the call site rather than deriving `Clone` here.
- **Deliberate debt:**
  - Writer is a `tokio::sync::Mutex<SqliteConnection>` rather than the dedicated single-writer task + mpsc queue described in design/09 §6.1. The `WriterGuard` newtype is the seam where that migration happens. Target task: **~20** (the first task that creates a real concurrent-writer scenario by introducing the workspace-creation RPC). No `TODO` comment is left in code — the deferred design is documented in the `WriterGuard` doc-comment plus this note.
  - `Persistence::shutdown` asserts that no `WriterGuard` is alive at shutdown time by `Arc::try_unwrap`. The error message is descriptive but the failure path is untested; integration tests for shutdown contention land alongside the dedicated writer task in ~20.
- **Smoke-gate state:** unchanged — Task 15 is the first that flips the smoke gate to v1. This task added no new smoke-gate assertions; `scripts/smoke.sh` still emits "PASSED (no checks active yet — Phase 0)".
