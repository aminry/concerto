# Task 105 — Delete Dead Crates (`pty-sup`, `desktop-shell`)

| Field | Value |
|---|---|
| Phase | 1 |
| Task type | rust |
| Verification tier | 1 |
| Size | small (≤4h) |
| Depends on | — |
| Touches subsystem(s) | (workspace hygiene) |
| Smoke gate | unchanged |

## Goal
Remove the two placeholder crates that V0.1 scaffolded but never used, so the V1.0 workspace has no dead members masquerading as real subsystems. `crates/pty-sup` was superseded by `crates/agent-host` (Task 21); `crates/desktop-shell` was never used because the real Tauri shell lives in `apps/desktop/src-tauri`. Both are 3-line `lib.rs` stubs.

## Inputs to read before starting
- `Cargo.toml` (workspace `members` list).
- `crates/pty-sup/src/lib.rs`, `crates/desktop-shell/src/lib.rs` (confirm they are 3-line stubs with no real dependents).
- `docs/interfaces/rust-api.md` (regenerated after removal).

## Scope — in
- Delete `crates/pty-sup/` and `crates/desktop-shell/` directories.
- Remove both from `Cargo.toml` workspace `members`.
- Grep the whole repo for any `concerto-pty-sup` / `concerto-desktop-shell` (or path) references and remove them (there should be none in real code; if there ARE non-trivial dependents, STOP and ask — that contradicts the premise).
- Regenerate `docs/interfaces/rust-api.md`.

## Scope — out
- `crates/cli`, `crates/relay`, `crates/transport` — these are also placeholders but are **real V1.0 subsystems** built later (Tasks 109, 214, 212). Do NOT delete them.

## Public interface this task locks
- Nothing added. Removes `concerto-pty-sup` and `concerto-desktop-shell` from the workspace permanently.

## Implementation notes
- This is pure deletion + workspace edit. The risk is a hidden reference; the grep is the safety check. If `git grep -n "pty-sup\|pty_sup\|desktop-shell\|desktop_shell"` returns only the deleted files and this task file, you're clear.

## Verification
Tier 1.
1. `cargo check --workspace` → succeeds (14→ fewer members, all resolve).
2. `cargo clippy --workspace --all-targets -- -D warnings` → clean.
3. `cargo test --workspace --no-fail-fast` → all pass.
4. `git grep -n "pty.sup\|desktop.shell"` → no hits outside this task file / its commit.
5. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commits the regen.
6. `scripts/smoke.sh` → still exits 0 (smoke gate unchanged).

## Definition of Done
- [x] Both crates deleted and removed from workspace members
- [x] No dangling references anywhere in the repo
- [x] `docs/interfaces/rust-api.md` regenerated and committed
- [x] Verification commands pass
- [x] Single commit created with the message below

## Outputs
- `crates/pty-sup/` (deleted)
- `crates/desktop-shell/` (deleted)
- `Cargo.toml` (modified)
- `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-1: delete dead pty-sup and desktop-shell crates

Both were V0.1 placeholders never wired up — pty-sup superseded by
crates/agent-host, desktop-shell superseded by apps/desktop/src-tauri.
Removing them from the workspace before V1.0 build-out.

Refs: tasks/v1.0/105-delete-dead-crates.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** None to the build. Prose mentions of `pty-sup`/`desktop-shell` were intentionally left untouched in `design/00`, `design/15`, `design/Concerto_TechStack_Evaluation.md`, `CHANGELOG.md:52`, `docs/superpowers/plans/*`, the frozen V0.1 task files (`tasks/01`, `tasks/14`, `tasks/README.md`), and unrelated same-string comments (`apps/desktop/src/App.tsx` UI comment, `crates/agent-host/src/main.rs:694` agent-host's own "pty supervisor" log line, `crates/core/src/security/managed.rs:94` prose). These are not code dependents — editing `design/` is forbidden for a non-doc task and the rest is history/prose. A future doc task (e.g. 107/712) can sweep them. The `docs/interfaces/rust-api.md` regen produced NO diff because both stubs had no public API to summarize; `git diff --exit-code docs/interfaces/` is clean.
- **Open questions for next task:** None. Task 113 (CI matrix) depends on 105 only for the smaller member set — unaffected.
- **Deliberate debt:** None. Pure deletion; no TODO/FIXME/todo!() introduced.
- **Smoke-gate state:** unchanged — `scripts/smoke.sh` still exits 0 ("V0.1 alpha — all checks PASSED"), no capability added or removed.
