# Concerto Smoke Gate

The smoke gate (`scripts/smoke.sh`) is the layer-2 verification backstop
described in `tasks/README.md §5`. It boots `concerto-core` end-to-end,
drives it through the canonical happy path via `tools/smoke-client`, and
asserts the on-disk + wire-level outputs are well-formed. Every task in
the build keeps it green; CI runs it on every push and PR.

## Gate versions

The gate evolves with the build. A task that adds a new feature surface
generally bumps the gate version and adds a check.

| Version | Task | Phase | Adds |
| --- | --- | --- | --- |
| **v1** | 15 | 1 | Core boots, UDS socket appears, `Runtime.GetServerCapabilities` round-trips, Core shuts down cleanly. |
| **v2** | 27 | 2 | Project + bare repo + clone + workspace + workarea created via gRPC; `.context/` + repo `.git` present on disk; echo session spawned via `Sessions.CreateSession`; output streams via `Streams.Subscribe(session.io.<sid>)`; session stopped; clean shutdown. |
| **v3** | 52 | 3+4 | Workarea permission mode flipped to `auto` via `Workareas.UpdateWorkareaPermissionMode`; today's JSONL audit log contains `workspace_created`; `/loop` schedule created + listed via `Schedules.{Create,List}Schedule`; fake personal `SKILL.md` discovered via `Skills.{RefreshMarketplaces,ListSkills}`; fake personal `mcp.json` surfaced via `Sessions.ListMcpServers`. Runs on Linux **and** macOS in CI. |

A green v3 means every V0.1 feature surface that can be exercised
non-interactively has been exercised in one CI run.

## What v3 still defers (intentionally)

These are *not* gaps in V0.1 — they're surfaces the smoke gate can't
exercise without either user interaction or test-only RPCs we declined
to ship in release builds.

| Surface | Why not in v3 | Where it IS verified |
| --- | --- | --- |
| Destructive-command intercept | Requires faking a `ParseEvent::AwaitingApproval` from the agent supervisor; the cleanest way is a `Sessions.InjectTestEvent` RPC gated by a `--test-mode` build feature, which we declined for V0.1 to keep the wire surface lean. | `crates/core/tests/destructive_intercept.rs` (integration test path). |
| Tool-approval resolve (`Sessions.ResolveApproval`) | Same root cause — needs a fake `AwaitingApproval` event source. | `crates/core/tests/tool_approval.rs`. |
| Yolo mode entry ceremony | The "I understand" string is a human-facing ceremony; faking it in the smoke gate would normalise it. | `crates/core/tests/permission_runtime.rs`. |
| Managed.json cap enforcement + hot reload | Inappropriate to mutate a user-facing policy file as a smoke side-effect. | `crates/core/tests/permission_runtime.rs`. |
| `/loop` fire-and-spawn | Would need a ≥35s wait; v3 budgets < 3 min total. The smoke gate asserts row insert + list round-trip only. | `crates/core/tests/scheduler_loop.rs`. |
| Desktop / Tauri | Headless Tauri is V1.0. V0.1 keeps Desktop verification manual. | Manual `make dev` smoke. |
| Multi-machine / split-host | V1.0 (transport / pairing aren't in V0.1). | — |
| Real `gh` CLI integration | V1.0 — V0.1 only ships the wiring; tests use a mocked `gh`. | `crates/core/tests/vcs_*.rs`. |
| Force-failure rehearsals | Operator-driven — temporarily break a handler, confirm the smoke fails cleanly, revert. Not part of the automated run. | Documented in each smoke-task's "Handoff Notes". |

## `--ci-mode` flag

V0.1: no-op. Every check in v3 is CI-safe today. The flag is wired so
that future network-touching checks (real `gh` round-trips, push
notifications, marketplace fetches) can opt out cleanly. The workflow
at `.github/workflows/smoke.yml` passes `--ci-mode` so the contract is
already in place.

## Running locally

```sh
./scripts/smoke.sh           # interactive run
./scripts/smoke.sh --ci-mode # mirror CI behaviour locally
```

The script provisions everything it needs under a `mktemp` directory
and cleans up on exit. It does **not** touch your real `~/.concerto/`,
`~/concerto/`, or `~/.claude/` — the Core child process runs with
`HOME` and `CONCERTO_*_DIR` overridden.

Wall-clock target is < 3 minutes on a CI runner; warm-cache local runs
finish in well under a minute.
