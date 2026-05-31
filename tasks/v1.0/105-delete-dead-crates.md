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
- [ ] Both crates deleted and removed from workspace members
- [ ] No dangling references anywhere in the repo
- [ ] `docs/interfaces/rust-api.md` regenerated and committed
- [ ] Verification commands pass
- [ ] Single commit created with the message below

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
- **Drift from plan:**
- **Open questions for next task:**
- **Deliberate debt:**
- **Smoke-gate state:**
