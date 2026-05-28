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
- [ ] Verification commands pass.
- [ ] JSON file output verified.
- [ ] Redaction verified for at least one secret-named field.
- [ ] Console output remains human-readable.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/core/Cargo.toml` (modified — tracing-appender features)
- `crates/core/src/logging.rs` (modified)
- `crates/core/src/log_fields.rs` (new)
- `crates/core/src/log_filter.rs` (new)
- `crates/core/src/lib.rs` (modified — module declarations + macro exports)
- `crates/core/tests/logging.rs` (new)

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
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** redaction is name-based (allow-list); value-based heuristics (e.g., "looks like a JWT") deferred to V1.5.
- **Smoke-gate state:** unchanged.
