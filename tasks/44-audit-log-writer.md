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
- [ ] Verification commands pass.
- [ ] Every prior task's emission flows through AuditWriter.
- [ ] Daily rotation verified.
- [ ] No partial-line writes verified.
- [ ] Smoke gate's audit check passes.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Single commit created.

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
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** Syslog + HttpsForwarder subscribers and at-rest encryption deferred (V1.0 / V2.0).
- **Smoke-gate state:** v2 augmented with audit checks. Still labeled v2 — formal v3 arrives Task 52.
