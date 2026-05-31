# Task 110 — Persistence Hardening (Integrity Check + Downgrade Refusal)

| Field | Value |
|---|---|
| Phase | 1 |
| Task type | rust |
| Verification tier | 1 |
| Size | small (≤4h) |
| Depends on | 108 |
| Touches subsystem(s) | 09 (Persistence) |
| Smoke gate | extends:persistence-integrity |

## Goal
Add the V1.0 startup safety checks `design/09` calls for so a corrupt or future-version database fails loudly at boot instead of producing silent misbehavior: an on-startup `PRAGMA quick_check`, and a binary-vs-schema-version guard that refuses to start when the DB schema is newer than the binary understands (a downgrade). The forward-only migration runner already aborts on a failed migration; this task adds the two missing guards around it.

## Inputs to read before starting
- `design/09_Persistence.md` §2 (V1.0 fidelity: on-startup integrity check), §6.2/§6.3 (migration + integrity), §8 (failed migration aborts at prior version; binary-downgrade refuses to start).
- `crates/persist/src/migration_runner.rs` (the existing forward-only runner) and `crates/persist/src/lib.rs` (the connection/pool setup + where boot opens the DB).
- `crates/core/src/boot.rs` (Core's boot sequence — Persistence starts first per `design/01 §6.1`; the integrity check belongs here on the open path).
- `tasks/v1.0/108-smoke-gate-refactor.md` → "Handoff Notes" — the `scripts/smoke.d/` layout the `persistence-integrity` check plugs into.

## Scope — in
- On opening the DB at boot: run `PRAGMA quick_check`; if it does not return `ok`, fail boot with a clear `Error` naming the DB path and that it appears corrupt (suggest `concerto backup`/restore once Task 111 lands — reference it, don't depend on it).
- Record the binary's max-known schema version (the highest migration number the binary ships). After connecting, read the DB's current `schema_version` (or migration table); if `db_schema_version > binary_max`, refuse to start with a downgrade error naming both versions. If `db_schema_version < binary_max`, the existing forward-only runner migrates up as today.
- Make both checks part of the persistence open path so embedded Core and the daemon both get them.
- Unit/integration tests: a corrupt-DB fixture fails `quick_check`; a DB stamped with a higher schema version triggers the downgrade refusal; a normal DB boots and migrates as before.

## Scope — out
- `concerto backup`/restore CLI (Task 111).
- At-rest encryption (V2.0) and audit-log changes (Task 112).
- Multi-device key store schema (that's Phase 2, Task 209's `devices` wiring).

## Public interface this task locks
- Rust: the persistence open/boot function's error contract — distinct `Error` variants for `DatabaseCorrupt` and `SchemaDowngrade` (add to `crates/error` if that's where wire codes live; match the existing `Error` enum style). Variant names FROZEN.

## Implementation notes
- `quick_check` is cheaper than `integrity_check`; the design specifies `quick_check` for the startup path — use it.
- Derive `binary_max` from the migrations embedded at compile time (don't hardcode a literal that drifts from the migration files).
- Keep the checks ordered: open → `quick_check` → version compare → migrate-up. A corrupt DB should fail before the migrator touches it.

## Verification
Tier 1.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-persist` → corrupt-DB + downgrade-refusal + normal-boot tests pass.
4. `cargo test --workspace --no-fail-fast` → all pass (existing migration tests still green).
5. `scripts/smoke.sh` → add a `persistence-integrity` check (`extends:`): Core boots on a fresh DB (quick_check ok). Exits 0.
6. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit regen if the error enum changed the rust-api summary.

## Definition of Done
- [x] Startup `PRAGMA quick_check`; corrupt DB fails boot with a clear, path-naming error
- [x] Binary-downgrade refusal when `db_schema_version > binary_max`; both versions named
- [x] `binary_max` derived from embedded migrations, not hardcoded
- [x] Tests cover corrupt / downgrade / normal paths
- [x] Verification commands pass; smoke gate green
- [x] Single commit created with the message below

## Outputs
- `crates/persist/src/lib.rs` and/or `src/migration_runner.rs` (modified)
- `crates/core/src/boot.rs` (modified if the check is invoked there)
- `crates/error/src/lib.rs` (modified — new error variants)
- `crates/persist/tests/integrity.rs` (new)
- `docs/interfaces/rust-api.md` (regenerated if needed)

## Commit message
```
phase-1: persistence startup integrity + downgrade refusal

Adds an on-boot PRAGMA quick_check and a binary-vs-schema-version guard
that refuses to start on a DB newer than the binary understands. Both
guards run on the shared persistence open path (daemon + embedded).

Refs: tasks/v1.0/110-persistence-hardening.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** Input paths in this task file are stale; adapted to the real layout. The persistence open path is `crates/persist/src/api.rs::Persistence::open` (there is NO `crates/persist/src/migration_runner.rs` — only a `crates/persist/tests/migration_runner.rs` integration test). Both the daemon (`crates/core/src/runtime.rs`) and embedded Core reach `Persistence::open`, so the two guards landed there and need no `boot.rs` change. The error enum lives in `crates/error/src/api.rs` (not `lib.rs`); the new variants are **`DatabaseCorrupt`** and **`SchemaDowngrade`** (FROZEN names), with kebab wire codes `database.corrupt` / `schema.downgrade` added in `crates/error/src/error.rs::wire_code` and mapped in `crates/core/src/error_map.rs` (`database.corrupt` → `Code::Internal`, `schema.downgrade` → `Code::FailedPrecondition`). `binary_max` is derived at runtime from the embedded migrator: `sqlx::migrate!("./migrations").iter().map(|m| m.version).max()` — never hardcoded (currently 8 / migration `0008`). Applied DB version = `MAX(version)` from `_sqlx_migrations`, probed defensively (the table is absent on a fresh DB → treated as "no applied version"). Guard order is open → on-open `quick_check` → downgrade compare → forward-only `sqlx::migrate!`; the pre-existing post-migration `quick_check` (design/09 §6.3) is retained and now also returns `DatabaseCorrupt`. Smoke check `scripts/smoke.d/05-persistence-integrity.sh` greps `CORE_LOG` for a new single success line `"persistence integrity ok (quick_check passed, schema not downgraded)"` emitted by `Persistence::open` on the happy path; inserted as `persistence-integrity` in `scripts/smoke.manifest` right after `core-boot`.
- **Open questions for next task:** Task 111 (`concerto backup`) references the corrupt-DB recovery path — the corrupt-DB error now surfaces as `Error::DatabaseCorrupt` from `Persistence::open` (boot abort), wire code `database.corrupt`, and its message already suggests `concerto backup`/restore. 111 should make that suggestion real (the restore is the reverse of `VACUUM INTO`, Core stopped). The on-open `quick_check` is the natural pre-restore integrity probe to reuse.
- **Deliberate debt:** The downgrade guard reads the applied version from `_sqlx_migrations` (sqlx-internal); if sqlx ever renames that table the probe silently returns `None` (degrades to "no guard"), not a panic — acceptable for a forward-compat guard but worth a note. The design's `concerto db migrate-down --to N` / `concerto db recover` tools are V1.5+ and only referenced in error text, not implemented (per Scope — out).
- **Smoke-gate state:** `extends:persistence-integrity` — new check live and green. `scripts/smoke.sh --list` shows it after `core-boot`; full `scripts/smoke.sh --ci-mode` exits 0 with `PASS persistence-integrity`. `shellcheck` clean on the new file. Driver `scripts/smoke.sh` untouched.
