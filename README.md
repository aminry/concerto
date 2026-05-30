# Concerto

[![CI](https://github.com/aminry/concerto/actions/workflows/ci.yml/badge.svg)](https://github.com/aminry/concerto/actions/workflows/ci.yml)
[![Smoke](https://github.com/aminry/concerto/actions/workflows/smoke.yml/badge.svg)](https://github.com/aminry/concerto/actions/workflows/smoke.yml)
[![cargo-deny](https://github.com/aminry/concerto/actions/workflows/deny.yml/badge.svg)](https://github.com/aminry/concerto/actions/workflows/deny.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Version](https://img.shields.io/badge/version-0.0.1-orange.svg)](CHANGELOG.md)

> A local-first orchestration platform for a concerted ensemble of AI coding agents.

Concerto runs Claude Code, Codex, and other AI coding agents in isolated git
worktrees on your machine, and exposes the same control surface through a
native desktop app. The Core daemon is the durable home of your work: agents
keep running when you close the window, state survives across restarts, and
nothing leaves your machine without explicit consent.

> **Status — V0.1 alpha.** This release is **macOS-only**, supports
> **single-repo workspaces**, and ships with the **Claude** and **Codex** CLIs
> as the only first-class agents. Expect rough edges, missing UI affordances,
> and breaking changes between alphas. No relay, no mobile clients, no web
> client — those land in V1.0.

---

## What's in V0.1

- **Workspaces over git worktrees.** Each workspace is a real worktree under
  `~/concerto/`. Sessions are isolated; deleting a workspace cleans up
  cleanly.
- **Claude + Codex agents.** Spawned as long-lived PTY subprocesses
  supervised by `concerto-agent-host`. Output streams to the Desktop in
  real time and survives Core restarts via PTY reconnect + cold resume.
- **Permission modes + audit log.** Four-level permission modes
  (`strict` / `normal` / `auto` / `yolo`), filesystem allow/deny lists, a
  destructive-command intercept, and a JSON-lines audit log at
  `~/concerto/audit/`.
- **Tauri 2 Desktop client.** Three-panel layout, Monaco diff viewer,
  xterm.js terminal, tray icon. Connects to the Core over a Unix domain
  socket — no network surface at rest.
- **Scheduler, skills, suggestions, `gh` CLI VCS integration.** The V0.1
  fidelity of `design/05` (`/loop`), `design/06` (skills registry),
  `design/07` (rule-based suggestion chips), and `design/13` (PR list /
  view / create via `gh`).

For the full design — and what's deliberately deferred to V1.0 — see
[`design/00_Architecture_Overview.md`](design/00_Architecture_Overview.md)
§10 (phasing) and the 18 sub-system docs (`design/01..18_*.md`).

---

## Install (macOS)

Prerequisites: macOS 13+, Rust stable (`rustup`), Node 20+, `pnpm`, the
`gh` CLI authenticated to GitHub, and the `claude` CLI (Claude Code)
authenticated to Anthropic.

```sh
git clone https://github.com/aminry/concerto.git
cd concerto

# Build concerto-core in release mode and install it as a per-user
# LaunchAgent at ~/Library/LaunchAgents/com.concerto.core.plist.
./scripts/install-macos.sh

# Verify the agent is loaded.
launchctl print "gui/$(id -u)/com.concerto.core"
```

To uninstall, run `./scripts/uninstall-macos.sh` — it removes the
LaunchAgent and the installed binary but leaves your data under
`~/concerto/` and `~/.concerto/` alone.

The full walkthrough — first workspace, first session, troubleshooting —
lives in [`docs/getting-started.md`](docs/getting-started.md).

---

## Run your first agent

1. **Start the Desktop.** From the repo root:

   ```sh
   cd apps/desktop
   pnpm install
   pnpm tauri dev
   ```

   The window opens and connects to the running Core over
   `~/.concerto/core.sock`.

2. **Add a repository.** Use the *Add repository* action and point it at
   any local git repo or a GitHub URL. V0.1 does a full clone into
   `~/concerto/repos/` — sparse / blobless / multi-repo are V1.0.

3. **Create a workspace, spawn a Claude session.** Pick the repo, create
   a workspace (a git worktree under `~/concerto/workspaces/`), then
   start a session. The terminal panel streams the agent's PTY output;
   the right rail shows tool-approval prompts and suggestions.

That's the V0.1 happy path. Close the Desktop, reopen it — the session
is still running. Kill the Core and `launchctl kickstart` it back —
existing PTYs reconnect and ring-buffered output replays.

---

## Embedded mode (testing & standalone)

By default the Desktop dials a separately-installed Core daemon. An
optional **embedded mode** links Core into the Desktop binary and boots
it in-process — one process, no separate daemon install, and a fast
hot-reload dev loop.

Enable it with the `embedded-core` Cargo feature. The launch mode is
chosen at runtime:

| Launch | Behavior |
|---|---|
| default / `CONCERTO_EMBEDDED=1` | Boot Core in-process against your real `~/concerto` data. If a daemon is already running it is detected via the PID lock and the app dials it instead. |
| `CONCERTO_HOME=/path` | Boot Core in-process against an isolated scratch root — runs alongside an installed daemon with no conflict. Use this for testing. |
| `CONCERTO_EMBEDDED=0` / `--external` | Skip embedding; dial an existing daemon (default production behavior). |

Fast dev loop — hot-reloads the frontend (Vite HMR), the `src-tauri` crate,
and `crates/core` (via `cargo watch`), running against your **real**
`~/concerto` data (the same folder the standalone daemon uses):

```sh
make stop-core      # stop the standalone daemon so embedded mode can boot
make dev-embedded   # requires: cargo install cargo-watch
```

`make dev-embedded-scratch` runs the same loop against an isolated scratch
data root instead. `make stop-core` stops the launchd daemon and releases its
PID lock (macOS).

To build a self-contained **Concerto Embedded** app (Desktop + Core in one
binary, installable alongside a normal Concerto) for people who don't want to
install Core separately:

```sh
make build-embedded
```

Tagged releases (`v*`) also publish signed `Concerto Embedded` artifacts
automatically. A headless smoke check for the embedded boot path lives at
`scripts/smoke-embedded.sh` (also `make smoke-embedded`).

In embedded mode, **closing the window quits the app and stops all
agents** — the "agents survive window close" guarantee holds only with
the separate daemon.

---

## Repository layout

| Path | What it holds |
|---|---|
| `crates/` | The Rust workspace: `concerto-core`, `concerto-persist`, `concerto-keychain`, `concerto-proto`, `concerto-agent-host`, and supporting crates. |
| `apps/desktop/` | The Tauri 2 desktop client (React + Vite + Tailwind + Rust shell). |
| `design/` | The 18 sub-system design docs + PRD + tech-stack evaluation. The source of truth for what V0.1 and V1.0 look like. |
| `tasks/` | The V0.1 task breakdown. Every task ships as one PR; see [`tasks/README.md`](tasks/README.md) for the methodology. |
| `scripts/` | `install-macos.sh`, `uninstall-macos.sh`, `smoke.sh`, `regen-interfaces.sh`. |
| `dist/macos/` | Packaging assets (the LaunchAgent plist template). |
| `docs/` | Generated interface summaries + `getting-started.md`. |

---

## Where things live at runtime

| Path | Contents |
|---|---|
| `~/concerto/concerto.db` | SQLite database (workspaces, workareas, sessions). |
| `~/concerto/logs/core-YYYY-MM-DD.log` | Rotating JSON logs. |
| `~/concerto/audit/` | Append-only JSON-lines audit log. |
| `~/concerto/repos/` | Cloned repositories. |
| `~/concerto/workspaces/` | Git worktrees (one per workspace) + `.context/`. |
| `~/.concerto/core.pid` | Single-instance guard. |
| `~/.concerto/core.sock` | Local API (UDS, Tonic / gRPC). |
| `~/.concerto/managed.json` | Optional org-managed permission overrides. |

Override the data root by setting `CONCERTO_HOME` before launching the
Core — the smoke gate does this; you can too for sandboxed experiments.

---

## License

[MIT](LICENSE). The codebase is intentionally easy to fork, audit, and
self-host. See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the DCO sign-off
rule (`git commit -s`) and the license allow-list CI enforces, and
[`TRADEMARKS.md`](TRADEMARKS.md) for how the *Concerto* name may be used.

Security reports: `security@concerto.app` — see [`SECURITY.md`](SECURITY.md).

Third-party licenses: [`THIRD_PARTY_LICENSES.md`](THIRD_PARTY_LICENSES.md).

---

## Contributing

V0.1 is built one task at a time under [`tasks/`](tasks/). Each task file
is the whole prompt: a fresh agent reads the file, implements it,
verifies against the Definition of Done, and lands a single PR. Read
[`tasks/README.md`](tasks/README.md) before opening one.

Sign off your commits (`git commit -s`), keep new dependencies inside
the MIT/Apache-2.0/BSD/ISC allow-list, and don't add phone-home
telemetry, account requirements, or license checks to the Core. The full
rules — and why they exist — are in
[`CONTRIBUTING.md`](CONTRIBUTING.md).

---

## Changelog

See [`CHANGELOG.md`](CHANGELOG.md). Current release: **0.0.1 — V0.1 alpha**.
