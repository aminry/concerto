# Task 112 — Audit-Log Rotation + `AuditLogSubscriber` Trait

| Field | Value |
|---|---|
| Phase | 1 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 108 |
| Touches subsystem(s) | 09 (Persistence), 18 (Distribution — trait seam) |
| Smoke gate | extends:audit-rotation |

## Goal
Generalize V0.1's single JSONL audit writer into the V1.0 audit pipeline `design/09` specifies: an `AuditLogSubscriber` trait (one of the `design/18 §3.7` extension seams) with the MIT-shipped implementations, plus file rotation. This locks the trait signature now so the V2.0 BSL `SiemForwarderSubscriber`/`EncryptedAtRestSubscriber` can be added later without a Core refork, and gives V1.0 syslog/HTTPS forwarding hooks.

## Inputs to read before starting
- `design/09_Persistence.md` §3.5 (audit is JSONL on disk, not in SQLite; `AuditLogSubscriber` trait; `id`/`on_event`/`flush`), §5.3 (`AuditWriter` — non-blocking batched append, fsync every 100 ms), §2 (V1.0: rotation + syslog forwarding hook).
- `design/18_Distribution_and_Operations.md` §3.7 (trait-seam registry — `AuditLogSubscriber` must have ≥1 OSS impl and reserve BSL variants).
- The existing V0.1 audit writer (`crates/core/src/audit*` — find it; V0.1 Task 44 added the JSON-Lines writer at `~/concerto/audit/`).
- `crates/core/src/runtime/` actor + `ActorContext` (audit is reachable via the persistence/notification handles).
- `tasks/v1.0/108-smoke-gate-refactor.md` → "Handoff Notes" — the `scripts/smoke.d/` layout the audit check plugs into.

## Scope — in
- Define `pub trait AuditLogSubscriber { fn id(&self) -> &str; async fn on_event(&self, event: &AuditEvent); async fn flush(&self); }` (match the design's signature; place it where the audit writer lives).
- V1.0 MIT impls: `JsonlFileSubscriber` (always present — refactor the existing writer to this), `StdoutSubscriber`, `SyslogSubscriber` (RFC 5424), `HttpsForwarderSubscriber`.
- Fan-out: the `AuditWriter` dispatches each event to all registered subscribers; `JsonlFileSubscriber` stays the always-on default; others are opt-in via config.
- **Rotation** in `JsonlFileSubscriber`: roll the daily file (and/or by size) keeping the `~/concerto/audit/` layout; retain per `design/12 §3.7` (90-day retention is policy — implement rotation + a retention setting, default documented).
- Reserve (commented, not implemented) the V2.0 BSL variant names in the trait-seam doc/registry so Task 707's completeness check passes.
- Tests: events reach all registered subscribers; rotation produces a new file at the boundary; a failing subscriber (e.g. HTTPS down) does not block the JSONL default or the foreground.

## Scope — out
- SIEM forwarding + at-rest encryption (V2.0 — only reserve the names).
- New audit *event kinds* (this task is the pipeline, not new events).
- The `managed.json` `auditEndpoint` wiring beyond reading a URL for `HttpsForwarderSubscriber` (full managed-settings enforcement is Phase 2, Task 211).

## Public interface this task locks
- Rust: `AuditLogSubscriber` trait signature (`id`/`on_event`/`flush`) — FROZEN; this is a published extension seam.
- The set of V1.0 OSS subscriber type names: `JsonlFileSubscriber`, `StdoutSubscriber`, `SyslogSubscriber`, `HttpsForwarderSubscriber`.
- The on-disk rotated-file naming under `~/concerto/audit/`.

## Implementation notes
- Keep the foreground non-blocking: the design's `AuditWriter` batches + fsyncs every 100 ms. Subscriber dispatch must not stall the caller; a slow/failing forwarder is isolated (spawn or bounded channel), and a dropped-on-overflow event is logged, not panicked.
- Don't reorder the JSONL default behind network subscribers — JSONL is the durable floor.
- Match the trait's async shape to how the rest of the codebase does async traits (check whether the workspace uses `async-trait` or native async-in-trait and follow it).

## Verification
Tier 1.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core audit` → fan-out, rotation, and failing-subscriber-isolation tests pass.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `scripts/smoke.sh` → the existing audit-log capability still passes; extend it to assert a rotated file naming once a boundary is crossed (or assert the JSONL default still writes). Exits 0.
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit the regen (new pub trait).

## Definition of Done
- [x] `AuditLogSubscriber` trait defined with the locked signature
- [x] Jsonl/Stdout/Syslog/HttpsForwarder impls; JSONL is the always-on default
- [x] Rotation + retention setting in `JsonlFileSubscriber`
- [x] Failing subscriber isolated from the JSONL default + foreground
- [x] V2.0 BSL variant names reserved for Task 707's registry check
- [x] Verification commands pass; smoke gate green; interfaces regenerated
- [x] Single commit created with the message below

## Outputs
- `crates/core/src/audit/` (modified/new — trait + impls + writer fan-out)
- `crates/core/tests/audit_subscribers.rs` (new)
- `scripts/smoke.d/<NN>-audit*.sh` (modified)
- `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-1: AuditLogSubscriber trait + rotation

Generalizes the V0.1 JSONL audit writer into an AuditLogSubscriber
fan-out (Jsonl/Stdout/Syslog/HttpsForwarder, JSONL always-on) with file
rotation + retention, and reserves the V2.0 BSL variants. Locks the
design/18 §3.7 extension seam.

Refs: tasks/v1.0/112-audit-log-subscribers.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **Trait signature.** Implemented the FROZEN framing signature (`fn id(&self) -> &str`,
    `async fn on_event(&self, event: &AuditEvent)`, `async fn flush(&self)`) — i.e. NO
    `Result` return. `design/09 §3.5` shows `-> Result<()>` and `+ 'static`; the task framing
    overrode that to the no-`Result` shape, which also matches the V0.1 subscriber contract
    (a subscriber must absorb its own errors — `tracing::warn!` + drop — so a failing
    subscriber can never propagate an error that stalls the fan-out or the foreground). The
    V0.1 method was named `emit`; renamed to `on_event` per the locked signature (only call
    sites were the audit module + `tests/audit_log.rs`, both updated).
  - **HTTP client = `reqwest` 0.13, rustls (NOT openssl/native-tls).** Pinned in
    `[workspace.dependencies]` as `reqwest = { version = "0.13", default-features = false,
    features = ["rustls", "json", "http2"] }` (reqwest 0.13 renamed the backend feature
    `rustls-tls` → `rustls`). `apps/desktop` already pulls reqwest+rustls transitively via
    `tauri-plugin-updater`, so the license posture (rustls/ring/webpki-root-certs/CDLA) was
    already ratified in `deny.toml`; `cargo deny check` is green (advisories/bans/licenses/
    sources all ok). Cross-platform: no `std::os::unix` / `UnixStream` / libc in
    `crates/core/src/audit/` (grep-confirmed) — builds on the Windows CI lane.
  - **Syslog = network RFC 5424 over UDP/TCP** (`host:port` from config), NOT libc
    `syslog()` / `/dev/log` / Unix-domain sockets — cross-platform + that's what a forwarding
    hook needs. TCP uses RFC 6587 octet-counting framing and reconnects lazily.
  - **Config wiring: only the JSONL default is registered at boot.** `boot.rs` still spawns
    `AuditWriterTask::spawn(vec![jsonl_subscriber], ...)` — the always-on durable floor. The
    opt-in Stdout/Syslog/Https subscribers are fully implemented + tested but NOT yet wired to
    a config source (no `managed.json` / settings plumbing reads an audit-forwarders list
    yet). That's the deliberate seam below — appending them is a one-line `vec!` change once a
    config field exists. JSONL is registered FIRST and is never reordered behind a network
    subscriber.
  - **Rotation/retention defaults.** Daily UTC rotation (unchanged) + NEW optional size
    rotation (`RotationConfig::max_bytes`, default `None` → off; rolls to
    `audit-<YYYY-MM-DD>.<seq>.jsonl`) + NEW retention (`RotationConfig::retention_days`,
    default `DEFAULT_RETENTION_DAYS = 90` per `design/12 §3.7`; pruned on each daily roll).
    Defaults preserve V0.1 behaviour exactly.
- **Open questions for next task:**
  - Where does the opt-in forwarder config live? Likely a `managed.json` audit-forwarders
    list (Phase 2, Task 211 owns full `managed.json` enforcement) or a project/local setting.
    Decide the schema and wire the `vec!` in `boot.rs` then.
  - `size`-based rotation defaults to off — if ops wants a default cap (e.g. 100 MB) that's a
    policy call for the managed-settings task.
- **Deliberate debt:**
  - Stdout/Syslog/Https subscribers are implemented + unit/integration-tested but registered
    NOWHERE by default (JSONL-only at boot). Clear seam: extend the `boot.rs` subscriber `vec!`
    once a config field exists. No `TODO`/`unimplemented!()` left.
  - V2.0 BSL variant names `SiemForwarderSubscriber` + `EncryptedAtRestSubscriber` are RESERVED
    as comments only (in `audit/mod.rs` module doc + the `AuditLogSubscriber` trait doc in
    `audit/writer.rs`) per `design/18 §3.7`, for Task 707's trait-seam completeness check.
  - `regen-interfaces.sh` produces NO diff for this trait: the generator scans
    `crates/<crate>/src/api.rs` at depth exactly 3, so `crates/core/src/audit/api.rs` (depth 4)
    is not indexed and `crates/core/src/api.rs` doesn't exist. `git diff --exit-code
    docs/interfaces/` is clean. The audit public surface is still gathered in `audit/api.rs`
    for convention; if a future task wants it in `rust-api.md`, the generator's depth rule (or a
    `crates/core/src/api.rs` re-export) is the lever.
- **Smoke-gate state:** `extends:audit-rotation`. Kept `60-audit-log.sh` green (JSONL default
  still records `workspace_created` under the new fan-out). Added
  `scripts/smoke.d/65-audit-rotation.sh` (`check_audit_rotation`) + appended `audit-rotation`
  to `scripts/smoke.manifest` after `audit-log`. It asserts: today's daily file exists with the
  `audit-<YYYY-MM-DD>.jsonl` rotated-naming convention, every file in the audit dir matches the
  rotated-naming glob (primary or `.<seq>.` sibling), and the JSONL default carries the
  `workspace_created` row routed through the fan-out. The true day-boundary rotation is covered
  by the Rust test (`daily_rotation_opens_new_file_at_boundary`) since the smoke run can't
  cross a real UTC midnight. `scripts/smoke.sh --ci-mode` exits 0 with `PASS audit-log` +
  `PASS audit-rotation`; `--list` shows the new cap; `shellcheck` clean.
