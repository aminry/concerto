# Changelog

All notable changes to Concerto are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
follows [SemVer](https://semver.org/) once it reaches 1.0; pre-1.0
releases may make breaking changes between alphas.

---

## 0.0.1 — V0.1 alpha

The first end-to-end Concerto release. macOS-only, single-repo
workspaces, Claude + Codex agents, co-located deployment. See
[`tasks/README.md`](tasks/README.md) for the task-based methodology
this release was built under, and
[`design/00_Architecture_Overview.md`](design/00_Architecture_Overview.md)
§10 for the phase split between V0.1 (this release) and V1.0.

### Phase 0 — Repo bootstrap

- **Cargo workspace skeleton** with the V0.1 crate set
  (`concerto-core`, `concerto-persist`, `concerto-keychain`,
  `concerto-proto`, `concerto-error`, `concerto-agent-host`, and
  supporting crates) — [`tasks/01-cargo-workspace-skeleton.md`](tasks/01-cargo-workspace-skeleton.md).
- **CI matrix + license enforcement** (`ci.yml`, `deny.yml`, `format.yml`)
  with `cargo deny` gating the MIT/Apache-2.0/BSD/ISC/0BSD allow-list —
  [`tasks/02-ci-and-license-enforcement.md`](tasks/02-ci-and-license-enforcement.md).
- **Smoke-gate scaffolding** (`scripts/smoke.sh`) that grows over
  Phases 1–4 — [`tasks/03-smoke-gate-scaffolding.md`](tasks/03-smoke-gate-scaffolding.md).
- **Interface-summary generator** (`scripts/regen-interfaces.sh`
  → `docs/interfaces/`) — [`tasks/04-interface-summary-generator.md`](tasks/04-interface-summary-generator.md).
- **Error and tracing baseline** (`concerto-error`, structured `tracing`
  layers) — [`tasks/05-error-and-logging-baseline.md`](tasks/05-error-and-logging-baseline.md).

### Phase 1 — Foundations

- **Proto schema + first messages** (`Workspace`, `Workarea`, `Session`,
  `ServerCapabilities`) — [`tasks/06-proto-schema-scaffolding.md`](tasks/06-proto-schema-scaffolding.md),
  [`tasks/07-first-proto-messages.md`](tasks/07-first-proto-messages.md).
- **SQLite migration runner + initial schema** (`projects`,
  `workspaces`, `workareas`, `sessions`) —
  [`tasks/08-sqlite-migration-runner.md`](tasks/08-sqlite-migration-runner.md),
  [`tasks/09-initial-db-schema.md`](tasks/09-initial-db-schema.md).
- **Keyring wrapper** for OS keychain secrets keyed by
  `(service, account)` — [`tasks/10-keychain-wrapper.md`](tasks/10-keychain-wrapper.md).
- **Runtime skeleton, supervision tree, gRPC over UDS** — graceful
  shutdown, panic-isolation, `Runtime.GetServerCapabilities` on
  `~/.concerto/core.sock` —
  [`tasks/11-runtime-skeleton.md`](tasks/11-runtime-skeleton.md),
  [`tasks/12-supervision-tree.md`](tasks/12-supervision-tree.md),
  [`tasks/13-grpc-uds-server.md`](tasks/13-grpc-uds-server.md).
- **Tauri 2 desktop shell** connecting to the Core over UDS —
  [`tasks/14-tauri-shell-skeleton.md`](tasks/14-tauri-shell-skeleton.md).
- **Smoke gate v1** (Core boots, UDS round-trip, clean shutdown) —
  [`tasks/15-smoke-gate-v1.md`](tasks/15-smoke-gate-v1.md).
- **Logging discipline** (rotating file at
  `~/concerto/logs/core-YYYY-MM-DD.log` with span fields) —
  [`tasks/16-logging-discipline.md`](tasks/16-logging-discipline.md).
- **Integration test harness** (`dev-deps` crate spawning a tempdir
  Core) — [`tasks/17-integration-test-harness.md`](tasks/17-integration-test-harness.md).

### Phase 2 — Vertical slice

- **Repository cloning** via `gix` (full clone, no sparse yet) —
  [`tasks/18-repository-cloning.md`](tasks/18-repository-cloning.md).
- **Workspace + workarea creation** with `git worktree add` —
  [`tasks/19-workspace-creation.md`](tasks/19-workspace-creation.md),
  [`tasks/20-workarea-creation.md`](tasks/20-workarea-creation.md).
- **`concerto-agent-host` PTY supervisor** detached via `setsid`,
  owning the PTY master, exposing a UDS for I/O —
  [`tasks/21-agent-host-binary.md`](tasks/21-agent-host-binary.md).
- **Agent spawn + session lifecycle** (`StartSession`,
  `StreamSessionIO`, `StopSession`) —
  [`tasks/22-agent-spawn-and-session.md`](tasks/22-agent-spawn-and-session.md),
  [`tasks/23-sessions-grpc-service.md`](tasks/23-sessions-grpc-service.md).
- **Desktop: workspace list, create-workspace flow, session terminal**
  (xterm.js + `react-xtermjs`) —
  [`tasks/24-desktop-workspace-list.md`](tasks/24-desktop-workspace-list.md),
  [`tasks/25-desktop-create-workspace-flow.md`](tasks/25-desktop-create-workspace-flow.md),
  [`tasks/26-desktop-session-terminal.md`](tasks/26-desktop-session-terminal.md).
- **Smoke gate v2** (end-to-end workspace creation, agent spawn,
  Core-restart reconnect) —
  [`tasks/27-smoke-gate-v2.md`](tasks/27-smoke-gate-v2.md).

### Phase 3 — Subsystem thickening

- **Repo Manager — `fsmonitor`, untracked cache, commit-graph,
  `manyFiles`, `git maintenance start`** —
  [`tasks/28-repo-fsmonitor-and-maintenance.md`](tasks/28-repo-fsmonitor-and-maintenance.md).
- **Repo Manager — `gix` status hot path** (target: <100 ms on a 2M-file
  repo) with committed benchmarks —
  [`tasks/29-gix-status-hot-path.md`](tasks/29-gix-status-hot-path.md).
- **Workspace Manager — `.context/` + files-to-copy on workarea
  creation, archive lifecycle, permission-mode inheritance** —
  [`tasks/30-context-and-files-to-copy.md`](tasks/30-context-and-files-to-copy.md),
  [`tasks/31-archive-lifecycle.md`](tasks/31-archive-lifecycle.md),
  [`tasks/32-permission-mode-inheritance.md`](tasks/32-permission-mode-inheritance.md).
- **Agent Supervisor — tool-approval intercept, checkpoint capture,
  MCP config surfacing, PTY hot reconnect, cold resume** from
  `~/.claude/projects/<id>/*.jsonl` —
  [`tasks/33-tool-approval-intercept.md`](tasks/33-tool-approval-intercept.md),
  [`tasks/34-checkpoints.md`](tasks/34-checkpoints.md),
  [`tasks/35-mcp-config-surfacing.md`](tasks/35-mcp-config-surfacing.md),
  [`tasks/36-pty-hot-reconnect.md`](tasks/36-pty-hot-reconnect.md),
  [`tasks/37-cold-resume.md`](tasks/37-cold-resume.md).
- **Scheduler — `/loop` session-scoped repeating prompt** —
  [`tasks/38-scheduler-loop.md`](tasks/38-scheduler-loop.md).
- **Skills Registry** (personal / project / plugin scopes, per-project
  enable/disable, slash-command surface) —
  [`tasks/39-skills-registry.md`](tasks/39-skills-registry.md).
- **Suggestion Engine — rule-engine chips** (no learning yet) —
  [`tasks/40-suggestion-rule-engine.md`](tasks/40-suggestion-rule-engine.md).
- **Security — filesystem allow/deny, four-level permission modes,
  destructive-command intercept, JSON-lines audit log at
  `~/concerto/audit/`** —
  [`tasks/41-filesystem-allow-deny.md`](tasks/41-filesystem-allow-deny.md),
  [`tasks/42-permission-modes-runtime.md`](tasks/42-permission-modes-runtime.md),
  [`tasks/43-destructive-command-intercept.md`](tasks/43-destructive-command-intercept.md),
  [`tasks/44-audit-log-writer.md`](tasks/44-audit-log-writer.md).
- **VCS Integration — `gh` CLI shell-out** (PR list / view / create) —
  [`tasks/45-vcs-gh-cli.md`](tasks/45-vcs-gh-cli.md).
- **Desktop — three-panel layout, Monaco diff viewer, tray icon** —
  [`tasks/46-desktop-three-panel-layout.md`](tasks/46-desktop-three-panel-layout.md),
  [`tasks/47-desktop-monaco-diff.md`](tasks/47-desktop-monaco-diff.md),
  [`tasks/48-desktop-tray-icon.md`](tasks/48-desktop-tray-icon.md).

### Phase 4 — Ship-readiness

- **macOS LaunchAgent install + uninstall** (`scripts/install-macos.sh`,
  `scripts/uninstall-macos.sh`) — [`tasks/49-launchd-install.md`](tasks/49-launchd-install.md).
- **Performance-budget verification** gating the V0.1-relevant rows of
  `design/00 §7.7` (Core idle <100 MB, Core at 8 agents <600 MB,
  Desktop cold start <2 s, `gix` status <100 ms) —
  [`tasks/50-perf-budget-verification.md`](tasks/50-perf-budget-verification.md).
- **README + getting-started + CHANGELOG** (this task) —
  [`tasks/51-readme-and-getting-started.md`](tasks/51-readme-and-getting-started.md).
- **Smoke gate v3** (full V0.1 happy-path scenario, run in CI) —
  [`tasks/52-smoke-gate-v3.md`](tasks/52-smoke-gate-v3.md).
- **Tauri auto-update + macOS codesign + notarization** —
  [`tasks/53-auto-update-and-signing.md`](tasks/53-auto-update-and-signing.md).

### Known limitations (V0.1)

- **macOS only.** Linux and Windows desktop ports are V1.0.
- **Single-repo workspaces.** Multi-repo / multi-PR sessions are V1.0.
- **No relay, no mobile, no web.** Co-located deployment only.
- **No Maestro chat agent** (sub-system 08) and **no notifications /
  push** (sub-system 14).
- **Claude + Codex only.** Gemini CLI and the Claude Agent SDK land in
  V1.0.
- **`gh` CLI shell-out** for VCS. No GitHub webhooks, no PR-set
  coordination yet.
- **Full clone only.** Sparse, blobless, and partial-clone support is
  V1.0 — see [`design/02_Repository_Manager.md`](design/02_Repository_Manager.md).
