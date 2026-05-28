# Concerto V0.1 — AI-Agent Task Breakdown

*Meta-document for the task files in this directory. Read this first.*

| Field | Value |
|---|---|
| Status | Approved (2026-05-27) |
| Scope | **V0.1 only** (alpha, macOS, minimal happy path). V1.0 task breakdown is a separate exercise after V0.1 ships. |
| Owner | Amin Roudaki |
| Related docs | `../design/00_Architecture_Overview.md`, `../design/01..18_*.md`, `../design/Concerto_PRD.md` |

---

## 1. Purpose

Concerto is a large system: a Rust daemon (the Core), three client platforms, a relay binary, and 18 named sub-systems. Building it with AI coding agents requires decomposing the work into self-contained tasks that (a) can each be executed by a fresh agent with no prior context, (b) have machine-verifiable completion criteria, and (c) integrate cleanly with the tasks before and after them.

This document captures the decisions about *how* the V0.1 build is decomposed. The individual task files (`NN-<slug>.md`) capture *what* each task does.

V1.0 will be planned as its own follow-on breakdown after V0.1 ships. We are deliberately not pre-planning V1.0 task files until we have evidence from V0.1 about whether the task shape is right.

---

## 2. Scope of V0.1

V0.1 is the alpha slice defined in `design/00_Architecture_Overview.md` §10. In scope:

- **Sub-systems 01, 02, 03, 04, 05, 06, 07, 09, 10, 12, 13, 15, 18** at their V0.1 fidelity per the design doc's phasing table.
- **macOS only** for the Desktop client. Linux/Windows ports are V1.0.
- **Co-located deployment only.** No split-host, no relay, no mobile, no web.
- **`gh` CLI shell-out** for GitHub. No webhooks, no PR sets.
- **Claude + Codex agents only.** No Gemini, no Claude Agent SDK.
- **No Maestro chat agent** (08), **no notifications/push** (14), **no mobile/web clients** (16, 17).

What "done" means for V0.1: a developer can install the Core + Desktop on a Mac, create a workspace from a local git repo, spawn a Claude or Codex session in that workspace, see the agent's terminal output streamed in real time, approve tool calls, and survive a Core restart without losing the agent session.

---

## 3. Decisions locked

Six decisions made during brainstorming on 2026-05-27. Each task file inherits these as fixed; revising any of them is a new planning conversation, not a task-level decision.

| # | Decision | Choice |
|---|---|---|
| D1 | Build scope | V0.1 first; V1.0 planned after V0.1 ships. |
| D2 | Task granularity | Mixed: foundations small (≤4h), feature work medium (1–3d). |
| D3 | Verification level | Compile + clippy + unit + integration tests + **smoke gate after every task**, plus golden-file snapshots for the schema layer (proto, SQLite DDL). |
| D4 | Interface contracts | Schema files (`*.proto`, `migrations/*.sql`, `pub` Rust traits) are canonical. A generated `docs/interfaces/<subsystem>.md` summary is what each next task reads first. |
| D5 | Sequencing | Foundations in topological order → vertical-slice spine → per-subsystem thickening → ship-readiness. |
| D6 | Task-file format | Strict template (see §6) including Definition-of-Done checklist and Handoff Notes the agent fills in when done. |

---

## 4. Phase structure

V0.1 is broken into **four phases, ~52 tasks total**. Tasks are numbered globally (`01` … `52`); the phase boundary is reflected in the `Phase` field of each task file, not in the file name.

### Phase 0 — Repo bootstrap (~5 tasks, small)

Set up the things every later task depends on. If any of these is wrong, the cost of fixing it grows linearly with the number of tasks already shipped.

1. Cargo workspace + crate skeleton (`core`, `relay`, `cli`, `proto`, `transport`, `gix-wrap`, `keychain`, `pty-sup`, `desktop-shell`, `persist`, `agent-host`).
2. CI matrix (macOS/Windows/Linux for Core; macOS-only for Desktop in V0.1) + `cargo deny` for license enforcement (MIT/Apache-2.0/BSD/ISC/0BSD allow-list per `design/00 §6.11`).
3. Smoke-gate scaffolding (`scripts/smoke.sh` that fails initially; later tasks make it green incrementally).
4. Interface-summary generator (`scripts/regen-interfaces.sh` that walks `.proto`, `migrations/*.sql`, and pub trait modules → writes `docs/interfaces/<subsystem>.md`).
5. Base `tracing` + error-type crate (the `Result<T, E>` and `thiserror` enums every other crate uses).

### Phase 1 — Foundations (~12 tasks, small)

The minimum skeleton that boots, persists state, and accepts a connection. Each task here locks an interface that downstream tasks treat as immutable.

6. Proto schema crate scaffolding (build.rs, tonic-build, no messages yet).
7. First proto messages (`Workspace`, `Workarea`, `Session`, `ServerCapabilities`) + `ServerService.GetCapabilities`.
8. SQLite migration runner (`crates/persist`) with forward-only ordered migrations.
9. Initial DB schema migration: `projects`, `workspaces`, `workareas`, `sessions` tables.
10. `keyring-rs` wrapper crate (`crates/keychain`) — get/set/delete secrets keyed by `(service, account)`.
11. Runtime skeleton (`crates/core`): single-instance guard via lockfile, supervision tree shell, graceful shutdown on SIGTERM.
12. Panic-isolation harness (`tokio::task` with `catch_unwind` + restart policy).
13. gRPC server skeleton over UDS (one no-op method: `GetCapabilities`).
14. Tauri shell skeleton (`crates/desktop-shell`) — window opens, connects to UDS, shows ServerCapabilities response.
15. Smoke gate v1: Core boots, Desktop connects via UDS, `GetCapabilities` round-trips, both shut down cleanly.
16. Logging discipline: rotating file at `~/concerto/logs/core-YYYY-MM-DD.log`; span fields include workspace ID, session ID, device cert ID (placeholder in V0.1).
17. Integration test harness: shared `dev-deps` crate that spawns a Core in a tempdir, returns a connected client.

### Phase 2 — Vertical slice (~10 tasks, medium)

A user can create a workspace, an agent spawns, output streams to Desktop, agent survives Core restart. End-to-end before any subsystem is "finished."

18. Repository cloning (`crates/gix-wrap`): full clone only, no sparse, no blobless. Just `clone` + `fetch`.
19. Workspace creation API (`CreateWorkspace` gRPC method) + persistence row.
20. Workarea creation API + worktree via `git worktree add`.
21. `concerto-agent-host` helper binary: PTY supervisor that detaches via `setsid`, owns the PTY master, exposes a UDS for I/O.
22. Spawn agent CLI from agent-host: V0.1 starts by spawning `echo hello`, then `claude` once that works.
23. Session lifecycle (`StartSession`, `StreamSessionIO`, `StopSession`) — server-streaming RPC for stdout/stderr.
24. Desktop: workspace list screen (Zustand store + shadcn/ui list).
25. Desktop: create-workspace flow (modal → calls `CreateWorkspace` → routes to new workspace).
26. Desktop: session terminal (xterm.js + `react-xtermjs`) — renders stream from `StreamSessionIO`.
27. Smoke gate v2: user creates a workspace from a local git repo, spawns a `claude` session, sees output stream to Desktop, kills Core, restarts Core, reconnects to same session, output continues.

### Phase 3 — Subsystem thickening (~20 tasks, medium)

Each thickened to its V0.1 fidelity per the design doc. Tasks here can fan out a little because some subsystems are independent of others — but we keep the order linear for simplicity.

28. **02 Repo Manager** — `fsmonitor` + untracked cache + commit-graph + `manyFiles` auto-applied per project; `git maintenance start` registered.
29. **02 Repo Manager** — `gix status` hot path (target: <100 ms on 2M-file repo); benchmarks committed.
30. **03 Workspace Manager** — `.context/` directory creation + files-to-copy mechanism on workarea creation.
31. **03 Workspace Manager** — archive lifecycle (workspace `Active` → `Archived` → `Deleted` state machine + audit).
32. **03 Workspace Manager** — permission-mode inheritance (workspace default → workarea → session override).
33. **04 Agent Sup** — tool-approval boundary detection (PTY output pattern intercept).
34. **04 Agent Sup** — checkpoint capture (snapshot of agent state per turn).
35. **04 Agent Sup** — MCP config surfacing (read `~/.claude/mcp.json`, `~/.codex/config.toml`, `.mcp.json`; expose via gRPC).
36. **04 Agent Sup** — PTY reconnect after Core restart (Core finds existing agent-host UDS sockets, replays ring buffer).
37. **04 Agent Sup** — cold resume from `~/.claude/projects/<id>/*.jsonl` (the floor when ring buffer is gone).
38. **05 Scheduler** — `/loop` primitive (session-scoped repeating prompt).
39. **06 Skills Registry** — discovery across personal / project / plugin scopes; per-project enable/disable toggle; slash-command surface.
40. **07 Suggestion Engine** — rule engine over agent events; chip generation; no learning yet (per V0.1 fidelity).
41. **12 Security** — filesystem allow-list (worktree + `.context/` + declared paths) and hard deny-list (`~/.ssh`, `~/.aws`, etc.).
42. **12 Security** — four-level permission modes (`strict` / `normal` / `auto` / `yolo`); `managed.json` cap enforcement.
43. **12 Security** — destructive-command intercept (pattern set, red-styled approval prompt, independent of permission mode).
44. **12 Security** — audit log writer (JSON Lines at `~/concerto/audit/`).
45. **13 VCS Integration** — `gh` CLI shell-out wrapper (PR list, PR view, PR create); no GitHub API yet.
46. **15 Desktop** — three-panel layout (sidebar / center / right rail) per `design/15_Desktop_Client.md`.
47. **15 Desktop** — Monaco diff viewer with custom decoration layer.
48. **15 Desktop** — tray icon (menu-bar host) + open/quit actions.

### Phase 4 — Ship-readiness (~5 tasks, small/medium)

49. launchd plist + install/uninstall scripts (`concerto-core` runs as user agent).
50. Performance-budget verification: gates checking the V0.1-relevant rows of `design/00 §7.7` (Core idle <100MB, Core at 8 agents <600MB, Desktop cold start <2s, `gix status` <100ms).
51. README + getting-started doc (developer install, "create your first workspace" walkthrough).
52. Smoke gate v3: full V0.1 happy-path scenario covering everything above, run in CI.
53. Tauri auto-update channel wiring + release-signing setup (Mac codesign + notarization).

(53 tasks — the count drifted slightly from "~52" during writing. Phase 0–4 boundaries are firm.)

---

## 5. Verification model

Every task ends with the same machine-checkable bar. Three layers:

**Per-task automated checks (mandatory):**

```bash
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/
scripts/smoke.sh
```

If any of these fails, the task is not done — full stop.

**Smoke gate (the integration backstop):**

`scripts/smoke.sh` is a single script that grows over the build. After Phase 1 it boots the Core and verifies `GetCapabilities`. After Phase 2 it adds end-to-end workspace creation and agent spawn. After Phase 3 it adds permission modes, audit log presence, and `/loop`. The `Smoke gate` field in each task file declares whether the task changes what the smoke gate covers (`unchanged`, `v1`, `v2`, `v3`, or `new`).

**Schema snapshots (the silent-drift backstop):**

For the layers where breakage is hardest to detect — gRPC proto, SQLite DDL, public Rust trait modules — we keep checked-in snapshot files. CI fails if a task changes these without updating the snapshot. This is the cheapest way to catch "task 23 broke task 9's wire format and we didn't notice until task 31."

**Why this minimizes human verification:** every task's `Definition of Done` is a checklist of pass-or-fail commands. A reviewer (or another agent) can rerun the verification commands and see green/red without reading the diff. The schema snapshots and smoke gate catch the integration-drift class of bug that unit tests miss. Human review is reserved for the cases where the smoke gate is green but something else looks wrong — taste, not correctness.

---

## 6. Task-file template

Every task is a single markdown file at `tasks/NN-<slug>.md` (zero-padded two-digit number, kebab-slug title). The file IS the prompt: an operator hands the file to a fresh agent with no additional context, and the agent should be able to complete the work.

```markdown
# Task NN — <Title>

| Field | Value |
|---|---|
| Phase | 0 / 1 / 2 / 3 / 4 |
| Size | small (≤4h) / medium (1–3d) |
| Depends on | NN, NN, … (prior task numbers) |
| Touches subsystem(s) | 01, 09, … |
| Smoke gate | unchanged / v1 / v2 / v3 / new |

## Goal
One paragraph. What this task makes true that wasn't true before.

## Inputs to read before starting
- design/<doc>.md §<section> — <why>
- docs/interfaces/<file>.md — <why>
- tasks/<NN-1>-<slug>.md → "Handoff Notes" — drift from prior task

## Scope — in
- bullet list of what this task IS doing

## Scope — out
- bullet list of what this task is NOT doing

## Public interface this task locks
- proto: `proto/<file>.proto` — messages X, Y, Z; service S with methods …
- SQL: `crates/persist/migrations/NNNN_<name>.sql` — tables …
- Rust: `crates/<crate>/src/<mod>.rs` — `pub trait Foo { … }`, `pub struct Bar { … }`

## Implementation notes
Short, opinionated guidance on the non-obvious parts. Linked design-doc sections do the heavy lifting; don't restate them.

## Verification
Exact commands the agent runs with expected outcomes:
1. `cargo check --workspace` → no warnings
2. `cargo clippy --workspace --all-targets -- -D warnings` → clean
3. `cargo test -p <crate> <test-name>` → all pass
4. `cargo deny check` → clean
5. `scripts/smoke.sh` → exits 0, log contains "<expected line>"
6. `scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → no unintended drift

## Definition of Done
- [ ] All Verification commands pass on a clean checkout
- [ ] No `TODO` / `FIXME` / `unimplemented!()` / `todo!()` in new code (any deliberate ones noted in Handoff Notes)
- [ ] No files outside the intended Outputs list modified
- [ ] `docs/interfaces/` regenerated and committed if any schema changed
- [ ] Smoke gate is green
- [ ] Single commit created with the message specified below

## Outputs
- `crates/<crate>/src/<file>.rs` (new)
- `proto/<file>.proto` (new)
- `docs/interfaces/<file>.md` (regenerated)

## Commit message
```
<phase>: <one-line summary>

<2–4 line body explaining what changed and why>

Refs: tasks/NN-<slug>.md
```

## Handoff Notes (filled in when finishing this task)
- **Drift from plan:** anything implemented differently than the task file said, and why
- **Open questions for next task:** anything the next task's author should know
- **Deliberate debt:** TODOs left in, with rationale and the task number that will close them
- **Smoke-gate state:** what's now covered; what's still stubbed
```

---

## 7. Operator workflow (how to execute a task)

For each task in numeric order:

1. **Pre-flight:** check that prior task's commit is on the branch and CI is green.
2. **Brief the agent:** hand it the task file's full contents as the prompt. No other context needed.
3. **Agent works:** reads `Inputs`, implements `Scope — in`, runs `Verification` until green.
4. **Definition of Done:** agent ticks every checkbox.
5. **Handoff Notes:** agent fills in the Handoff Notes section in the same file, commits it together with the code.
6. **Operator spot-check:** read the diff summary and Handoff Notes only. If anything in Handoff Notes flags drift, decide whether to revise the next task file before starting it.

If a task fails verification and the agent can't recover, the operator either (a) makes the task file more precise and reruns, or (b) splits it into two tasks. **Do not edit prior tasks' code outside of an explicit revision task.**

---

## 8. Revising the plan

The task list will need to change. Two rules:

- **Adding a task** between N and N+1: insert as `tasks/N.5-<slug>.md` (renumbering breaks references in commits and handoff notes). At V1.0 planning time we re-number.
- **Revising a shipped task's locked interface**: write a new task (`NN-<slug>.md`) titled "Revise <prior task's interface>" with the same template. Never edit the old task file's `Public interface this task locks` section.

---

## 9. What this document does not do

- It does not pre-plan V1.0. V1.0 task breakdown is a follow-on exercise after V0.1 ships.
- It does not specify subsystem internals. That's in `design/01..18_*.md`.
- It does not replace the design docs. Each task file points its agent at the specific design-doc sections it needs.
- It does not invent verification commands the build doesn't have yet. Phase 0 tasks set up the verification infrastructure (smoke gate, interface generator, schema snapshots) so Phase 1+ tasks can rely on it.

---

*End of meta-document. The individual task files (`01-…` through `53-…`) start in this same directory once Phase 0 task generation is complete.*
