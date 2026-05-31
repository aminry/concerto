# Task 113 — CI Matrix: Windows + Linux Core Build/Test

| Field | Value |
|---|---|
| Phase | 1 |
| Task type | infra-ops |
| Verification tier | 2 |
| Size | medium (1–3d) |
| Depends on | 105 |
| Touches subsystem(s) | 18 (Distribution & Operations), 01 (Core Daemon Runtime) |
| Smoke gate | unchanged |

## Goal
Stand up the cross-platform CI lanes V1.0 needs so Windows and Linux regressions in the Core are caught from the start, not at ship time. V0.1 CI was macOS-centric; `design/18 §3.3` requires a build matrix across Mac/Windows/Linux. This task adds Windows and Linux Core build+test lanes (Desktop stays Mac+Windows, no Linux desktop per the design), gating off the parts not yet portable (the Unix-only agent-host PTY) so the lanes are honestly green rather than green-by-skipping-everything.

## Inputs to read before starting
- `.github/workflows/ci.yml` (the current matrix), `format.yml`, `deny.yml`, `smoke.yml`, `bench.yml`, `perf.yml`.
- `design/18_Distribution_and_Operations.md` §3.3 (GHA matrix: Mac universal2, Windows x64+arm64, Linux x64+arm64), §2 (V1.0 ports), §10 (self-host parity).
- `design/00_Architecture_Overview.md` §10 (Core on macOS+Windows+Linux; Desktop Mac+Windows only).
- `crates/agent-host/` (Unix-only today — `setsid`/PTY; this is the main thing to feature-gate or `#[cfg]` out of the Windows lane until Task 702).
- `tasks/v1.0/105-delete-dead-crates.md` → "Handoff Notes" (clean member list).

## Scope — in
- Extend `.github/workflows/ci.yml` to run `cargo check` + `cargo test` for the **Core and portable crates** on `ubuntu-latest` and `windows-latest` in addition to macOS.
- Make the Windows lane compile: `#[cfg(unix)]`-gate or feature-gate the agent-host PTY/`setsid` code paths so the workspace builds on Windows with the agent-host's Unix internals excluded (the binary may be a stub on Windows for now — Task 702 implements ConPTY). Do NOT fake the tests; exclude the genuinely-Unix-only test modules with `#[cfg(unix)]` and leave a clear marker that Windows agent-host is Task 702.
- Keep clippy `-D warnings` enforced on all three OS lanes.
- Linux lane: full `cargo test --workspace` (Linux Core is fully supported; the agent-host PTY works on Linux).
- Leave Desktop/web/mobile lanes as-is (those arrive with their tasks); this task is Core/Rust cross-platform only.
- Document in the workflow comments what each lane covers and what's gated off pending later tasks.

## Scope — out
- Windows agent-host ConPTY (Task 702) — only gate it off cleanly here.
- Release/signing matrix (Task 706).
- arm64 runners if GHA-hosted arm64 isn't readily available — x64 lanes are the floor; note arm64 as a Task 706 concern if you can't add it cheaply.
- Linux Desktop (explicitly not built — design decision).

## Public interface this task locks
- The CI lane contract: Core + portable crates build and test green on macOS, Linux, and Windows (with the documented Unix-only gates). Future tasks must keep all three lanes green.

## Implementation notes
- The realistic blocker is the agent-host's `pre_exec(setsid)` and PTY types. Gate the module with `#[cfg(unix)]` and provide a `#[cfg(windows)]` stub that returns a clear "agent-host not yet supported on Windows (Task 702)" error so dependent crates still compile and link.
- Prefer matrix `strategy.matrix.os` over duplicated jobs.
- If `cargo deny` / interface regen are macOS-only steps today, keep them on one lane (don't triple their cost) — cross-platform is about *compile + test*, not re-running every gate 3×.

## Verification
Tier 2 (CI is the runner; "real Windows/Linux hardware" beyond GHA is the Tier-3 phase-checklist item).
1. `cargo check --workspace` and `cargo test --workspace --no-fail-fast` pass locally on macOS (unchanged).
2. The updated `ci.yml` is valid: `actionlint .github/workflows/ci.yml` (or the repo's workflow linter) → clean.
3. On a pushed branch, the **Linux and Windows lanes go green** (this is the actual proof — the orchestrator confirms via `gh pr checks`).
4. The Windows lane compiles with agent-host Unix internals gated; the gated test modules are `#[cfg(unix)]`, not deleted.
5. `scripts/smoke.sh` (macOS) → still exits 0.

## Definition of Done
- [x] CI runs Core + portable crates' check/test on macOS, Linux, Windows
- [x] Windows lane compiles via `#[cfg(unix)]`/stub gating of agent-host PTY (Task 702 marker left)
- [x] clippy `-D warnings` on all three lanes; no test faked or silently skipped beyond documented Unix-only gates
- [ ] Linux + Windows lanes green on a pushed branch (pending CI — orchestrator pushes; macOS proven locally, Windows reasoned via cfg-gate audit below)
- [x] Workflow lints clean
- [x] Single commit created with the message below

## Outputs
- `.github/workflows/ci.yml` (modified — OS matrix)
- `crates/agent-host/src/*.rs` (modified — `#[cfg(unix)]` gating + Windows stub)
- `crates/core/src/agent_supervisor/*.rs` (modified if needed for the Windows stub to link)
- `docs/interfaces/rust-api.md` (regenerated if cfg-gating changed the summarized surface)

## Commit message
```
phase-1: cross-platform CI for Core (linux + windows lanes)

Adds ubuntu/windows lanes to ci.yml for the Core and portable crates,
cfg-gating the agent-host PTY/setsid internals so Windows compiles with
a clear "ConPTY pending (Task 702)" stub. Linux Core is fully tested.

Refs: tasks/v1.0/113-ci-matrix-windows-linux-core.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** Minimal. The agent-host Rust gating that the plan anticipated was already in place from Task 21: the entire PTY/UDS supervisor lives in `crates/agent-host/src/main.rs`'s `#[cfg(unix)] mod unix`, `main()` has a `#[cfg(not(unix))]` stub (exit 2), `libc` is already `[target.'cfg(unix)'.dependencies]`, and `tests/echo_round_trip.rs` is already `#![cfg(unix)]`. The lib surface (`api.rs`, `bridge.rs`, `ring.rs`, `exit.rs`) was already fully cross-platform. So no *new* cfg-gates were required to compile on Windows — I only refreshed the marker comments (main.rs module doc + `#[cfg(not(unix))]` branch, lib.rs doc, Cargo.toml comment, echo_round_trip.rs doc) to say "ConPTY pending (Task 702)" instead of the vaguer "V1.0", and clarified the lib is the portable surface. The real change is in `ci.yml`: `concerto-agent-host` was REMOVED from the Windows `--exclude` list on all three commands (check/clippy/test), so Windows now builds + lints + tests it (its tests cfg-gate to empty there). `concerto-desktop`, `concerto-smoke-client`, and `concerto-test-harness` STAY excluded on Windows (out of scope: desktop-on-Windows is a later task with no shipped Windows icons; smoke-client/test-harness are UDS dev/test tooling). `crates/core/src/agent_supervisor/*` needed NO changes — `mod.rs` is already `#![cfg(unix)]` and Task 106's resolver already handles the `.exe` suffix. `docs/interfaces/rust-api.md` unchanged (only `api.rs` is summarized; it was untouched).
- **Open questions for next task:** arm64 runners were NOT added — GHA-hosted arm64 Linux/Windows runners aren't a cheap default, so x64 is the cross-platform floor here and arm64 is deferred to Task 706 (release/signing matrix). If V1.0 wants arm64 *test* coverage (not just release builds) before 706, that needs its own decision.
- **Deliberate debt:** Windows `concerto-agent-host` is a ConPTY *stub* — it compiles and links (so the Core's portable crates build on the Windows lane) but the binary just prints "ConPTY support pending (Task 702)" and exits 2. The Unix-only `echo_round_trip` integration test does NOT run on Windows (it cfg-gates to empty); Task 702 owns the Windows agent-host round-trip coverage. Real Windows *hardware* validation beyond GHA-hosted runners remains a Tier-3 phase-checklist item.
- **Smoke-gate state:** Unchanged. `scripts/smoke.sh --ci-mode` still exits 0 (verified on macOS); this task touched only CI matrix config + agent-host cfg-marker comments, not the smoke path.
