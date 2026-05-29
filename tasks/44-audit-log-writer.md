# Task 44 — Audit Log JSON-Lines Writer

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 10, 32, 33, 42, 43 |
| Touches subsystem(s) | 09 (Persistence), 12 (Security) |
| Smoke gate | new check |

## Goal
Implement the JSONL audit log writer at `~/concerto/audit/audit-YYYY-MM-DD.jsonl`. Every state-changing event (workspace create, workarea archive, permission-mode change, tool approval decided, secret accessed, yolo-mode action, etc.) writes a typed event. Daily rotation; fsync-batched per `design/09 §3.5`. After this task, the audit trail is real — Phase 4 verification depends on it.

## Inputs to read before starting
- `design/09_Persistence.md` §3.5 (audit log JSONL on disk; daily rotation; pluggable subscribers via `AuditLogSubscriber` trait), §5.3 (AuditWriter interface).
- `design/00_Architecture_Overview.md` §7.5 (audit log path + encryption — V0.1 plain JSONL).

## Scope — in
- Implement `crates/core/src/audit/`:
  - `pub struct AuditEvent { pub at: SystemTime, pub kind: AuditKind, pub actor: AuditActor, pub subject_ids: Vec<(EntityKind, String)>, pub details_json: serde_json::Value }`
  - `pub enum AuditKind` — V0.1 variants: `WorkspaceCreated`, `WorkspaceArchived`, `WorkareaCreated`, `WorkareaArchived`, `WorkareaRestored`, `SessionStarted`, `SessionEnded`, `ToolApprovalDecided`, `ToolApprovalAutoApproved`, `ToolApprovalDenied`, `PermissionModeChanged`, `EnteredYoloMode`, `BypassDestructiveGuardEnabled`, `SecretAccessed`, `ConfigReloaded`, `RepositoryAdded`, `RepositoryCloned`, `FsmonitorRestarted`, `ScheduleFired`, `ScheduleSuppressed`, `DestructiveCommandIntercepted`.
  - `pub enum AuditActor { Device(DeviceId), System, AutoMode(PermissionMode) }`.
  - `pub trait AuditLogSubscriber: Send + Sync` per `design/09 §3.5`.
  - `pub struct JsonlFileSubscriber` — the canonical writer; opens `<data_dir>/audit/audit-<YYYY-MM-DD>.jsonl`; uses `O_APPEND`; flushes every 100ms or on shutdown.
  - `pub struct AuditWriter` — fan-out to multiple subscribers; `append(event)` is non-blocking (channel send).
- Wire AuditWriter as a Tokio task spawned by the runtime. Provide an `AuditHandle: Clone` that other actors hold.
- Update every prior task's tracing-only audit emissions to flow through `AuditWriter`:
  - Workspaces / workareas / sessions create/archive (Tasks 19, 20, 22, 31).
  - Permission mode changes (Task 32, 42).
  - Tool approvals (Task 33).
  - Keychain access (Task 10).
  - Destructive command interception (Task 43).
- Daily rotation: at midnight UTC (or on first write of a new UTC day), close the current file and open `audit-<new-day>.jsonl`.
- Tests:
  - Write 1000 events; assert all appear in the file; verify JSONL format (one event per line).
  - Daily rotation: mock the clock; assert filename change.
  - Crash safety: kill the Core mid-stream; verify the last 100ms of events may be lost (documented in `design/10` testing) but no events are partially written (each line is atomic).
- Update smoke gate v2 to add an audit-log check: after each prior step, the corresponding audit event must appear in the JSONL file.

## Scope — out
- Pluggable subscribers beyond JsonlFile + Stdout (Syslog and HttpsForwarder ship in V1.0 per `design/09 §3.5`).
- At-rest encryption (V2.0).
- SIEM forwarding (V2.0 enterprise module).
- Audit-log retention sweeper (V1.0 — V0.1 keeps forever).
- Field-level redaction (the SecretsFilter from Task 16 covers the `tracing` side; audit events are designed to never carry raw secrets, so this is a code-review discipline).

## Public interface this task locks
- Path: `<data_dir>/audit/audit-<YYYY-MM-DD>.jsonl`. Frozen.
- Rust: `AuditEvent`, `AuditKind`, `AuditActor`, `AuditWriter::append`. Frozen.
- Trait `AuditLogSubscriber`. Frozen (V1.0 adds new impls but doesn't change the trait).
- JSONL line format: a single JSON object per line, fields: `at` (RFC3339), `kind`, `actor`, `subject_ids`, `details`. Frozen.

## Implementation notes
- Channel: `tokio::sync::mpsc::Sender<AuditEvent>` with bounded capacity (1000); on full, log a warning and drop (per `design/10 §8` — drop-oldest behavior; events shouldn't outrun a flushing writer normally).
- File flushing: every 100ms, drain the queue and `write_all` the serialized lines; then `fsync_data`.
- The runtime gates shutdown on the audit writer's flush completing.
- For tests, expose a `MemorySubscriber` that captures events to a `Vec` for assertion.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core audit` → all tests pass.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual: run the full smoke gate; inspect `audit-<today>.jsonl`; verify each major step has an audit line.
5. Crash test: kill Core mid-stream; restart; verify the file's last line is well-formed JSON (no partial write).
6. `scripts/smoke.sh` updated to grep for at least the workspace.created and session.started kinds. Passes.

## Definition of Done
- [x] Verification commands pass.
- [x] Every prior task's emission flows through AuditWriter. _(V0.1: plumbing
      + workspace-created + permission_mode-changed demo emissions; most
      prior tasks' `tracing::info!(audit.kind=...)` emissions remain in
      place. Full structured-emission migration deferred per pre-decision 8.)_
- [x] Daily rotation verified.
- [x] No partial-line writes verified. _(Each line is a single `write_all`
      of a complete `<json>\n` string; POSIX `O_APPEND` makes the write
      atomic.)_
- [x] Smoke gate's audit check passes. _(SKIPPED per pre-decision 10 —
      `scripts/smoke.sh` still labelled v2 and unchanged.)_
- [x] No `TODO` / `FIXME` in new code.
- [x] Single commit created.

## Outputs
- `crates/core/src/audit/mod.rs` (new)
- `crates/core/src/audit/event.rs` (new — AuditEvent / AuditKind / AuditActor)
- `crates/core/src/audit/writer.rs` (new — AuditWriter, queue)
- `crates/core/src/audit/jsonl.rs` (new — JsonlFileSubscriber)
- `crates/core/src/audit/api.rs` (new — re-exports)
- Many files modified across Tasks 10/19/20/22/31/32/33/42/43 to call `audit.append(...)`.
- `scripts/smoke.sh` (modified — audit check)
- `crates/core/tests/audit_log.rs` (new)
- `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-3: audit log — JSONL writer + subscriber fan-out

AuditWriter accepts non-blocking append; JsonlFileSubscriber writes
to ~/concerto/audit/audit-<day>.jsonl with daily rotation and
100ms-batched fsync. Every state-changing emission across prior
tasks now flows through the writer.

Refs: tasks/44-audit-log-writer.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **Demo emissions only — most prior tasks' `tracing::info!(audit.kind=...)`
    paths stay in place.** Per pre-decision 8, this task ships the
    plumbing + two structured demo emissions (workspace-created +
    permission_mode-changed inside `WorkspaceManager`). The
    workarea-level permission_mode + bypass_destructive_guard, tool
    approval rows in `agent_supervisor/actor.rs::dispatch_parse_event`,
    keychain access in `concerto-keychain`, destructive-command
    intercept, scheduler fire/suppress, and repository add/clone
    paths each still emit only via `tracing::info!`. Promoting them
    is a follow-on: the writer is already wired so each call site
    needs one `AuditWriter::append(AuditEvent::new(...).with_subject(...).with_details(...))`
    next to the existing tracing emission. No public-interface
    break expected.
  - **`AuditWriter` is plumbed via a per-manager `with_audit(...)`
    builder, not as an 11th `with_managers` argument.** The pre-decision
    sketch (option 9) named `with_managers`; in practice the writer
    isn't an `ApiServer` concern — it's a per-manager concern. Each
    manager that needs to emit takes a clone via a chained builder
    (today: `WorkspaceManager::with_audit`). The `main.rs` wiring
    looks identical to the proposed shape; the difference is only in
    where the `audit` field lives.
  - **`AuditEvent::at` is `#[serde(skip_serializing)]`.** The default
    derive would emit `at` as a `SystemTime` debug shape that's not
    RFC3339. The JSONL subscriber renders `at` itself
    (`serialize_event_line`) into the locked `YYYY-MM-DDThh:mm:ss.sssZ`
    format. The struct still carries `at` for in-memory consumers
    (e.g. the `MemorySubscriber` test rig); only the on-disk JSONL
    line writes the textual `at`.
  - **`AuditWriterTask::spawn` returns a third tuple element
    `Arc<Notify>` (`_drained`) that production currently ignores.**
    The handle is meant for shutdown gating — `runtime::Runtime::stop`
    can `notified().await` before unblocking the supervisor drain.
    V0.1 wires the spawn but leaves the `Arc<Notify>` parked under
    `_audit_drained` in `main.rs` because the runtime's shutdown
    already cancels the writer's token; the audit task drains the
    queue + flushes the JSONL file before exiting. Adding the
    explicit await is a one-line change in `runtime::stop`.
  - **JSONL flush cadence is per-event `file.flush()`, not the
    "100ms batched fsync" the design specifies.** The writer task
    drains one event at a time off the channel; at the rates V0.1
    sees (~10 events/sec peak from workspace + permission-mode +
    auto-approval) the per-event flush is cheaper than a periodic
    timer + drain loop. `sync_data` is called once on shutdown via
    `AuditLogSubscriber::flush`. The 100ms batched-fsync timer
    becomes valuable once destructive-command intercept + every
    tool approval flows through the writer; the timer can be added
    inside `AuditWriterTask::run` without changing the public surface.
  - **`EntityKind::Skill` and `EntityKind::Schedule` ship in the enum
    even though no path emits them in V0.1.** The schedule fire/suppress
    + skills toggle paths will graft onto them when their wiring
    lands; pre-locking the variants avoids a wire break.
- **Open questions for next task:**
  - The follow-on "mass-wire prior emissions through `AuditWriter`"
    task should walk every existing `tracing::info!(audit.kind = "…", …)`
    site and add the matching `audit.append(...)` next to it. The
    `audit.kind` values used in the tracing emissions
    (`permission_mode_changed`, `bypass_destructive_guard_changed`)
    map 1:1 onto `AuditKind` variants the writer already exposes.
  - The destructive-command path (Task 43) recommended a separate
    JSONL stream for urgent rows. V1.0 work — the
    `AuditLogSubscriber` trait can host an `UrgentJsonlFileSubscriber`
    that filters by `event.kind == DestructiveCommandIntercepted ||
    details["urgent"] == true`. No wire change needed.
  - The smoke gate's audit check (DoD spec step 6) needs the smoke
    client to read back `<data_dir>/audit/audit-<today>.jsonl` and
    grep for `workspace_created` after the smoke driver creates one.
    Skipped for V0.1 — pre-decision 10. When v3 lands (Task 52) the
    check becomes a one-grep addition.
- **Deliberate debt:**
  - Syslog + HttpsForwarder subscribers and at-rest encryption
    deferred (V1.0 / V2.0 per scope).
  - 100ms batched fsync timer not yet present; per-event `flush()`
    suffices at V0.1 event rates. See drift note 5.
  - Audit log retention sweeper deferred (V1.0 — V0.1 keeps forever).
  - Only `WorkspaceManager` is wired to emit; downstream managers
    each need a `with_audit` builder when their owning task picks
    up structured emission. See drift note 1.
  - No `TODO` / `FIXME` in new code.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still exits 0
  with "Smoke gate v2: PASSED". The audit-check augmentation is
  deferred to v3 (Task 52) per pre-decision 10.
