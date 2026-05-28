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
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** —
- **Smoke-gate state:** unchanged.
