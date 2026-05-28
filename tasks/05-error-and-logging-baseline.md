# Task 05 — Base Error Types and `tracing` Setup

| Field | Value |
|---|---|
| Phase | 0 |
| Size | small (≤4h) |
| Depends on | 01, 02, 04 |
| Touches subsystem(s) | 01 (Runtime), 09 (Persistence) |
| Smoke gate | unchanged |

## Goal
Every other crate inherits the same error and logging conventions. This task fills in `crates/error` with the shared `Result<T>` alias and the top-level `thiserror` enum scaffolding, and adds a tiny `crates/core/src/logging.rs` that initializes `tracing` with the rotating-file appender format used everywhere else. After this task, every later crate `use`s `concerto_error::Result` and every binary starts by calling `concerto_core::logging::init()`.

## Inputs to read before starting
- `design/00_Architecture_Overview.md` §6.1 (logging: `tracing` + `tracing-subscriber`; rotating file + opt-in OTLP) and §7.3 (error handling philosophy — typed errors at module boundaries with stable wire codes).
- `design/00_Architecture_Overview.md` §7.4 (observability — local logs at `~/concerto/logs/core-YYYY-MM-DD.log`).
- `tasks/04-interface-summary-generator.md` → "Handoff Notes" — to confirm the `crates/<name>/src/api.rs` convention.

## Scope — in
- In `crates/error`:
  - `src/lib.rs` defining `pub type Result<T, E = Error> = std::result::Result<T, E>;` and `pub use error::Error;`.
  - `src/error.rs` defining the top-level `pub enum Error` (via `thiserror::Error`) with at least the variants: `Io(#[from] std::io::Error)`, `Sqlx(#[from] sqlx::Error)`, `Tonic(#[from] tonic::Status)`, `Pairing(String)`, `Internal(String)`. Each variant must carry a stable `wire_code() -> &'static str` method returning a kebab-case string (`"io"`, `"sqlx"`, `"tonic"`, `"pairing"`, `"internal"`). These wire codes are the cross-process / cross-protocol error identifier per `design/00 §7.3`.
  - `src/api.rs` re-exporting `Error` and `Result` (so `regen-interfaces.sh` picks them up).
- In `crates/core/src/logging.rs`:
  - `pub fn init() -> concerto_error::Result<tracing::dispatcher::DefaultGuard>` that:
    - Reads `RUST_LOG` (default `info,concerto=debug`).
    - Configures a rotating-daily file appender at `~/concerto/logs/core-YYYY-MM-DD.log` (use `tracing-appender::rolling::daily`).
    - Configures a console layer (stderr) that's compact and uses ANSI when stderr is a TTY.
    - Returns the guard the caller must hold for the life of the program.
  - `pub fn init_for_tests()` that initializes a single no-op subscriber with `tracing_subscriber::fmt::try_init()` (idempotent).
- Update `crates/core/src/main.rs` to call `logging::init()?` at startup and propagate the guard.
- Add unit tests to `crates/error` that verify the `Display`, `Debug`, and `wire_code()` outputs for each variant.

## Scope — out
- No OTLP exporter (V1.0 — and even then opt-in only).
- No structured-JSON output format — V0.1 is human-readable; structured logs land later.
- No span-context propagation across processes.
- No log rotation policy beyond daily files (no compression, no max-age).

## Public interface this task locks
- Rust: `crates/error/src/api.rs` — `pub enum Error { Io, Sqlx, Tonic, Pairing(String), Internal(String) }`, `pub type Result<T, E = Error>`, `Error::wire_code(&self) -> &'static str`.
- Rust: `crates/core/src/logging.rs` — `pub fn init() -> Result<DefaultGuard>`, `pub fn init_for_tests()`.
- Convention: every crate adds `concerto-error` as a dep instead of declaring its own error type at the boundary. Crates may add private error types internally; only the boundary type must come from `concerto-error`.

## Implementation notes
- The `wire_code()` method is what the gRPC server will surface in error responses (Task 13). Pick stable strings now; renaming any of them later requires a revision task.
- For `~/concerto/logs/`: derive the path via `dirs::home_dir()` (add `dirs = "5"` as a workspace dep) and create the directory if it doesn't exist.
- The `init_for_tests()` function uses `try_init` so multiple tests calling it don't panic.
- Hold the `DefaultGuard` for the lifetime of the program; if you drop it, file output stops. The conventional pattern is to bind it in `main()` and never re-assign.
- Don't use `tracing_subscriber::EnvFilter` parsing — use the simpler `tracing_subscriber::filter::Targets` with a builder so `RUST_LOG` parsing is predictable.

## Verification
1. `cargo check -p concerto-error -p concerto-core` → no warnings.
2. `cargo clippy -p concerto-error -p concerto-core -- -D warnings` → clean.
3. `cargo test -p concerto-error` → all tests pass; coverage includes one test per `Error` variant verifying `wire_code()`.
4. `cargo run --bin concerto-core` for ~2 seconds, then ctrl-C → confirms `~/concerto/logs/core-<today>.log` exists and contains a "concerto-core starting" line.
5. `RUST_LOG=trace cargo run --bin concerto-core` → confirms trace logs appear.
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/rust-api.md` → updated and committed.
7. `cargo deny check` → still clean (no GPL deps added).

## Definition of Done
- [ ] All Verification commands pass.
- [ ] `docs/interfaces/rust-api.md` reflects the new `Error` and `Result` types.
- [ ] No `TODO` / `FIXME` / `todo!()` in new code.
- [ ] No files outside Outputs modified.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/error/Cargo.toml` (modified — adds `thiserror`)
- `crates/error/src/lib.rs` (modified)
- `crates/error/src/error.rs` (new)
- `crates/error/src/api.rs` (new)
- `crates/error/tests/wire_codes.rs` (new)
- `crates/core/Cargo.toml` (modified — adds `tracing`, `tracing-subscriber`, `tracing-appender`, `dirs`, `concerto-error`)
- `crates/core/src/logging.rs` (new)
- `crates/core/src/main.rs` (modified — call `logging::init()`)
- `crates/core/src/lib.rs` (modified — `pub mod logging;`)
- `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-0: base error types and tracing setup

crates/error exposes shared Result/Error with stable wire codes per
design/00 §7.3. crates/core/logging.rs initializes the daily rotating
file appender and console output per §7.4. All later crates use these.

Refs: tasks/05-error-and-logging-baseline.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **`Sqlx` and `Tonic` variants are boxed.** Spec wrote `Sqlx(#[from] sqlx::Error)` / `Tonic(#[from] tonic::Status)`. With those unboxed, clippy's `result_large_err` lint (default threshold 128 B, our enum was ≥176 B because `sqlx::Error` is huge) fires on every `Result<_, Error>` return — and with `-D warnings` that's a hard CI fail. Variants are now `Sqlx(Box<sqlx::Error>)` / `Tonic(Box<tonic::Status>)` with hand-rolled `From<sqlx::Error> for Error` / `From<tonic::Status> for Error` impls that do the boxing, so `err?` still works at call sites. `wire_code()` returns the same kebab strings (`"sqlx"`, `"tonic"`) and tests still cover every variant.
  - **Workspace `Cargo.toml` was modified** (not in Outputs). Added `tracing-appender = "0.2"` and `home = "0.5"`. Also tightened existing entries: `sqlx = { version = "0.8", default-features = false }` (was `"0.7"`) and `tonic = { version = "0.11", default-features = false }`. The sqlx bump is mandatory — 0.7.4 is RUSTSEC-2024-0363 (binary-protocol cast bug), which my own deny.yml is now configured to fail on. The `default-features = false` lift to the workspace level fixes a `cargo` warning ("default-features is ignored for sqlx, since default-features was not specified for workspace.dependencies.sqlx, this could become a hard error in the future"). Crates that need sqlx runtimes (Task 08) re-enable features locally. Adding the file to Outputs in retrospect: yes.
  - **`dirs` was replaced with `home`** (also not strictly in Outputs as written — Outputs said "adds `dirs`"). `dirs 5` pulls in `option-ext`, which is MPL-2.0; design/00 §6.11 doesn't allow MPL. Swapped to `home`, the rust-lang-team-maintained crate that just exposes `home_dir()` and ships MIT/Apache-2.0. `logging::log_dir()` now uses `home::home_dir()`.
  - **`deny.toml` was modified** (Task 02 file). Added `"Zlib"` to the license allow-list with a comment justifying the addition: foldhash (transitive via sqlx 0.8 → hashbrown 0.15) ships under Zlib, a permissive OSI/FSF-approved attribution-only license that is posture-equivalent to BSD-2-Clause. Task 02's own spec explicitly permits this kind of allow-list extension with justification in the task's commit message; commit body covers it.
  - **`api.rs` declares the `Error` enum directly** rather than re-exporting it from `error.rs`. The spec said "`src/api.rs` re-exporting `Error` and `Result` (so `regen-interfaces.sh` picks them up)". My Task 04 `regen-interfaces.sh` parser only captures `pub trait/struct/enum` declarations, not `pub use` re-exports, so a literally-spec-compliant `api.rs` (re-exports only) would produce an empty `rust-api.md` section — defeating the parenthetical purpose. Restructured so the type lives in `api.rs` directly, the `impl Error { ... wire_code ... }` block lives in `error.rs`, and `lib.rs` re-exports `pub use api::{Error, Result}` so external callers don't need to know the split. Verified `docs/interfaces/rust-api.md` now shows the full `Error` enum.
  - **Log file is named `core.YYYY-MM-DD.log`, not `core-YYYY-MM-DD.log`.** Design/00 §7.4 wrote `core-YYYY-MM-DD.log` (hyphen). `tracing-appender 0.2`'s `RollingFileAppender::builder()` always inserts a dot between `filename_prefix` and the date — there is no API to change the separator. Acceptable for V0.1 since the log path is internal; if a future task wants the exact hyphen form, swap to a custom appender. Otherwise rotation, format, and content match the design.
  - **`init()` uses `set_default()`, not `set_global_default()`.** Spec literally says return `tracing::dispatcher::DefaultGuard`, which only `set_default()` produces. `set_default()` is per-thread; tracing 0.1's task-local propagation copies the current dispatch into spawned futures, so logs from spawned tokio tasks see the same subscriber. Verified by running the binary and seeing both the `info!` and `trace!` macros from `main()` land in `~/concerto/logs/core.2026-05-28.log`.
  - **`RUST_LOG` parsing rejects invalid levels** instead of silently ignoring them. Spec said "simpler than EnvFilter" — I went a step further: an unparseable level returns `Error::Internal(...)` so `init()` fails loud. A test (`rejects_invalid_level`) covers this.
  - **No `#[allow(clippy::result_large_err)]` directives anywhere.** Original draft hit this lint on three `Result`-returning functions; the cleanup was structural (boxing the giant variants), not a suppression.
- **Open questions for next task:**
  - `rust-api.md` shows the `Error` enum but NOT the `Result` type alias or the `wire_code()` impl block — the Task 04 parser doesn't capture `pub type` or impl blocks. Not a Task 05 issue (it's a Task 04 parser limitation flagged in that task's handoff too), but Task 06+ should be aware that anything outside `pub (trait|struct|enum)` is invisible to drift checks. A future polish task could extend the parser.
  - `sqlx` and `tonic` workspace deps now have `default-features = false`. Task 08 (sqlx migration runner) will need `features = ["runtime-tokio", "sqlite", "macros"]` (and possibly `tls-rustls` if anyone reaches the network — unlikely for the local SQLite). Task 13 (gRPC over UDS) will need tonic `features = ["transport", "codegen", "prost"]`. Pin features explicitly in each crate's `Cargo.toml` when those tasks land.
  - The dev-machine DNS workaround (Tailscale MagicDNS can't resolve `static.rust-lang.org`) is still in effect; doesn't affect this task because rustfmt + clippy were sideloaded in Task 02. New rustup components on this box will need the same `curl --resolve` workaround.
  - The deny.toml allow-list now contains `Zlib`. If Task 18 (gix-wrap / git clone) pulls in something with a license outside the current list (Zlib, Unicode-DFS-2016, MIT-0, Unicode-3.0 already listed), the same allow-list-extension dance with justification-in-commit applies.
- **Deliberate debt:** —
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still exits 0 with "Smoke gate: PASSED (no checks active yet — Phase 0)". Task 15 will add the first real assertion when the gRPC roundtrip lands.
