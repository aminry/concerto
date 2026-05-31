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
- [x] `VACUUM INTO` snapshot + optional worktree tar + audit-range export + manifest
- [x] Backup runs against the local DB path; concurrent-write behavior documented
- [x] Integration test verifies snapshot integrity + manifest
- [x] `backup` smoke check passes
- [x] Verification commands pass; smoke gate green
- [x] Single commit created with the message below

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
  - **tar crate:** `tar = "0.4"` (rust-lang org, pure-Rust, MIT/Apache-2.0,
    `cargo deny` clean) added as a workspace dep + a `crates/cli` dep, with
    `default-features = false`. `Builder::append_dir_all("workspaces", …)`
    streams each file (no whole-worktree buffering); the tar runs on a
    `spawn_blocking` thread. Fully cross-platform — no `#[cfg(unix)]` anywhere
    in the backup path (verified: `rg 'cfg\(unix\)|UnixStream|std::os::unix'
    crates/cli/src/commands/backup.rs` → no matches). `backup` is dispatched in
    `main.rs` BEFORE socket resolution, so it never pulls in the Unix-only
    `client::connect`.
  - **Cross-platform test (no Core / no test-harness):** `tests/backup.rs`
    seeds a migrated DB by calling `concerto-persist` (`Persistence::open` on a
    temp path), plants an audit JSONL, and drives the shipped `concerto backup`
    via `assert_cmd` with `CONCERTO_DATA_DIR` set on the child process (no
    process-global env mutation → no libtest race). It re-opens the snapshot
    read-only and asserts `PRAGMA quick_check == "ok"`, checks the manifest, and
    verifies the audit range filtered to exactly the in-range record. NOT under
    `#![cfg(unix)]`; `concerto-persist` is a normal (cross-platform) dep,
    `tempfile`/`sqlx` are added under the normal `[dev-dependencies]` (NOT the
    `cfg(unix)` block). It does NOT use `concerto-test-harness`.
  - **Concurrent-write behavior chosen:** backup does NOT refuse/warn when a
    Core is live. It opens the source DB **read-only** (`?mode=ro`, so it can
    never mutate the live DB) and `VACUUM INTO` takes a SQLite read lock that,
    under WAL, lets writers continue while producing a single consistent
    point-in-time snapshot. Documented in the module header.
  - **DB-path resolution:** mirrors the Core (`crates/core/src/runtime.rs`):
    `$CONCERTO_DB_PATH` → `<data_dir>/concerto.db`, where `data_dir` =
    `$CONCERTO_DATA_DIR` → `$CONCERTO_HOME/concerto` → the `<home>/concerto`
    default sourced from `concerto_persist::PersistenceConfig::default_for_user`
    (no second hardcoded home-relative path). `$CONCERTO_HOME` is honored as the
    smoke-gate scratch-home convention. Worktrees = `<data_dir>/workspaces/`,
    audit = `<data_dir>/audit/`.
  - **Manifest timestamps:** generated with an inline `civil_from_unix` (same
    Howard-Hinnant algorithm as `crates/core/src/audit/jsonl.rs`) to emit UTC
    ISO-8601 `YYYY-MM-DDTHH:MM:SS.mmmZ` — no `chrono`/`time` direct dep added.
    Audit range filtering is a lexicographic string compare on the fixed-width
    `at` field (sorts chronologically), so `--audit-from`/`--audit-to` accept
    any ISO-8601 prefix (e.g. `2026-05-30`).
- **Open questions for next task:** restore (future task) reads the FROZEN
  `<out>/` layout: `concerto.db`, `worktrees.tar` (top-level `workspaces/`
  entry inside the archive), `audit.jsonl`, `manifest.json` (`manifest_version:
  1`; `included.{db_snapshot,worktrees_tar,audit_jsonl,audit_from,audit_to,
  audit_records}`). Restore requires Core stopped (design/09 §6.4) and is the
  reverse of these four artifacts.
- **Deliberate debt:** `concerto restore` is deferred to a follow-on task
  (backup-only here, per Scope — out). Backup encryption (V2.0) and
  remote/cloud targets are out of scope. No `--audit-from`/`--audit-to`
  timestamp *validation* beyond the lexicographic compare (intentional: any
  ISO-8601 prefix works; a malformed bound simply matches lexically).
- **Smoke-gate state:** `extends:backup` — added `scripts/smoke.d/96-backup.sh`
  (`check_backup`) and appended `backup` to `scripts/smoke.manifest` after
  `cli`. Runs `concerto backup --out <tmp>` against the Core's scratch DB
  (resolved via the `CONCERTO_DATA_DIR` that `00-core-boot` exports), asserts
  `<tmp>/concerto.db` + `manifest.json` exist, and `PRAGMA quick_check`s the
  snapshot via `sqlite3` (falls back to a re-open-via-`concerto backup` smoke if
  `sqlite3` isn't on PATH). `scripts/smoke.sh --ci-mode` exits 0 with
  `PASS backup`; `--list` shows it; `shellcheck` clean.
