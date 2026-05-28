# Task 01 — Cargo Workspace and Crate Skeleton

| Field | Value |
|---|---|
| Phase | 0 |
| Size | small (≤4h) |
| Depends on | — |
| Touches subsystem(s) | 01 (Runtime), 18 (Distribution) |
| Smoke gate | new |

## Goal
Create the Cargo workspace and an empty crate skeleton that every subsequent task fills in. After this task, `cargo check --workspace` builds an empty (but well-structured) workspace with all the crate boundaries declared in `design/00 §6.1`. Nothing functional yet — this is the scaffolding the rest of V0.1 grows on.

## Inputs to read before starting
- `design/00_Architecture_Overview.md` §6.1 (language and runtime — locks Cargo workspace, crate list) and §6.11 (licensing — MIT only; permitted deps).
- `tasks/README.md` (the meta-document — read for context on how all tasks fit together).
- `LICENSE`, `NOTICE`, `TRADEMARKS.md` at the repo root — these already exist; do not modify.

## Scope — in
- Create `Cargo.toml` at repo root declaring a workspace.
- Create the following member crates with minimal `Cargo.toml` and a `src/lib.rs` (or `src/main.rs` for binaries) containing only a doc comment placeholder:
  - `crates/core` (library + binary `concerto-core`)
  - `crates/relay` (library + binary `concerto-relay`) — present but empty; used in V1.0.
  - `crates/cli` (binary `concerto`) — present but empty.
  - `crates/proto` (library) — protobuf-generated types live here in Task 06.
  - `crates/transport` (library) — Iroh + UDS abstractions; mostly stubs in V0.1.
  - `crates/gix-wrap` (library) — git operations.
  - `crates/keychain` (library) — OS keychain wrapper.
  - `crates/pty-sup` (library) — PTY supervision primitives.
  - `crates/desktop-shell` (library) — Tauri shell shared types.
  - `crates/persist` (library) — SQLite + migration runner.
  - `crates/agent-host` (binary `concerto-agent-host`) — detached PTY helper.
  - `crates/error` (library) — shared error/result types (Task 05 fills this in).
- Set `rust-version = "1.78"` (or latest stable as of project start) in the workspace `[workspace.package]` table.
- Set workspace-level `[workspace.dependencies]` with placeholder entries for: `tokio`, `tracing`, `tracing-subscriber`, `thiserror`, `serde`, `serde_json`, `sqlx`, `tonic`, `prost`, `keyring`. Do not import them in any crate yet — that's later tasks.
- Add `.gitignore` entries for `/target`, `*.db`, `~/concerto/`, `.DS_Store`.
- Add `rust-toolchain.toml` pinning stable Rust.

## Scope — out
- No actual implementation in any crate (next tasks do that).
- No Tauri scaffolding (Task 14 does that — the Tauri shell is generated separately).
- No CI yet (Task 02).
- No `cargo deny` config yet (Task 02).

## Public interface this task locks
- File layout: `Cargo.toml` at repo root, `crates/<name>/Cargo.toml`, `crates/<name>/src/lib.rs` (or `main.rs`).
- Crate names: as listed in "Scope — in". Renaming any of these after this task ships requires a revision task.
- Workspace package metadata: `version = "0.0.1"`, `edition = "2021"`, `license = "MIT"`, `authors = ["Concerto contributors"]`.

## Implementation notes
- Use `default-members` in the workspace `Cargo.toml` to include `core` and `cli` only — relay and agent-host are built explicitly when needed.
- Use the `resolver = "2"` workspace setting.
- Each member crate's `Cargo.toml` should set `version.workspace = true`, `edition.workspace = true`, `license.workspace = true` so version bumps happen in one place.
- Binary crates (`core`, `relay`, `cli`, `agent-host`) need a `[[bin]]` entry naming the executable; library crates use the default.
- Place a `// Placeholder — Task NN will implement this.` comment in each `lib.rs` / `main.rs`. Replace `NN` with the task number that owns that crate's first real content (e.g., `crates/persist/src/lib.rs` → "Task 08").

## Verification
1. `cargo check --workspace` → succeeds with no warnings.
2. `cargo build --workspace` → all crates compile (they're empty stubs).
3. `cargo metadata --no-deps --format-version 1 | jq '.workspace_members | length'` → 12 (one per declared crate).
4. `cargo run --bin concerto-core` → runs, prints nothing meaningful, exits 0. (Replace with `eprintln!("concerto-core placeholder")` if needed for the binary to do anything.)
5. `git status` shows the expected new files and nothing else.

## Definition of Done
- [ ] All Verification commands pass on a clean checkout.
- [ ] No `TODO` / `FIXME` / `unimplemented!()` / `todo!()` in new code (placeholder comments are fine).
- [ ] No files outside the intended Outputs list modified.
- [ ] No schema artifacts changed (no `docs/interfaces/` regen needed yet — that script doesn't exist).
- [ ] Smoke gate not yet established (Task 03 creates it).
- [ ] Single commit created with the message specified below.

## Outputs
- `Cargo.toml` (new, workspace root)
- `rust-toolchain.toml` (new)
- `.gitignore` (new)
- `crates/core/Cargo.toml`, `crates/core/src/lib.rs`, `crates/core/src/main.rs` (new)
- `crates/relay/Cargo.toml`, `crates/relay/src/lib.rs`, `crates/relay/src/main.rs` (new)
- `crates/cli/Cargo.toml`, `crates/cli/src/main.rs` (new)
- `crates/proto/Cargo.toml`, `crates/proto/src/lib.rs` (new)
- `crates/transport/Cargo.toml`, `crates/transport/src/lib.rs` (new)
- `crates/gix-wrap/Cargo.toml`, `crates/gix-wrap/src/lib.rs` (new)
- `crates/keychain/Cargo.toml`, `crates/keychain/src/lib.rs` (new)
- `crates/pty-sup/Cargo.toml`, `crates/pty-sup/src/lib.rs` (new)
- `crates/desktop-shell/Cargo.toml`, `crates/desktop-shell/src/lib.rs` (new)
- `crates/persist/Cargo.toml`, `crates/persist/src/lib.rs` (new)
- `crates/agent-host/Cargo.toml`, `crates/agent-host/src/main.rs` (new)
- `crates/error/Cargo.toml`, `crates/error/src/lib.rs` (new)

## Commit message
```
phase-0: cargo workspace and crate skeleton

Declares the Cargo workspace with 12 member crates per design/00 §6.1.
All crates are empty stubs; subsequent tasks fill them in. Workspace
package metadata (version, license, edition) is centralized.

Refs: tasks/01-cargo-workspace-skeleton.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** —
- **Smoke-gate state:** —
