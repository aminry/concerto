# Task 106 — Robust agent-host Binary Resolution

| Field | Value |
|---|---|
| Phase | 1 |
| Task type | rust |
| Verification tier | 1 |
| Size | small (≤4h) |
| Depends on | — |
| Touches subsystem(s) | 04 (Agent Supervisor), 01 (Core Daemon Runtime) |
| Smoke gate | unchanged (re-run as a regression check; the existing session-spawn check already exercises agent-host, and 108's composable smoke.d doesn't exist yet) |

## Goal
Make Core's discovery of the `concerto-agent-host` binary robust instead of dev-fragile. Today `crates/core/src/agent_supervisor/spawn.rs::default_host_binary` resolves the helper only as `current_exe().parent()/concerto-agent-host`. In embedded-Core dev (`tauri dev` compiles only `concerto-desktop`), that path is empty, so every session-create failed with `spawn agent-host: io: No such file or directory` until `scripts/dev-embedded.sh` was taught to pre-build the binary into `target/debug/`. That's a build-step workaround, not a resolution strategy. This task adds an explicit override + a documented search order so embedded and packaged builds resolve the helper deterministically.

## Inputs to read before starting
- `crates/core/src/agent_supervisor/spawn.rs` (the `default_host_binary` fn and `spawn_host`).
- `apps/desktop/src-tauri/src/embedded.rs` (how embedded Core boots; it shares the desktop binary's `current_exe()`).
- `scripts/dev-embedded.sh` (the current pre-build workaround this task makes unnecessary as a *correctness* dependency).
- `design/04_Agent_Supervisor.md` §3.9 (host bridge), `design/01 §6.3` (agent host orphan adoption).

## Scope — in
- Replace `default_host_binary` with a resolution function that tries, in order, and returns the first that exists:
  1. `$CONCERTO_AGENT_HOST_BIN` (explicit absolute path override) — if set and non-empty.
  2. `current_exe().parent()/concerto-agent-host[.exe]` (the packaged/co-located case — unchanged behavior).
  3. `current_exe().parent()` walked up to a `target/<profile>/` sibling `concerto-agent-host` (the cargo-dev / embedded case where the desktop binary and the helper share a target dir).
- A clear, actionable error if none resolve, naming the env override and the paths it tried (the previous error — a bare "io: No such file or directory" surfaced as "Rpc" — was the whole motivation).
- Unit tests for the resolution order (override wins; co-located found; helpful error lists tried paths). Use `tempfile` + a fake executable, following the test style in `spawn.rs`.
- Update `scripts/dev-embedded.sh` to set `CONCERTO_AGENT_HOST_BIN` to the freshly built path (belt-and-suspenders) rather than relying on co-location by accident; keep the `cargo build -p concerto-agent-host` step but make resolution correct without it.

## Scope — out
- Windows ConPTY agent-host (Task 702). The `.exe` suffix handling here is just so the search doesn't break the future Windows build; the host itself stays Unix-only for now.
- Changing the host bridge protocol or spawn argv (locked by Task 21/22).

## Public interface this task locks
- Rust: `crates/core/src/agent_supervisor/spawn.rs` — the resolution fn signature (keep `pub fn ... -> Result<PathBuf>`; rename to `resolve_host_binary` if clearer, and re-export at the prior path so callers don't break). State the final name in Handoff.
- Env contract: `CONCERTO_AGENT_HOST_BIN` (absolute path) as the highest-precedence override.

## Implementation notes
- Keep the function pure/testable: take the override value and a base dir as params for the unit tests; the public wrapper reads `std::env::current_exe()` + `std::env::var`.
- The "walk up to target/<profile>" search should be bounded (a couple of levels) and only kick in when co-location fails — don't scan the filesystem.

## Verification
Tier 1.
1. `cargo check --workspace` clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo test -p concerto-core agent_host_resolution` (or the test module name) → override/co-located/error-path cases pass.
4. `cargo test --workspace --no-fail-fast` → all pass (existing `agent_spawn` / `hot_reconnect` tests still green).
5. `scripts/smoke.sh` → exits 0; agent-host session still spawns.
6. Manual (operator, Phase-1 checklist): `make dev-embedded` and create a session — it spawns the agent-host with no co-location pre-build accident.
7. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → no unintended drift (or commit regen if the pub signature changed).

## Definition of Done
- [ ] `CONCERTO_AGENT_HOST_BIN` override honored first; co-located path second; target-sibling search third
- [ ] Resolution failure produces an actionable error listing tried paths + the env override
- [ ] Unit tests cover all three resolution branches + the error
- [ ] `dev-embedded.sh` sets the override explicitly
- [ ] Verification commands pass; smoke gate green
- [ ] Single commit created with the message below

## Outputs
- `crates/core/src/agent_supervisor/spawn.rs` (modified)
- `scripts/dev-embedded.sh` (modified)
- `crates/core/src/agent_supervisor/` test module or `crates/core/tests/agent_host_resolution.rs` (new)
- `docs/interfaces/rust-api.md` (regenerated if signature changed)

## Commit message
```
phase-1: robust agent-host binary resolution

Adds CONCERTO_AGENT_HOST_BIN override plus a documented search order
(override → co-located → target-sibling) so embedded-Core dev and
packaged builds resolve concerto-agent-host deterministically. Failure
now names the tried paths instead of surfacing a bare "Rpc".

Refs: tasks/v1.0/106-agent-host-binary-resolution.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
- **Open questions for next task:**
- **Deliberate debt:**
- **Smoke-gate state:**
