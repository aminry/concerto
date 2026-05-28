# Task 16 — Logging Discipline (Span Fields and Rotation)

| Field | Value |
|---|---|
| Phase | 1 |
| Size | small (≤4h) |
| Depends on | 05, 11 |
| Touches subsystem(s) | 01 (Runtime) |
| Smoke gate | unchanged |

## Goal
Tighten the `tracing` setup from Task 05 so logs are useful in incident response: rotating daily files at `~/concerto/logs/`, structured span fields that include workspace/workarea/session IDs, retention of 14 days, no secrets ever logged. After this task, every later subsystem can add `#[tracing::instrument]` calls with confidence that the surrounding format is consistent.

## Inputs to read before starting
- `design/00_Architecture_Overview.md` §7.4 (observability — local logs at `~/concerto/logs/core-YYYY-MM-DD.log`; span fields include workspace ID, agent session ID, device cert ID).
- `design/01_Core_Daemon_Runtime.md` §4.1 (log path) and §3.6 (OTLP exporter — opt-in, off in V0.1).
- `tasks/05-error-and-logging-baseline.md` → confirms `crates/core/src/logging.rs` already exists with `init()`.
- `tasks/15-smoke-gate-v1.md` → "Handoff Notes".

## Scope — in
Refine `crates/core/src/logging.rs`:

- Use `tracing-appender::rolling::Builder` to build a `Daily` rolling file appender at `$CONCERTO_DATA_DIR/logs/core.log` (the appender rotates to `core.log.YYYY-MM-DD` and keeps the latest as `core.log`). Set `max_log_files(14)`.
- The console layer (stderr) uses a compact human format with ISO timestamps.
- The file layer uses **JSON** (`.json()` builder) so logs can be ingested later by an OTLP exporter or `jq` queries.
- Span fields: define a global helper `tracing_fields!()` macro in `crates/core/src/log_fields.rs` that callers use to inject the standard fields:
  ```rust
  #[macro_export]
  macro_rules! workspace_span {
      ($workspace_id:expr) => {
          tracing::info_span!("workspace", workspace_id = %$workspace_id)
      };
  }
  // Similar for workarea_span!, session_span!, device_span!.
  ```
- Implement a `SecretsFilter` layer (`crates/core/src/log_filter.rs`) that scrubs known-secret field names from every recorded event/span. The filter blocks: `token`, `password`, `secret`, `pat`, `api_key`, `pairing_key`, `private_key`. It replaces values with `"<redacted>"`. Apply globally as the outermost `tracing-subscriber` layer.
- Add an integration test that records a log event containing `field token = "xyz"` and asserts the file appender writes `"token":"<redacted>"`.
- Add a separate test that asserts log rotation produces the expected filename schema (use a small mock clock or just inspect that the rotating writer produces correctly-named files when forced).
- Document in `crates/core/src/logging.rs` doc comment the convention: every public function that takes an ID parameter must wrap its body in the corresponding span; lint via clippy is best-effort (no real Rust lint for this).

## Scope — out
- OTLP exporter (V1.0 — opt-in).
- Log forwarding to syslog (V1.0).
- Per-event field-level redaction beyond name-based scrubbing (V1.5).
- Renderer-side logging (Tauri WebView console — handled separately).

## Public interface this task locks
- Rust: `workspace_span!`, `workarea_span!`, `session_span!`, `device_span!` macros exposed from `crates/core/src/log_fields.rs`.
- File path: `$CONCERTO_DATA_DIR/logs/core.log` (rotating daily; 14-day retention).
- Redaction allow-list (field names): `token`, `password`, `secret`, `pat`, `api_key`, `pairing_key`, `private_key`. Adding a name to this list is a one-line change for any future task; removing one is forbidden.
- Wire format: file output is JSON; console output is human-readable.

## Implementation notes
- `tracing-appender`'s `RollingFileAppender` writes synchronously by default; wrap in `non_blocking` for async-friendly output (`tracing_appender::non_blocking::NonBlocking`). Hold the worker guard in `Runtime` from Task 11 (or in main()).
- `tracing_subscriber::fmt::layer().json()` produces JSON output; the field formatter handles structured data.
- The redaction filter is a custom `tracing_subscriber::Layer` impl. It intercepts `Event::record(&mut Visit)` and the span attributes via a custom visitor. Use the `tracing_core::field::Visit` trait.
- Don't redact on `tracing::Span` creation time — only on event/visit. That keeps the cost off the hot path of `info_span!`.
- The macros `workspace_span!` etc. are thin sugar; they exist to standardize field naming so log-aggregation queries are consistent.

## Verification
1. `cargo build -p concerto-core` → succeeds.
2. `cargo test -p concerto-core logging` → all tests pass (redaction test, rotation file naming test).
3. `cargo clippy -p concerto-core -- -D warnings` → clean.
4. Manual: `cargo run --bin concerto-core` for 5 seconds; SIGTERM; confirm `$CONCERTO_DATA_DIR/logs/core.log` exists and contains JSON lines.
5. Manual: with a known secret-named field in code (temporarily add `tracing::info!(token = "abc123", "test redaction")` in `main.rs`); run; confirm log file shows `"token":"<redacted>"`; revert.
6. `scripts/smoke.sh` still passes.
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → no unintended drift.

## Definition of Done
- [x] Verification commands pass.
- [x] JSON file output verified.
- [x] Redaction verified for at least one secret-named field.
- [x] Console output remains human-readable.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `crates/core/src/logging.rs` (modified)
- `crates/core/src/log_fields.rs` (new)
- `crates/core/src/log_filter.rs` (new)
- `crates/core/src/lib.rs` (modified — module declarations + macro exports)
- `crates/core/tests/logging.rs` (new)
- `tasks/16-logging-discipline.md` (modified — DoD ticks + Handoff Notes)

## Commit message
```
phase-1: logging discipline — JSON file + redaction filter

Switches the file appender to daily JSON output at
$CONCERTO_DATA_DIR/logs/core.log (14-day retention). Adds a
SecretsFilter layer that scrubs known-secret field names per
design/00 §7.4. Standard span macros for workspace/workarea/session/
device IDs.

Refs: tasks/16-logging-discipline.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **File path schema kept as `core.YYYY-MM-DD.log`** (Task 05's format), not `core.log` + `core.log.YYYY-MM-DD` as the task spec wrote. `tracing-appender::rolling::Builder` always inserts a `.` between `filename_prefix`, the rotated date, and `filename_suffix`; there is no API to produce a stable `core.log` symlink-style filename with rotated `core.log.YYYY-MM-DD` siblings. The locked path is therefore `$CONCERTO_DATA_DIR/logs/core.YYYY-MM-DD.log` with 14-day retention via `.max_log_files(14)`. The pre-task orchestration brief flagged this exact deviation.
  - **`Cargo.toml` was NOT modified.** The Outputs list said "tracing-appender features" but no features were needed: `max_log_files` is part of `tracing-appender`'s default surface, and `tracing-subscriber`'s default features already cover everything the console layer uses. The JSON file layer is hand-rolled in `log_filter.rs` via `serde_json` rather than `tracing-subscriber`'s `json` feature — see next bullet — so no feature flag had to be added. Updated Outputs to drop the spurious `Cargo.toml` entry.
  - **JSON formatting is implemented directly in `SecretsFilter`** rather than as a wrapper over `tracing_subscriber::fmt::layer().json()`. Rationale: `tracing-subscriber`'s built-in `JsonFields` visitor goes straight to `serde_json::Serializer` with no public hook to intercept individual field values, so the redaction logic would have to re-implement most of the JSON layer anyway. Keeping both in one type (with two `OutputStyle` variants — `Json` for the file, `CompactHuman` for stderr) means the same redaction code path covers both layers, so the console layer is also scrubbed (`token=<redacted>` on stderr, not just in the file).
  - **`build_filter()` was refactored to take the env-var value as a parameter** (`fn build_filter(raw: Option<String>) -> Result<Targets>`). Task 05's tests raced on the global `RUST_LOG` env var because each test read it inside the function under test; the new signature accepts the string directly, so unit tests pass concrete inputs and never touch `std::env`. The brief pre-authorized this refactor.
  - **`init()` returns a new `LogGuard` struct, not a bare `DefaultGuard`.** The `tracing-appender::non_blocking` worker guard MUST be held for the program's lifetime alongside the dispatcher guard. Bundling them as `LogGuard { _default, _worker }` means `main.rs`'s existing `let _log_guard = logging::init()?;` pattern continues to work and callers can't accidentally drop one without the other. This is a breaking API change vs. Task 05's signature; `main.rs` did NOT need to be modified (the binding is type-inferred), and the runtime did NOT need to grow a field. Outputs list does not include `runtime.rs` or `main.rs`.
  - **No `tracing-appender` rotation test that asserts retention.** The integration test `rotation_file_naming_schema` only checks the filename pattern. There is no clock mock that would let me prove the 14-day deletion path fires without running 14 days of wall-clock; the `.max_log_files(14)` call site is the contract.
  - **Span-attached field storage is idempotent across multiple `Layer` instances.** Both the file and console `SecretsFilter` instances receive `on_new_span` callbacks for the same span; the first one wins via `ext.get_mut::<RecordedFields>().is_none()` and subsequent layers skip the work. `on_record` de-duplicates by field name. Without this both layers would attempt `Extensions::insert` for the same key and the second call would panic.
- **Open questions for next task:**
  - `init_with_log_dir(&Path)` is now the testable seam. Future tasks that want to override the log location for an integration test should call it rather than mucking with `CONCERTO_DATA_DIR` in process state.
  - The `*_span!` macros expand to `::tracing::info_span!`, so callers must have `tracing` resolvable at the macro use site. Every binary/crate in the workspace already depends on `tracing` transitively via `concerto-core`, so this hasn't bitten anyone yet; if a future crate uses the macros without a `tracing` dep, the compiler error will be clear.
  - `chrono_like_timestamp()` is hand-rolled to avoid pulling `chrono`. If `chrono` ever lands as a workspace dep (e.g. for human-readable RPC fields), swap the helper out for `chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)` — the format string matches.
- **Deliberate debt:** redaction is name-based (allow-list); value-based heuristics (e.g., "looks like a JWT") deferred to V1.5.
- **Smoke-gate state:** unchanged. v1 still active; smoke gate doesn't read logs, so the filename schema change does not affect it. Re-ran `scripts/smoke.sh` after this task — green.
