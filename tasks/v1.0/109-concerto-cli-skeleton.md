# Task 109 — `concerto` CLI Skeleton

| Field | Value |
|---|---|
| Phase | 1 |
| Task type | rust |
| Verification tier | 1 |
| Size | medium (1–3d) |
| Depends on | 108 |
| Touches subsystem(s) | 10 (Client API Protocol) |
| Smoke gate | extends:cli |

## Goal
Turn the `crates/cli` placeholder (currently a 7-line `main.rs` that prints "concerto placeholder") into a real CLI that wraps the gRPC API over UDS. `design/10` mandates the `concerto` CLI as a V1.0 deliverable (R-6) and later tasks build on it (`concerto pair` in Task 713, `concerto backup` in Task 111). This task delivers the skeleton: a client that dials the Core's UDS socket and implements a first set of read commands plus the command scaffolding everything else hangs off.

## Inputs to read before starting
- `crates/cli/src/main.rs` (the placeholder).
- `tools/smoke-client/` (the existing dev gRPC client — the reference for how to dial Core's UDS and call services; reuse its connection approach).
- `crates/proto/` + `docs/interfaces/proto.md` (the services/messages: `Runtime.GetServerCapabilities`/`GetStatus`, `Workspaces.List`, `Sessions` list).
- `apps/desktop/src-tauri/src/core_client.rs` (how the desktop resolves the socket path + the `set_socket_override` convention — mirror the default socket path logic).
- `design/10_Local_API_Protocol.md` §1 (CLI mandate, R-6).
- `tasks/v1.0/108-smoke-gate-refactor.md` → "Handoff Notes" — the `scripts/smoke.d/` layout + `scripts/smoke.manifest` format this task's `cli` check plugs into.

## Scope — in
- Replace `crates/cli` with a `clap`-based binary `concerto` exposing:
  - `concerto status` — calls `Runtime.GetServerCapabilities` + `GetStatus`, prints version/uptime/transport_kind/actors.
  - `concerto workspace ls` — calls `Workspaces.List`, prints a table.
  - `concerto session ls [--workarea <id>]` — lists sessions.
  - `concerto --socket <path>` global flag + `CONCERTO_SOCKET` env to point at a non-default Core (mirroring the desktop's socket override).
  - `--json` global flag for machine-readable output.
- A small internal `client` module that establishes the UDS gRPC channel (factor it so Tasks 111/713 reuse it).
- Helpful errors when Core isn't running (name the socket path tried).
- Integration test: spawn a Core via the `crates/test-harness`, run `concerto status` against it, assert output.

## Scope — out
- `concerto pair` (Task 713 — needs the pairing flow from Phase 2).
- `concerto backup` (Task 111 — separate task, builds on this client module).
- Any write/mutating commands beyond what's listed (create workspace, start session) — keep the skeleton read-only; mutations come with their subsystems.
- Iroh/remote transport (CLI is local-UDS only in Phase 1; remote is a later concern).

## Public interface this task locks
- Rust: `crates/cli` binary name `concerto`; the `client` module's connect function signature; the global flags `--socket`/`--json` and `CONCERTO_SOCKET`.
- The command surface above (subcommands may be added later; these names are stable).

## Implementation notes
- Copy the channel-setup approach from `tools/smoke-client`, but keep the CLI's `client` module **self-contained** — do not refactor `tools/smoke-client` (that's outside this task's `Outputs`). Factor the module cleanly so Tasks 111/713 reuse it within `crates/cli`.
- Keep output rendering separated from the RPC calls so `--json` is a thin switch.
- Default socket path must match what Core writes and what the desktop reads — derive it the same way (don't hardcode a second source of truth).

## Verification
Tier 1.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo build -p concerto-cli` → produces `concerto`.
4. `cargo test -p concerto-cli` → the spawn-core-and-`status` integration test passes.
5. `cargo test --workspace --no-fail-fast` → all pass.
6. `scripts/smoke.sh` → add a `cli` capability check (`extends:cli`): with Core running, `concerto status` exits 0 and prints the version. Exits 0.
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → commit any regen.

## Definition of Done
- [ ] `concerto status` / `workspace ls` / `session ls` work against a live Core over UDS
- [ ] `--socket`/`CONCERTO_SOCKET`/`--json` honored; Core-down error names the socket
- [ ] Reusable `client` module factored for Tasks 111/713
- [ ] Integration test + `cli` smoke check pass
- [ ] Verification commands pass; smoke gate green
- [ ] Single commit created with the message below

## Outputs
- `crates/cli/src/main.rs` (rewritten)
- `crates/cli/src/client.rs`, `src/commands/*.rs` (new)
- `crates/cli/Cargo.toml` (modified — clap, tonic client deps)
- `crates/cli/tests/status.rs` (new)
- `scripts/smoke.d/<NN>-cli.sh` + manifest line (new)
- `docs/interfaces/rust-api.md` (regenerated if pub surface changed)

## Commit message
```
phase-1: concerto CLI skeleton over UDS gRPC

Replaces the crates/cli placeholder with a clap-based `concerto` binary:
status / workspace ls / session ls, --socket/--json flags, and a
reusable UDS client module for later `pair` and `backup` subcommands.

Refs: tasks/v1.0/109-concerto-cli-skeleton.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
- **Open questions for next task:**
- **Deliberate debt:**
- **Smoke-gate state:**
