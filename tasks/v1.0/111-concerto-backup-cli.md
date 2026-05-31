# Task 111 — `concerto backup` CLI

| Field | Value |
|---|---|
| Phase | 1 |
| Task type | rust |
| Verification tier | 1 |
| Size | small (≤4h) |
| Depends on | 108, 109, 110 |
| Touches subsystem(s) | 09 (Persistence), 10 (CLI) |
| Smoke gate | extends:backup |

## Goal
Implement the `concerto backup` command `design/09 §6.4` specifies: a consistent SQLite snapshot via `VACUUM INTO`, an optional worktree tarball, and an audit-range export — so a user can capture and move their Concerto state. Builds on the CLI client module (Task 109) and complements the integrity guard (Task 110).

## Inputs to read before starting
- `design/09_Persistence.md` §6.4 (`concerto backup`: `VACUUM INTO`, optional worktree tar, audit-range export), §4 (DB path policy, worktree directory policy, audit JSONL location).
- `crates/cli/src/client.rs` + command scaffolding (Task 109's reusable module).
- `crates/persist/src/lib.rs` (DB path resolution) and the audit-log path convention (`~/concerto/audit/`).
- `tasks/v1.0/110-persistence-hardening.md` → "Handoff Notes".
- `tasks/v1.0/108-smoke-gate-refactor.md` → "Handoff Notes" — the `scripts/smoke.d/` layout the `backup` check plugs into.

## Scope — in
- `concerto backup [--out <dir>] [--with-worktrees] [--audit-from <ts>] [--audit-to <ts>]`:
  - DB snapshot via `VACUUM INTO <out>/concerto.db` (a hot-consistent copy — do NOT just `cp` the live WAL DB).
  - With `--with-worktrees`: tar the worktree directory tree into `<out>/worktrees.tar`.
  - With an audit range: copy/filter the JSONL audit records in `[from,to]` into `<out>/audit.jsonl`.
  - Writes a small `<out>/manifest.json` (timestamps, versions, what was included).
- The command can run **against the local DB path directly** (it doesn't need a running Core for the file-level `VACUUM INTO`), but should refuse/ warn sensibly if a Core is actively writing — document the chosen behavior.
- Integration test: seed a temp DB + audit file, run `backup`, assert the snapshot opens and `quick_check`s ok and the manifest is correct.

## Scope — out
- Restore (`concerto restore`) — note as deliberate debt / future task; backup-only here.
- Encryption of the backup (V2.0).
- Remote/cloud backup targets.

## Public interface this task locks
- The `concerto backup` flag surface and the `<out>/` layout (`concerto.db`, `worktrees.tar`, `audit.jsonl`, `manifest.json`). FROZEN; restore (future) reads this layout.

## Implementation notes
- `VACUUM INTO` requires opening the source DB read-only-ish; reuse `crates/persist` to resolve the canonical DB path rather than reconstructing it.
- Stream the tar; don't read whole worktrees into memory.
- Keep timestamps in the manifest UTC ISO-8601 for portability.

## Verification
Tier 1.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-cli backup` → snapshot opens + quick_checks ok; manifest correct; audit range filtered.
4. `cargo test --workspace --no-fail-fast` → all pass.
5. `scripts/smoke.sh` → add a `backup` check (`extends:`): boot Core, `concerto backup --out <tmp>`, assert `<tmp>/concerto.db` exists and opens. Exits 0.

## Definition of Done
- [ ] `VACUUM INTO` snapshot + optional worktree tar + audit-range export + manifest
- [ ] Backup runs against the local DB path; concurrent-write behavior documented
- [ ] Integration test verifies snapshot integrity + manifest
- [ ] `backup` smoke check passes
- [ ] Verification commands pass; smoke gate green
- [ ] Single commit created with the message below

## Outputs
- `crates/cli/src/commands/backup.rs` (new)
- `crates/cli/src/main.rs` (modified — register subcommand)
- `crates/cli/Cargo.toml` (modified — tar dep)
- `crates/cli/tests/backup.rs` (new)
- `scripts/smoke.d/<NN>-backup.sh` + manifest line (new)

## Commit message
```
phase-1: concerto backup (VACUUM INTO + worktrees + audit export)

Adds `concerto backup` producing a hot-consistent SQLite snapshot, an
optional worktree tarball, an audit-range export, and a manifest, per
design/09 §6.4. Restore is deferred to a follow-on task.

Refs: tasks/v1.0/111-concerto-backup-cli.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
- **Open questions for next task:**
- **Deliberate debt:**
- **Smoke-gate state:**
