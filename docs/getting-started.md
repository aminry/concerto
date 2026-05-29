# Getting started with Concerto

This walkthrough takes you from a fresh clone to a running Claude session
inside a Concerto workspace. Budget: **under 10 minutes** once
prerequisites are installed.

> Concerto V0.1 is alpha. macOS-only, single-repo workspaces, Claude and
> Codex agents only, no relay / mobile / web. See
> [`../README.md`](../README.md) and
> [`../design/00_Architecture_Overview.md`](../design/00_Architecture_Overview.md)
> §10 for what's in vs. out.

---

## 1. Prerequisites

| Tool | Version | Why |
|---|---|---|
| macOS | 13 (Ventura) or newer | LaunchAgent + Tauri 2 baseline. Linux/Windows ports are V1.0. |
| Rust | 1.78+ (stable) | Build the Core. Install via [`rustup`](https://rustup.rs/). |
| Node | 20+ | Tauri 2 renderer toolchain. |
| pnpm | 9+ | Renderer package manager (`npm i -g pnpm`). |
| `gh` CLI | 2.40+ | VCS integration (`brew install gh`, then `gh auth login`). |
| `claude` CLI | latest | The agent itself. Authenticate with `claude login`. |
| `codex` CLI | optional | The other supported V0.1 agent. |

Quick sanity check:

```sh
rustc --version
node --version
pnpm --version
gh auth status
claude --version
```

If any of those fail, fix them before continuing — Concerto shells out
to `gh` and the agent CLIs directly.

---

## 2. Install the Core

```sh
git clone https://github.com/aminry/concerto.git
cd concerto

./scripts/install-macos.sh
```

`install-macos.sh` does four things, in order:

1. Builds `concerto-core` in release mode.
2. Copies the binary to `~/Applications/concerto/concerto-core`.
3. Renders `dist/macos/com.concerto.core.plist` with your `$HOME` and
   the absolute binary path, writing it to
   `~/Library/LaunchAgents/com.concerto.core.plist`.
4. `launchctl bootstrap`s the agent so it starts at login and is
   running right now.

Verify it's loaded:

```sh
launchctl print "gui/$(id -u)/com.concerto.core"
```

You should see `state = running` and a `pid = …`. The Core also leaves a
`~/.concerto/core.pid` file and is listening on `~/.concerto/core.sock`.

Logs:

```sh
ls ~/concerto/logs/
tail -f ~/concerto/logs/core-$(date +%F).log
```

---

## 3. Start the Desktop

In a second terminal, from the repo root:

```sh
cd apps/desktop
pnpm install
pnpm tauri dev
```

`pnpm tauri dev` builds the Rust shell (`concerto-desktop`), starts Vite
on `http://localhost:5173`, and opens the native window. The first run
takes a couple of minutes; subsequent runs are seconds.

The window should connect to the Core over `~/.concerto/core.sock` and
show `ServerCapabilities` populated. If the window opens but shows an
error, the Core probably isn't running — jump to
[Troubleshooting](#5-troubleshooting).

---

## 4. Your first workspace

1. **Add a repository.** From the Desktop, open *Add repository* and
   point it at any local git repo or a public GitHub URL (e.g.
   `https://github.com/aminry/concerto.git`). V0.1 does a full clone
   into `~/concerto/repos/<repo>/`; sparse / blobless / multi-repo land
   in V1.0.
2. **Create a workspace.** Pick the repo, name the workspace, pick a
   base branch. Concerto creates a git worktree under
   `~/concerto/workspaces/<workspace>/` and a `.context/` sibling
   directory for agent-scoped artifacts (per
   [`design/03_Workspace_Session_Manager.md`](../design/03_Workspace_Session_Manager.md)).
3. **Create a workarea.** Workareas are the unit of "one branch of
   thought." V0.1 supports one workarea per workspace; the model is in
   place for multi-workarea workflows in V1.0.
4. **Start a Claude session.** Choose `claude` from the agent picker.
   The Core spawns `concerto-agent-host`, which launches `claude` inside
   a PTY rooted at the worktree. Stdout/stderr stream over
   `Sessions.StreamSessionIO` to the Desktop's xterm.js terminal panel.
5. **Talk to the agent.** Send three messages. Approve tool calls from
   the right-rail prompts. Try one destructive command (e.g. `rm`) and
   watch the red-styled intercept prompt fire regardless of permission
   mode.

That's the V0.1 happy path. Close the Desktop window — the agent keeps
running. Kill the Core with `launchctl kickstart -k
"gui/$(id -u)/com.concerto.core"` — the agent-host stays up, and when
the Core comes back it reconnects to the existing PTY and replays the
ring buffer.

---

## 5. Troubleshooting

**Desktop says "Core unreachable."**
Check the LaunchAgent. `launchctl print "gui/$(id -u)/com.concerto.core"`
should report `state = running`. If not, look at
`~/concerto/logs/launchd-err.log`. A common cause: an old build of
`concerto-core` left behind a stale `~/.concerto/core.pid` — delete it
and `launchctl kickstart` the agent.

**`gh: command not found` when creating a PR.**
V0.1 shells out to the GitHub CLI; there's no fallback to the REST API
yet. `brew install gh && gh auth login`.

**`claude: command not found` when starting a session.**
The Core spawns the `claude` CLI from your `PATH`. If it's installed but
not visible to LaunchAgents, add the install dir to a login-time
`launchctl setenv PATH …` or symlink the binary into `/usr/local/bin/`.
Same applies to `codex`.

**Tauri build fails on first run.**
You probably need the macOS toolchain. Install Xcode Command Line Tools
(`xcode-select --install`) and rerun `pnpm tauri dev`.

**Smoke gate fails after a pull.**
Run `./scripts/smoke.sh` directly — its output is human-readable. The
smoke gate uses a tempdir `CONCERTO_HOME`, so it can't corrupt your real
data.

**"Concerto.app is damaged and can't be opened" / Gatekeeper rejection.**
You've downloaded an **unsigned** local or self-host build. The signed
official releases (see [`../dist/RELEASE.md`](../dist/RELEASE.md)) are
notarized and don't hit this. To run an unsigned build, strip the
quarantine xattr macOS adds to downloaded files:

```sh
xattr -d com.apple.quarantine /Applications/Concerto.app
```

(Adjust the path if the bundle lives elsewhere — e.g. inside
`apps/desktop/src-tauri/target/release/bundle/macos/` for a local
`pnpm tauri build`.) This is a one-time per-bundle workaround; it does
**not** disable Gatekeeper system-wide.

---

## 6. Where things live

| Path | Contents |
|---|---|
| `~/concerto/concerto.db` | SQLite (workspaces, workareas, sessions). |
| `~/concerto/logs/` | Rotating JSON logs from the Core. |
| `~/concerto/audit/` | Append-only JSON-lines audit log. |
| `~/concerto/repos/` | Cloned repositories (full clones in V0.1). |
| `~/concerto/workspaces/` | Git worktrees + `.context/` per workspace. |
| `~/.concerto/core.pid` | Single-instance lock. |
| `~/.concerto/core.sock` | UDS the Desktop dials. |
| `~/.concerto/managed.json` | Optional org-managed permission overrides. |
| `~/Library/LaunchAgents/com.concerto.core.plist` | The LaunchAgent. |
| `~/Applications/concerto/concerto-core` | The installed binary. |

To reset to a clean slate without uninstalling: stop the agent with
`launchctl bootout "gui/$(id -u)/com.concerto.core"`, then move
`~/concerto/` and `~/.concerto/` aside. `./scripts/install-macos.sh`
will recreate the LaunchAgent on the next run.

---

## Next steps

- Skim [`../design/00_Architecture_Overview.md`](../design/00_Architecture_Overview.md)
  — five-minute system overview.
- Read [`../tasks/README.md`](../tasks/README.md) if you plan to
  contribute. Every PR is one task; the methodology and verification
  bar are documented there.
- [`../CHANGELOG.md`](../CHANGELOG.md) lists the V0.1 feature set by
  phase with cross-links to the task files that delivered each piece.
