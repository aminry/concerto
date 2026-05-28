# Concerto

*A device-agnostic orchestration platform for a concerted ensemble of AI coding agents*

**Product Requirements Document**
Version 0.1 (Draft)

| Field | Value |
|---|---|
| Document owner | Amin Roudaki |
| Working name | Concerto (placeholder — final name TBD) |
| Category | Developer tools / AI coding orchestration |
| Target platforms | macOS, Windows, Linux, iOS, Android, Web |
| Status | Draft for internal review |

---

## 1. Executive summary

Concerto is a cross-device orchestration platform for AI coding agents. It runs Claude Code, Codex, and other agents in isolated workspaces on a developer's machine (or in a self-hosted environment), and exposes the same control surface through a polished native desktop app, a mobile app on iOS and Android, and a web app. The desktop, mobile, and web clients are thin views over a single local server process; the server holds the canonical state and runs the agents.

Concerto is built around two defining characteristics. The first is **"your dev workflow follows you"** — lock-screen approvals from a phone on a train, voice-driven session creation, Apple Watch glances. The work of orchestrating agents is asynchronous by nature; the tool that drives it should not be bound to a desk. The second is **"one Core, every device"** — a split-host model where work persists on a workstation or VM and a laptop, tablet, or phone is just a viewport onto it. Together they describe a product that meets real engineering orgs where they actually are:

- Engineers run mixed OSes. A 100-engineer org with macOS, Linux, and Windows laptops needs a single tool that works for all of them.
- Agent runs of 5 to 30 minutes are perfect for "kick it off, then go do something else" — *only* if the developer can check in from anywhere. Without mobile + remote control, every step away from the desk is a dead zone.
- Monorepos in the 10–100 GB range are the dominant repo shape past a certain company size. Full clones into every workspace are a non-starter; cloning takes hours, eats disk, and slows every agent operation.
- Real product changes routinely span 2–5 repositories and need a coordinated set of PRs. Single-repo-per-workspace tools push that coordination onto the developer.

Concerto is built around the workspace primitive — git worktrees, the diff viewer, checkpoints, the checks tab, slash commands, MCP, agent modes, scripts, deep links — and layers on top:

1. A split architecture (server + clients) that runs on macOS, Windows, and Linux.
2. Native iOS and Android apps plus a web client, all with full remote control.
3. Sparse and partial-clone (blobless) checkout for large monorepos, configurable per project.
4. Multi-repo workspaces and multi-PR sessions, so one change set can span several repositories.
5. First-class support for Claude Code's skills, /loop, and scheduled tasks, surfaced through dedicated explorers in the UI.
6. A security model designed for remote access with no third-party servers in the critical path — direct WebSocket over QUIC/TLS where possible, with a minimal relay used only for NAT traversal and push notifications.
7. **Smart suggestions and best-practice prompts** — instead of always typing a reply, the user gets one-tap suggestion chips driven by the agent's current state, learned from how the user actually works, and augmented with automatic prompts when a known best practice applies (e.g. "compact the context" when the window crosses 50%).
8. **Concerto chat — the central maestro** — an always-on chat at the top of the app where the user talks to Concerto itself rather than to any one workspace. The Concerto chat routes prompts to specific workspaces (`@bach run the linter`), surfaces a digest of what every workspace is doing, reminds the user what they were working on when a long session finishes, and proposes next steps. It is the mental-load layer on top of the workspace layer.

> **North star** — A senior engineer should be able to spin up five agents working on three repos before lunch, walk out for coffee, review their progress on a phone, send corrections from a tablet on the train, and merge from a browser on a borrowed machine — without their laptop ever leaving their desk and without any of that code touching a vendor's servers. **When they sit back down, Concerto tells them what changed while they were away and what to do next.**

---

## 2. Why we are building this

### 2.1 The bottleneck is no longer "can I generate code"

In 2024 the bottleneck was code generation quality. In 2026 it is orchestration. A senior engineer using Claude Code or Codex well will routinely have 3 to 8 agents in flight at once: one refactoring an API surface, one writing tests for a module they just changed, one investigating a production incident, one drafting a migration plan, one reviewing a junior's PR. The bottleneck is not the model — it is the human's ability to keep track of, steer, and merge that work.

The first generation of agent-orchestration tools correctly identified that the unit of work is the workspace, that git worktrees are the right primitive, and that a calm, dashboard-style UI beats a wall of terminals. With ~18 months of real practice across many teams, four limitations of that first generation have become structural:

#### 2.1.1 The desktop-only assumption no longer holds

Agent runs take long enough (5–30 minutes for a non-trivial task) that you naturally want to step away. But you also want to know the moment an agent needs you. Without a mobile companion, every step away becomes a "dead zone" — the agent is either working without supervision or blocked waiting for input you can't give. Anthropic shipped Remote Control in early 2026 and a wave of third-party mobile clients (Omnara, Happy Coder, AgentsRoom, Nimbalyst iOS) have appeared. The category is clearly real. None of them combine a polished workspace UI with a real workspace model.

#### 2.1.2 Monorepos are the dominant repo shape at scale

At Coupang, at Google, at Meta, at most companies past a certain size, the dominant repository shape is a large monorepo. A full clone of such a repo is a multi-hour, tens-of-gigabytes operation. A naive "clone the whole thing into a worktree" model multiplies that cost by the number of workspaces. Git has solved this — partial clones (`--filter=blob:none`), sparse checkout (`git sparse-checkout`), sparse index, and the filesystem monitor — but no agent orchestrator surfaces these as first-class settings. They should be.

#### 2.1.3 Real changes span multiple repositories

A backend API change ripples to a mobile client repo, a web client repo, and possibly an infra repo. Today you do this by opening four separate workspaces in whichever orchestrator you use, copying context between them, and manually keeping the four PRs in sync. The workspace abstraction is right; it just needs to scale to N repositories.

#### 2.1.4 Claude Code has grown its own agentic surface

Skills (a folder-based packaging format), the /loop slash command (recurring tasks within a session), and scheduled tasks (recurring tasks that survive session close) are now first-class concepts in Claude Code. They deserve dedicated explorer UI rather than living behind opaque slash commands the user has to remember.

#### 2.1.5 Running many agents in parallel is mentally expensive

Existing tools solved "I can run many agents." They didn't solve "I can keep my head straight while running many agents." With three to eight workspaces in flight, the developer is the bottleneck in a new way: every context switch back into a workspace costs minutes of "wait, what was this one doing?" Every completed session demands manual review even if the change is trivial. Every "I'm awaiting your input" prompt arrives without the context the developer had at hand when they last touched that workspace.

The user's job is partly orchestration — and orchestration is exactly the kind of task an LLM is now good at. Concerto should not just provide a board of workspaces; it should provide an **agent that helps the user drive the board**. That agent should know what each workspace was doing, what changed, what's blocked, what needs review, and how to phrase a one-line prompt that gets the next step done. Equally, the workspace-level agents themselves should not always require the developer to construct the next prompt from scratch — they should propose one or more next steps every time they finish a turn.

These are two sides of the same problem (reducing the developer's cognitive load), and Concerto addresses them with two new mechanisms: per-agent **suggestion chips** at the workspace level, and the **Concerto chat** at the top level.

### 2.2 What people will be able to do that they cannot do today

1. Start three agents on a Monday morning standup, **then leave for a customer meeting and approve their PRs from a phone on the way home.**
2. Work on the same Coupang monorepo from a personal Linux desktop, a work MacBook, and an iPad — **without re-cloning the 40 GB repo on each machine.**
3. Ship a vertically integrated feature **that touches the API, the iOS app, and the analytics pipeline, with one Concerto session producing three coordinated PRs.**
4. Schedule a nightly "/loop" task **that scans for new CVEs in dependencies and opens draft PRs with patches, then approve those PRs from the breakfast table.**
5. Browse and install skills **the same way you browse VS Code extensions, with per-project skill scoping and a curated, searchable explorer.**
6. Hand a junior engineer access to a senior's "Concerto server" **over an end-to-end-encrypted tunnel for live pair-programming review.**
7. Drive five active workspaces from a single chat at the top of the app — **typing "@mozart try the same fix bach just landed" and watching Concerto route, prompt, and report back.**
8. Sit back down after a two-hour meeting and have Concerto tell them, in two sentences, **what each of their six workspaces did while they were away, what's blocked, and which one to look at first.**
9. Accept a one-tap suggestion — "Compact the context" / "Run the failing test" / "Open PR for review" — **without having to type the prompt or remember the slash command.**

### 2.3 Why now

- **Models are good enough.** Claude 4.6/4.7 and GPT-5.x make 30-minute autonomous tasks routinely useful.
- **Agent surface area is stable.** Skills, /loop, and scheduled tasks are not going away. MCP is the consensus interop layer.
- **The orchestration category is established.** Multiple tools have shown developers want a dashboard over their agents. The remaining work is doing it across devices, monorepos, and multi-repo changes — which is where Concerto starts.
- **Mobile/remote is still an unsolved problem.** Of the five mobile-capable contenders surveyed (Omnara, Happy, AgentsRoom, Nimbalyst, Anthropic Remote Control), none of them combine a polished workspace UI with real workspace orchestration and a privacy story that holds up in an enterprise.
- **Monorepo support is uniformly bad.** Not a single competing tool surfaces partial clone or sparse checkout. This is a moat against the current incumbents.

---

## 3. Target users and personas

Concerto is built for software engineers who already use one or more terminal-based AI coding agents. The primary user is not a non-developer. The product is not aiming at the "no-code app generator" segment.

### 3.1 Primary persona — the senior IC who runs many agents at once

| Attribute | Profile |
|---|---|
| Role | Senior / Staff / Principal IC at a software-driven company |
| Team size | 20–500 engineers in their division |
| Repo shape | One or more large monorepos plus a handful of smaller repos |
| Agents used | Claude Code on Pro/Max, Codex CLI, possibly Gemini CLI |
| Devices | MacBook Pro at desk, iPhone or Android on person, sometimes an iPad |
| Pain points | Context-switching between agents; losing flow when an agent blocks; rebuilding mental state when returning to a long-running task; cloning monorepos repeatedly across machines |
| Success | Closes more PRs per week with the same number of hours at the keyboard, and reclaims evenings/weekends because supervision becomes a 30-second phone check instead of a 30-minute laptop session. |

### 3.2 Secondary persona — the engineering manager / tech lead

Tech leads need visibility into what their team's agents are doing without micromanaging. They benefit from Concerto's team-shared views (described later) and from the audit trail of agent actions.

### 3.3 Secondary persona — the founder / solo builder

Solo builders or small-team founders running 3–5 agents in parallel to ship a startup product. They need everything a desktop orchestrator offers plus the mobile dimension.

### 3.4 Tertiary persona — the enterprise platform team

Platform teams at large companies (think Coupang Marketplace, Stripe Internal Tools, Shopify Production Engineering) who want to standardize on a single orchestration layer for their engineers, with managed settings, audit logging, and the ability to self-host the server tier in a private VPC.

### 3.5 Non-goals

- Non-developers building apps from natural language. That's a different product (Lovable, v0, Bolt).
- Replacing IDEs. Concerto does not aim to be your editor. It launches into Cursor, VS Code, Zed, Xcode, JetBrains, Vim — whatever you use.
- Replacing the agent. Concerto is a thin orchestrator over Claude Code, Codex, and other CLIs. It does not implement its own LLM stack.
- Hosted cloud agents as the default mode. Concerto's primary execution model is local, on the user's hardware. A hosted "Concerto Cloud" tier is an explicit V2 stretch goal, not the V1 product.

---

## 4. Product principles

These principles resolve trade-offs when two features conflict. They are listed in priority order — if there is a tension, the earlier principle wins.

### 4.1 Local-first by default

Your code lives on your machine. The server runs on your machine. The clients connect to your machine. Concerto is not a vendor extracting your code for analytics. If we ever offer cloud execution, it is opt-in and clearly labelled.

### 4.2 Calm UI over busy UI

Calm by default. Whitespace, restrained color, status by glanceable dots rather than badges, no notification spam. New features earn their place on the screen.

### 4.3 Mobile is a first-class peer, not a remote view

The mobile app is not a tiny web page. It is a designed-for-touch surface with its own UX patterns (swipe between projects, pinch-to-zoom diffs, voice input, push-driven inbox). It can do anything the desktop can do that is sensible to do on a phone.

### 4.4 Workspace is the unit

The workspace is the unit of work. One workspace = one branch = one working tree = one stream of work = one PR. Everything else (multi-repo, scheduled tasks, skills, sessions) is layered on this primitive. We do not invent new abstractions where this one works.

### 4.5 Privacy of remote connections is non-negotiable

When a developer connects from a phone to their laptop, the traffic must be end-to-end encrypted and no third party is in a position to read it. Our minimal relay (for NAT traversal and push notifications) sees ciphertext only.

### 4.6 Standard primitives, not custom replacements

Use git worktrees, not a custom CAS. Use sparse-checkout, not a custom filter. Use MCP, not a proprietary tool protocol. Use SKILL.md, not Concerto Skills. We add a UI over standard primitives; we do not fragment the ecosystem.

### 4.7 The dashboard never lies

If a Concerto UI says an agent is "waiting for input," then the agent is actually waiting for input right now. Status comes from the running process, not from cached state that might be stale. This is harder than it sounds and is the single biggest UX bug in competing mobile tools today.

### 4.8 Reduce cognitive load, never add it

Every Concerto feature must make orchestrating N agents easier than orchestrating one. If a feature adds a new place the user has to look, it must remove two. Suggestion chips, the Concerto chat, and Workflow Explorer notifications are all instances of this principle: they exist to absorb the mental work of "what was I doing here, what's next" so the developer can keep their attention on the code that matters. Conversely, Concerto never pads chat output, never surfaces noisy status changes, and never asks a question the system could answer from state it already holds.

---

## 5. Architecture overview

Concerto is a single-product, multi-surface system. Two layers:

- **Concerto Core (the server).** A long-lived process that runs on the user's machine. It owns repositories, workspaces, git, agent subprocesses, configuration, and persistence. There is exactly one canonical Core per user/machine.
- **Concerto Clients.** The desktop app (macOS / Windows / Linux), the iOS app, the Android app, and the web app. All of them are views over the Core. None of them holds canonical state.

### 5.1 High-level diagram

```
                      ┌────────────────────────────────────────────┐
                      │           Concerto Core (daemon)            │
                      │   - Workspace manager (git worktrees)      │
                      │   - Repo manager (sparse / partial clones) │
                      │   - Agent supervisor (Claude Code, Codex,  │
                      │     Gemini CLI, custom MCP-backed agents)  │
                      │   - Scheduler (/loop, scheduled tasks)     │
                      │   - Skills registry                        │
                      │   - Persistence (SQLite + checkpoints)     │
                      │   - Local API (gRPC + WebSocket)           │
                      └─────┬──────────────────┬────────────────┬──┘
                            │                  │                │
                  ┌─────────▼────┐    ┌────────▼─────┐  ┌───────▼─────────┐
                  │ Desktop app  │    │ Mobile apps  │  │   Web app       │
                  │ (Tauri or    │    │ (React       │  │  (Same React    │
                  │  Electron;   │    │  Native /    │  │   shell as      │
                  │  Mac/Win/Lin)│    │  Swift +     │  │   desktop's     │
                  │              │    │  Kotlin)     │  │   inner view)   │
                  └──────────────┘    └──────────────┘  └─────────────────┘
                            ▲                  ▲                ▲
                            │                  │                │
                            └──────────────────┼────────────────┘
                                               │
                              ┌────────────────▼────────────────┐
                              │   Relay (minimal, optional)     │
                              │   - NAT traversal token swap    │
                              │   - APNs / FCM push delivery    │
                              │   - Sees ciphertext only        │
                              └─────────────────────────────────┘
```

> **Key separation** — The Core is the source of truth. Every client renders the Core's state and dispatches commands back. No client persists workspace or chat state on its own. Closing every client should not affect what the Core is doing.

### 5.2 Why split the app this way

1. **A 100-engineer rollout requires Linux and Windows.** Splitting the Core from the UI lets the same Core run on a Linux dev box and a Mac laptop with identical behavior.
2. **Mobile and web are vastly easier to build right when they don't fight the desktop for ownership of state.** The Core arbitrates conflicts; clients are stateless renderers.
3. **Self-hosting in V2 is essentially free.** A platform team can run Concerto Core on a beefy Linux box in their VPC and have engineers connect to it from anywhere.
4. **Open-sourcing the Core (eventually) is straightforward.** It is the only place real logic lives. Clients can stay closed source if we want; the Core protocol is documented.

### 5.3 Local API

The Core exposes one local API on a Unix socket (or named pipe on Windows). It speaks two protocols on top of that socket:

- **gRPC** for synchronous RPCs: list workspaces, create workspace, fetch diff, run command. Strongly typed, generates client libraries for every supported language.
- **WebSocket-style streaming** for live state: agent output, status changes, file changes, checkpoint creation. Each client subscribes to the streams it cares about.

When a client is on the same machine as the Core, it connects directly to the local socket. When a client is remote (phone, tablet, browser on another laptop), it connects through one of the two remote transports described in section 16.

### 5.4 Component responsibilities

| Component | Owns | Does not own |
|---|---|---|
| Repo manager | Cloning, fetching, sparse-checkout config, blobless refresh, multi-repo session linkage | Workspace state, branch state |
| Workspace manager | Worktrees, branches per workspace, the `.context` directory, archive lifecycle | Running agents |
| Agent supervisor | Launching Claude Code / Codex / others, streaming I/O, capturing checkpoints, restart on crash | Repo state, git |
| Scheduler | /loop registrations, scheduled tasks, cron expressions, jitter, fan-out | Agent execution (delegates to supervisor) |
| Skills registry | Discovery of project, personal, plugin skills; install/uninstall; visibility overrides | Skill execution (Claude Code handles) |
| **Suggestion engine** | Loading `suggestions.toml`, running rule triggers on workspace events, ranking chips, recording acceptance | Sending prompts (delegates to agent supervisor) |
| **Maestro agent** | The Concerto chat's LLM session, routing tools, digest generation, cross-workspace summaries | Direct code edits, shell access |
| Sync engine | Connecting remote clients, key exchange, push tokens, state diffing for low-bandwidth links | Persistence |
| Persistence | SQLite for metadata, on-disk worktrees for code, encrypted blobs for secrets, **suggestion learning counters, Concerto chat history** | Anything cross-Core |

---

## 6. Core feature set

These are the foundational features Concerto ships. New capabilities (§7 onward) build on top of them, not in place of them.

### 6.1 Project, repository, workspace model

A Project is the Concerto entry for a codebase. It holds repository settings, scripts, instructions, and the list of workspaces for that codebase. A Workspace is an isolated copy of a project and repository, on its own branch, with its own working tree. One workspace = one branch = one shippable unit of work.

### 6.2 Isolated workspaces via git worktrees

Each workspace is backed by a git worktree. Only files git is tracking are copied, which keeps node_modules / .venv / .env from duplicating across worktrees. On large monorepos, sparse or blobless worktrees are available (see section 9).

### 6.3 Files to copy

Concerto supports a configurable rule for which gitignored files (.env, IDE settings, credentials) are copied into new workspaces. The rule is surfaced in the repository settings UI.

### 6.4 The diff viewer

A side-by-side or unified diff view of the workspace's changes, with:

- Per-file navigation in a left sidebar with change counts.
- Switching between unified and split views.
- Filtering by commit so review can happen one commit at a time.
- Inline review comments that become composer attachments — clicking a line and writing a comment sends that comment back to the agent with the file/line attached as context.
- Surfacing GitHub review threads when the workspace has a PR open.
- Marking threads as resolved, which flows back to the Checks tab.

### 6.5 Checkpoints

Automatic snapshots of an agent's changes between turns, stored in a private git ref outside the working branch. The user can hover over a previous message and revert to that turn; this deletes all subsequent messages and reverts code to that point. Concerto surfaces a clear warning that reverts are destructive and that running multiple chats in one workspace makes checkpoint semantics murky.

### 6.6 The Checks tab

A merge-readiness panel that aggregates:

- Git status (clean, dirty, conflicting).
- Pull request metadata (open, draft, merged, closed; title and base branch).
- CI and status checks (each named check with pass/fail/pending state).
- Deployment status from GitHub deployments API.
- Outstanding review comments and threads.
- Workspace todos.

The Checks tab is the last gate before merge. Concerto may discourage merge while blockers are open and surfaces those blockers prominently.

### 6.7 Workspace lifecycle (create, work, review, PR, merge, archive)

End-to-end workflow:

1. Create a workspace from a branch, an existing PR, a GitHub issue, or a Linear issue.
2. The workspace gets a new branch (Concerto renames it after the first chat to match the work).
3. Setup script runs on workspace creation.
4. Run script starts the dev server on a workspace-specific port (the `CONCERTO_PORT` variable).
5. Agents work in the workspace.
6. Diff Viewer is used for review.
7. Create PR action opens the PR.
8. Concerto follows GitHub Actions and status checks.
9. Merge when green.
10. Archive runs an archive script (e.g., stop services, clear caches).
11. Archived workspaces can be restored from a separate page with chat history intact.

### 6.8 Repository scripts (`concerto.json`)

A repo-level file `concerto.json` at the project root configures:

- **`scripts.setup`** — runs when a workspace is created. Typical: `npm install`, `bundle install`, `uv sync`.
- **`scripts.run`** — runs when the user clicks the Run button. Typical: `npm run dev`.
- **`scripts.archive`** — runs before archiving. Typical: `docker compose down`.
- **`runScriptMode`** — concurrent or nonconcurrent (whether multiple workspaces can run dev servers simultaneously).
- **`enterpriseDataPrivacy`** — disables features that require external AI providers, like AI-generated chat titles and custom MCP servers.

### 6.9 Repository settings on the local machine

Concerto maintains per-machine, per-repo settings that override `concerto.json`. Precedence: local repo settings override the shared file, so a developer can experiment without breaking teammates. The UI surfaces all of these:

- Workspace path (where on disk the worktrees live).
- Files to copy patterns.
- Git remote behavior (whether to push automatically, which remote to push to).
- Setup / run / archive scripts.
- Spotlight testing — root-based automated test runs on a directory.
- Code review preferences (prompts and tone of agent reviews).
- Create PR preferences (template, default base branch, auto-link issues).
- Fix errors preferences.
- Resolve conflicts preferences.
- Branch rename preferences.
- General preferences (default model, default mode, default agent).

### 6.10 Agent modes — Plan, Fast, reasoning levels, personalities

Concerto exposes:

- **Plan Mode** — the agent produces a plan before editing files. Supported by both Claude Code and Codex.
- **Fast Mode** — prioritizes speed; appropriate for narrow edits.
- **Thinking / reasoning level** — when the model exposes it (Claude's extended thinking, Codex's reasoning effort).
- **Codex personalities** — session-level personality controls for Codex.
- **Checkpoints** — see 6.5.
- **Skills** — both Claude Code and Codex can use skills inside Concerto.

Concerto adds (see section 11) a Skill Explorer UI and a Workflow Explorer for /loop and scheduled tasks.

### 6.11 Multi-agent orchestration (Claude Code + Codex in one workspace)

Any number of agents (Claude Code, Codex, Gemini CLI, custom MCP-backed agents) can run in tabs within a workspace, sharing the same branch and files.

### 6.12 Slash commands

Reusable prompts stored as Markdown files in `.claude/commands/` or `.codex/commands/`, appear in the chat composer when the user types `/`. Concerto surfaces these in the chat composer and exposes them through the Skill Explorer for discoverability (see 11.2).

### 6.13 MCP (Model Context Protocol) support

Concerto picks up MCP servers configured at the user level (`claude mcp add`) and at the project level (`.mcp.json` at repo root), and also supports Codex's `~/.codex/config.toml` and per-project `.codex/config.toml`. An MCP Servers panel in Settings shows which servers are active, what tools they expose, and whether they should be allowed in the current session.

### 6.14 Deep links (`concerto://`)

Concerto supports `concerto://` URLs that open the app and trigger actions (open a specific workspace, create a workspace from a Linear issue URL, etc.). Covered actions include at least:

- Open workspace by ID.
- Create workspace from GitHub issue / Linear issue / branch.
- Jump to settings panel.
- Open a specific file in the diff viewer.
- Trigger a slash command.

### 6.15 Spotlight testing

A root-based automated test runner Concerto can invoke against a directory. Used for "verify this branch passes its tests before I look at it." Integrated into the Run flow.

### 6.16 Big Terminal Mode

An experimental layout mode that gives more screen real estate to the terminal in the workspace view. Concerto keeps it as a layout option, and extends it on mobile by offering a "fullscreen terminal" view that is touch-optimized (large hit targets for common keys, swipe-to-scroll the buffer).

### 6.17 Composers (workspace naming)

Concerto names workspace directories after composers — Bach, Mozart, Chopin, Grieg, Gershwin, Debussy, Brahms, Britten, Ravel, Holst, and so on, cycling through a curated list. It is a small touch that makes long lists of workspaces easier to scan, and a deliberate echo of the product's defining metaphor (a concerto for a soloist and an orchestra of agents). The sidebar shows both the composer name and the branch.

### 6.18 Keyboard shortcuts

Concerto ships with a rich keyboard shortcut set (Cmd+Shift+N for new workspace, Cmd+Shift+D for diff viewer, Cmd+Shift+P for create PR, etc.) on macOS, with platform-appropriate equivalents on Windows and Linux (Ctrl-based) and a discoverable shortcut palette accessible by `?`.

### 6.19 Privacy and enterprise data privacy

Concerto offers an `enterpriseDataPrivacy` toggle that disables features requiring external AI providers (AI-generated chat titles, custom MCP servers). Section 16 covers the more granular controls layered on top of it.

### 6.20 Provider configuration

Concerto supports Anthropic, OpenAI, Bedrock, Vertex, Gemini, OpenRouter, and Vercel AI Gateway, and passes through environment variables like `ANTHROPIC_BASE_URL`, `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_USE_BEDROCK`, `CLAUDE_CODE_USE_VERTEX`, `OPENAI_BASE_URL`. Anthropic Claude-on-AWS is supported as a first-class entry.

### 6.21 IDE integration (Cursor, VS Code, JetBrains, Zed, Xcode)

Concerto opens a workspace in the user's IDE of choice. Supported IDEs include VS Code, Cursor, Zed, Xcode, and JetBrains, plus a "no IDE, terminal only" mode for cloud-style usage from a phone.

---

## 7. What Concerto adds beyond the foundational layer

A summary table of everything Concerto layers on top of the core feature set in §6. Each row is expanded in the sections that follow.

| # | Feature | Why it matters | V1 / V2 |
|---|---|---|---|
| 1 | Concerto Core (server) + clients | Lets the same product run on Mac, Win, Linux; enables remote control; enables self-hosting later | V1 |
| 2 | Native iOS + Android apps | Closes the dead-zone gap when stepping away from desk | V1 |
| 3 | Web client | Access from a browser anywhere, including borrowed machines | V1 |
| 4 | Secure remote transport (E2EE) | No third party reads your code in flight | V1 |
| 5 | Sparse-checkout per workspace | Monorepos become usable; workspace creation goes from minutes to seconds | V1 |
| 6 | Blobless / partial clone per project | Initial clone of a large repo goes from hours to minutes | V1 |
| 7 | Sparse-index for sparse worktrees | Sub-second git status / checkout on monorepos | V1 |
| 8 | Multi-repo workspaces | One session, N repos, coordinated PRs | V1 |
| 9 | Multi-PR session management | Track and merge a set of related PRs together | V1 |
| 10 | Skill Explorer | Browse, install, scope, and override skills | V1 |
| 11 | Workflow Explorer (/loop) | Surfaces session-scoped recurring tasks in the UI | V1 |
| 12 | Scheduler (persistent scheduled tasks) | Tasks that survive session and machine restarts | V1 |
| 13 | Voice input on mobile | Hands-free dictation of prompts while commuting | V1 |
| 14 | Localhost preview tunnelling on mobile | See the workspace's dev server in the phone's browser | V1 |
| 15 | Apple Watch glance | One-line status of all workspaces; tap to act | V2 |
| 16 | Team-shared sessions (read-only) | A manager can spectate without taking control | V2 |
| 17 | Self-hosted Concerto Core in private VPC | Enterprise platform-team rollout pattern | V2 |
| 18 | Sparse-checkout learning mode | Auto-detect which paths an agent touches and propose sparse cones | V2 |
| 19 | Audit log of all agent actions | Compliance / forensics for regulated teams | V2 |
| 20 | Smart suggestions (per-workspace chips) | One-tap next-step prompts that reduce typing and surface best practices the user might forget | V1 |
| 21 | Concerto chat (central maestro) | Drive all workspaces from one chat; get back-from-meeting digests; reduce mental context-switching | V1 |

---

## 8. Concerto Core (server) and clients in detail

### 8.1 Server (Concerto Core)

The server is the only stateful component. Everything else is a renderer.

#### 8.1.1 Process model

- **Background daemon.** On macOS, runs as a launchd LaunchAgent. On Windows, as a Service or a logon-time scheduled task. On Linux, as a systemd user unit.
- **Single instance per user.** A second Core invocation detects the running one and exits.
- **Auto-restart.** If the Core crashes, it restarts within seconds and replays any in-flight agent process state.
- **Headless.** The Core does not draw a window. Its only UI is a tray/menu-bar icon showing online status and unread counts.

#### 8.1.2 What lives in the Core

- **SQLite database** — projects, workspaces, branches, chat history, checkpoints, todos, scheduled tasks, settings, audit log.
- **On-disk worktrees** — at a configurable root path. Default: `~/concerto/workspaces`.
- **Encrypted secrets store** — uses the OS keychain (Keychain on macOS, Credential Manager on Windows, Secret Service / libsecret on Linux). Stores API tokens, GitHub PATs, push notification credentials, device pairing keys.
- **Live state** — running agent processes, open WebSocket subscriptions, pending tool approvals.
- **Static config** — `~/.concerto/config.json` (per-user) and `~/.concerto/managed.json` (org-managed).

#### 8.1.3 The local API

Two protocols sharing one local transport:

**gRPC for RPCs.** Used for any synchronous operation that returns a defined result. Examples:

```proto
service Concerto {
  // Projects
  rpc CreateProject(CreateProjectRequest) returns (Project);
  rpc ListProjects(google.protobuf.Empty) returns (ListProjectsResponse);

  // Workspaces
  rpc CreateWorkspace(CreateWorkspaceRequest) returns (Workspace);
  rpc GetWorkspace(GetWorkspaceRequest) returns (Workspace);
  rpc ArchiveWorkspace(ArchiveWorkspaceRequest) returns (google.protobuf.Empty);

  // Agents
  rpc StartAgent(StartAgentRequest) returns (AgentSession);
  rpc SendMessage(SendMessageRequest) returns (google.protobuf.Empty);
  rpc ApproveTool(ApproveToolRequest) returns (google.protobuf.Empty);

  // Git / repo
  rpc UpdateSparseCones(UpdateSparseConesRequest) returns (google.protobuf.Empty);
  rpc CreatePullRequest(CreatePullRequestRequest) returns (PullRequest);

  // Scheduler
  rpc CreateLoop(CreateLoopRequest) returns (Loop);
  rpc CreateScheduledTask(CreateScheduledTaskRequest) returns (ScheduledTask);

  // Skills
  rpc ListSkills(ListSkillsRequest) returns (ListSkillsResponse);
  rpc InstallSkill(InstallSkillRequest) returns (Skill);
}
```

**WebSocket-style streams for live state.** Each subscription returns a stream of typed events. Example streams:

- **workspace.events** — all workspace state changes (status, branch, dirty, conflict).
- **agent.io.<sessionId>** — every line of stdout / stderr / tool call from a specific agent.
- **agent.events.<sessionId>** — high-level events (idle, working, blocked, finished, failed, awaiting approval).
- **diff.<workspaceId>** — the working tree changed; clients refresh the diff viewer.
- **checks.<workspaceId>** — CI status, PR state, deployment changes from GitHub webhooks.

#### 8.1.4 Persistence and crash recovery

Agent stdout is streamed to both subscribers and a per-session log file. If a client disconnects mid-stream and reconnects, the Core replays the log from the client's last acknowledged offset. If the Core crashes, on restart it scans for orphaned agent processes (via pidfile + cookie) and adopts them; clients reconnect transparently.

### 8.2 Desktop client

Native-feeling app on macOS, Windows, and Linux. Two architecture options under evaluation:

- **Tauri (preferred).** Rust core (sharing code with the Core daemon), small bundle (~15 MB), native WebView on each platform. Better RAM and battery on laptops.
- **Electron (fallback).** Larger bundle (~150 MB) but the ecosystem is more mature. Use this only if Tauri's native-webview quirks are blocking shipping.

Decision deferred to the engineering team after a one-week prototype.

#### 8.2.1 Layout

Three-panel layout:

```
┌──────────────────────────────────────────────────────────────────────────┐
│  ◐ Concerto                                                       ⌘K   ⚙  │
├──────────────┬───────────────────────────────────────────────────────────┤
│  PROJECTS    │   Bach  ·  feat/scroll-to-bottom-btn         ▶ Run  ⌘P PR │
│  ▸ coupang   │  ┌─────────────────────────────────────────────┬────────┐│
│    monorepo  │  │  Chat / Plan / Diff / Checks / Terminal     │ Sched  ││
│  ▸ mp-android│  │                                             │ Skills ││
│  ▸ mp-ios    │  │                                             │ Todos  ││
│  ▼ side proj │  │                                             │ Files  ││
│     chopin   │  │                                             │ MCP    ││
│     bach   ●│  │                                             │        ││
│     mozart ◐│  │   [composer]  type a message to the agent…   │        ││
│   + new      │  └─────────────────────────────────────────────┴────────┘│
├──────────────┴───────────────────────────────────────────────────────────┤
│  ● coupang-monorepo synced 2m ago   ●●●● 4 agents running   ⊕ Loop·2     │
└──────────────────────────────────────────────────────────────────────────┘
```

The left sidebar shows projects with their workspaces nested underneath, each workspace tagged with a colored status dot (green = running, amber = awaiting input, blue = idle, grey = archived). The center panel is the working area. The right rail holds context-sensitive tabs (scheduler, skill explorer, todos, file tree, MCP). The bottom bar is a single-line status indicator.

### 8.3 iOS app

Native Swift / SwiftUI. Not a wrapped web app.

#### 8.3.1 Layout

```
┌──────────────────────────┐    ┌──────────────────────────┐
│   Concerto     ⌘  ⚙       │    │  ←  bach    ●  ⋮         │
│                          │    │                          │
│  ●  coupang-monorepo     │    │ Branch  feat/scroll-btn  │
│     4 workspaces  •      │    │ ──────────────────────── │
│                          │    │                          │
│  ▼  chopin   ●           │    │ Chat   Diff   Checks     │
│     refactor:auth-flow   │    │                          │
│  ▼  bach     ◐ block     │    │ ┌──────────────────────┐ │
│     feat:scroll-button   │    │ │ Agent (Claude 4.7)   │ │
│  ▼  mozart   ◐ awaiting  │    │ │ "I've drafted the    │ │
│     fix:NPE-checkout     │    │ │  changes to handle   │ │
│  ▼  grieg    ✓ done      │    │ │  the empty case…     │ │
│     test:flaky-fixes     │    │ │  Should I add the    │ │
│                          │    │ │  regression test?"   │ │
│  + new workspace         │    │ └──────────────────────┘ │
│                          │    │                          │
│  ▾ side projects         │    │ [ Yes ] [ No ] [ Type ]  │
│  ▾ mp-android            │    │                          │
│                          │    │ 🎙  send a message       │
└──────────────────────────┘    └──────────────────────────┘
        Workspaces list                 Workspace detail
```

#### 8.3.2 What it does well

- **Kanban-style status view.** At a glance, see every workspace across every project, colored by state.
- **One-tap approvals.** When an agent needs a tool approval or wants to commit, the user gets a push notification with action buttons: Approve, Deny, Open.
- **Touch-first diff viewer.** Pinch to zoom, swipe between files, long-press a line to comment.
- **Voice input.** Hold the microphone, dictate, release to send. Long-running iOS Speech Recognition runs on-device.
- **Inline localhost preview.** The Core opens a tunnel and the phone displays the workspace's dev server in a WebView. Useful for "is the homepage broken?" while commuting.
- **Push notifications.** Delivered through APNs (Apple Push Notification service); only the wakeup is via Apple, the payload is fetched directly from the Core after wakeup.

#### 8.3.3 What it deliberately does not try to do

- Full code editing. The chat composer is the primary input. A line-comment is the only fine-grained editing surface.
- Long-form prompt authoring. Voice + short text only. For multi-screen prompts, hand off to desktop.
- Multi-window IDE-like layouts.

### 8.4 Android app

Native Kotlin / Jetpack Compose. Feature parity with iOS, FCM (Firebase Cloud Messaging) instead of APNs. The same UX patterns translated to Material Design 3.

### 8.5 Web client

A single React/TypeScript SPA. It shares ~80% of its component tree with the desktop client (which embeds the same SPA inside its native shell). The remaining 20% is desktop-only chrome (window controls, native menus, system tray).

Used for two scenarios:

1. **"I'm on a borrowed machine and want to check in."** A coworker's laptop, a hotel business center, an iPad in browser mode. Open a URL, log in, control your Core.
2. **"My team uses Linux and the desktop app isn't available there yet."** Web is the fallback for the long tail of platforms.

### 8.6 Tray / menu-bar app

A minimal always-on UI for the desktop:

- Online / offline indicator for the Core.
- Per-project unread badge.
- Quick start: pick a project, create a workspace, paste a task.
- Pending approvals popover.
- "Pair this device" QR code for a phone joining the Core.

---

## 9. Monorepo support: sparse checkout, blobless clone, sparse index

On large monorepos, a naive "clone the full repository into each worktree" approach is a non-starter: for a 40 GB monorepo with 2M files, workspace creation takes 30+ minutes, every git operation is slow, and disk fills fast. Concerto solves this with sparse + blobless checkout.

Concerto treats large monorepos as a first-class shape. Three Git capabilities are exposed in the project settings:

### 9.1 Partial clone (blobless)

A `git clone --filter=blob:none` clone downloads all commit and tree objects but defers blob (file content) download until needed. On a 40 GB Linux-kernel-sized repo this turns a 20-minute clone into ~2 minutes and saves ~80% of disk. When git needs a specific blob, it fetches it from the remote on demand. The trade-off is that operations like `git blame` and offline work require internet the first time those blobs are touched.

In Concerto, this is a per-project setting in Repository Settings:

```
Project: coupang-monorepo
  Clone strategy:
    ( ) Full clone  (default for small repos)
    (•) Blobless clone   git clone --filter=blob:none
    ( ) Treeless clone   git clone --filter=tree:0   (advanced)
  Pre-fetch:
    [✓] Eagerly pre-fetch blobs touched by HEAD
    [✓] Pre-fetch blobs for the workspace's sparse cone
    [ ] Pre-fetch on idle in the background
```

### 9.2 Sparse checkout (per workspace)

`git sparse-checkout` lets a working tree contain only a subset of the repository's files. Combined with the sparse index, this makes `git status`, `git checkout`, and file scans bounded by the sparse cone rather than the full repo.

In Concerto this becomes a per-workspace cone definition. When you create a workspace, you can choose:

- **All files (default for small repos).** Standard full clone.
- **Sparse cones.** Specify one or more directories (e.g., `services/checkout`, `libs/auth`). Files outside those cones are not materialized on disk. The agent only sees what is in the cones.
- **Inherit from project.** The project carries a default sparse cone set; new workspaces inherit it.

The UI for picking cones is a tree of the repository with checkboxes:

```
New workspace · sparse cones
  ┌─────────────────────────────────────────────────────────┐
  │  Filter: ____________________                           │
  │                                                         │
  │  ▾ apps/                                                │
  │    [✓] checkout-web/                  ~ 1,820 files     │
  │    [ ] checkout-mobile/               ~ 4,210 files     │
  │    [ ] seller-portal/                 ~12,030 files     │
  │  ▾ libs/                                                │
  │    [✓] auth/                          ~   312 files     │
  │    [✓] payments-sdk/                  ~   840 files     │
  │    [ ] analytics-sdk/                 ~ 1,128 files     │
  │  ▾ tools/                                               │
  │    [ ] ml-experiments/                ~25,400 files     │
  │                                                         │
  │  Selected:   3 cones    ~ 2,972 files / 4,830 total MB  │
  │              Disk used: ~ 480 MB (vs ~28 GB full)       │
  │                                                         │
  │  [ Suggest cones based on this issue ]                  │
  │                                                         │
  │  [Cancel]                            [Create workspace] │
  └─────────────────────────────────────────────────────────┘
```

> **Suggest cones** — When the workspace is being created from a GitHub or Linear issue, Concerto can call the agent in plan mode to read the issue and propose which cones are needed. This is the killer ergonomics feature — most users won't want to manually pick cones; the agent's plan-mode read of the issue picks them.

### 9.3 Sparse index

When sparse checkout is on, enable the sparse index unconditionally (`git config core.sparseCheckoutCone true` and the modern sparse index). This keeps the Git index proportional to the sparse cone size, not the repository size. The performance impact is dramatic: on a 2M-file repo with a 100k-file sparse cone, `git status` goes from seconds to milliseconds.

### 9.4 Filesystem monitor

On macOS and Windows, enable `core.fsmonitor = true` and run Git's built-in FS monitor daemon. This eliminates the file-stat traversal cost on every Git command. The Core manages the daemon lifecycle so the user doesn't have to know it exists.

### 9.5 The combined recommended setup for very large repos

```bash
git clone \
  --filter=blob:none \
  --sparse \
  --depth=1 \
  --no-checkout \
  git@github.com:coupang/marketplace.git mp

cd mp
git sparse-checkout init --cone
git sparse-checkout set apps/checkout-web libs/auth libs/payments-sdk
git checkout main

# Performance settings written by Concerto:
git config core.fsmonitor true
git config core.untrackedCache true
git config feature.manyFiles true
git config core.commitGraph true
git config gc.writeCommitGraph true
git maintenance start
```

### 9.6 What changes per workspace vs per project

| Setting | Scope | Why |
|---|---|---|
| Clone strategy (full / blobless / treeless) | Project | Determined once when the repo is first added |
| Sparse cones | Workspace | Different tasks touch different parts of the repo |
| Sparse index enabled | Project (auto) | Always on when sparse checkout is on |
| Filesystem monitor | Project (auto) | Cross-cutting Git performance setting |
| Shallow depth | Project | Trade-off between history and disk |
| Background maintenance | Project | Runs git maintenance to keep packs healthy |

### 9.7 Worktree behavior with sparse + blobless

Worktrees share the parent repository's object database. A blobless clone's object store has metadata but missing blobs; each worktree only triggers blob fetches for files in its own sparse cone. The Core monitors these fetches and pre-warms them in the background when bandwidth is plentiful. This means switching between workspaces is fast (no re-clone, no fresh object fetch), and creating a new workspace on the same repo costs only the cone's worth of disk.

> **Adoption guidance** — For repos under ~1 GB, default to full clone. For repos 1–10 GB, default to blobless with full files on disk. For repos over 10 GB, default to blobless + sparse with cones picked per workspace. Concerto warns the user before creating a non-sparse workspace on a >10 GB repo.

### 9.8 Sparse-checkout learning mode (V2)

A background mode that records which files the agent actually reads and writes over time, then suggests refinements to the sparse cones ("you're touching libs/notifications/ but it's not in your cone — add it?"). This is V2 — V1 ships with manual cone selection plus the plan-mode auto-suggest.

---

## 10. Multi-repo workspaces and multi-PR sessions

Most workspace orchestrators map one workspace to one repository. Real product changes routinely span two or more — an API change in a server repo plus a client change in a mobile repo, or a shared-library bump that ripples to five consumers. Doing that today means manually creating N workspaces and copy-pasting context between them. Concerto raises it to a first-class concept: a session can be scoped to multiple repositories with linked branches and a linked PR set.

### 10.1 Model: session as the higher-level container

| Concept | In Concerto |
|---|---|
| Repository | Multiple repos can belong to one Concerto Project (a "monorepo group") |
| Workspace | One repo, one branch, one PR — still the unit of isolation |
| Session | Explicit. A session can hold N linked workspaces across N repos |
| PR | A session can drive a "PR set" (N PRs that ship together) |

A Concerto Project may contain one or many Repositories. A Session within that Project may span any subset of those repositories. The Session is the unit of "what work am I doing right now"; the Workspaces under it are the units of "where on disk and on which branch."

### 10.2 UI for a multi-repo session

```
Session · "add idempotency keys to payment endpoints"
┌──────────────────────────────────────────────────────────────────────┐
│  Repos in this session: 3                              [+ Add repo]  │
│                                                                      │
│  ▾ marketplace-api · feat/idempotency-keys             ● running     │
│       2 commits ahead · 14 files changed · PR #4821 (draft)          │
│  ▾ marketplace-android · feat/idempotency-headers      ◐ blocked     │
│       1 commit ahead · 3 files changed · PR #1207 (draft)            │
│  ▾ marketplace-ios · feat/idempotency-headers          ✓ done        │
│       2 commits ahead · 5 files changed · PR #882 (ready)            │
│                                                                      │
│  ─────────────────────────────────────────────────────────────────   │
│   Shared chat (cross-repo) · 14 turns                                │
│   ┌────────────────────────────────────────────────────────────┐    │
│   │  Agent (Claude 4.7):                                       │    │
│   │  I added the X-Idempotency-Key header in both client libs  │    │
│   │  and the server validator. Server changes are in           │    │
│   │  services/payments/api.py:42. Client changes are in        │    │
│   │  android/lib/HttpClient.kt:118 and ios/Networking/         │    │
│   │  HttpClient.swift:204.                                     │    │
│   │  Should I add integration tests?                           │    │
│   └────────────────────────────────────────────────────────────┘    │
│                                                                      │
│  PR set view  |  Per-repo view  |  Combined diff                     │
└──────────────────────────────────────────────────────────────────────┘
```

### 10.3 How the agent sees a multi-repo session

The agent runs in one workspace at a time but is aware of sibling workspaces. The Core injects a synthetic ROOT-level instruction at session start that lists the other repos and their paths, lets the agent jump between them with a `/switch-repo <name>` slash command, and exposes a `concerto_link_pr` tool the agent can call to declare "PR #X in repo A is linked to PR #Y in repo B."

### 10.4 PR set semantics

A PR set is a named group of pull requests that should land together. Concerto tracks the set's state:

- **All-green** — every PR's checks are passing.
- **Ready** — every PR is approved.
- **Merge ordering** — Concerto can detect dependencies between PRs (PR A requires PR B because A imports a symbol B exports) and propose a merge order.
- **Coordinated merge** — click "Merge set" and Concerto merges in the proposed order, waiting for each PR's post-merge CI to pass before merging the next.
- **Coordinated revert** — if a post-merge canary fires, one click reverts all PRs in the set.

### 10.5 Cross-repo conflict detection

When the agent is about to commit in a multi-repo session, Concerto runs a cross-repo coherence check: does the API contract change in repo A still type-check in repo B's client code? This requires the agent to declare contracts (in V1, manually flagged "shared types" files; in V2, learned). If a coherence check fails, the agent is asked to fix it before the session can proceed.

### 10.6 What stays per-workspace

- The sparse cone (each repo has its own cone).
- The branch and the worktree.
- The setup / run / archive scripts (these are per-repo, not per-session).
- The local commit history.

### 10.7 What is session-level

- The chat history (shared across all workspaces in the session).
- The PR set.
- The session's instruction file (analogous to CLAUDE.md but session-scoped).
- The merge plan.

---

## 11. Skill Explorer

Claude Code Skills (the open Agent Skills standard) are a folder-based packaging format: a `SKILL.md` plus optional scripts, references, and templates. Skills are discovered by Claude Code at four scopes — enterprise (managed), personal (`~/.claude/skills/`), project (`.claude/skills/`), and plugin (`<plugin>/skills/`). Codex consumes the same format. Today most developers use skills entirely through the filesystem, with no UI.

Concerto adds a dedicated Skill Explorer as a right-rail tab. It is to skills what VS Code's Extensions panel is to extensions.

### 11.1 What the Skill Explorer does

- **Browse installed skills** across all four scopes, grouped by scope, with the skill's description and the path it was discovered at.
- **Search and install from public marketplaces** — Anthropic's skills repo, the Antigravity Awesome Skills library, alirezarezvani/claude-skills, and any user-added Git URL that follows the marketplace format.
- **Per-project scope toggling** — turn an installed skill on or off for one specific project without uninstalling.
- **Visibility overrides** — the four states from Claude Code's `skillOverrides` setting (on, name-only, user-invocable-only, off) exposed as toggles.
- **Invocation control** — the `disable-model-invocation` and `user-invocable` frontmatter fields surfaced as checkboxes (with the underlying SKILL.md editable when needed).
- **Test a skill** — a "Try this skill" button that opens a sandboxed chat with the skill pre-invoked, against an example workspace.
- **See last invocation** — for each skill, when was it last used in this project, by which agent, and what did it return.

### 11.2 UI

```
Skill Explorer
┌──────────────────────────────────────────────────────────────────────┐
│  Search ____________________   [ Personal | Project | Marketplaces ] │
│                                                                      │
│  Bundled (always available)                                          │
│    /code-review         Reviews staged or PR changes                 │
│    /debug               Step-through debugging with hypothesis test  │
│    /loop                Schedule a recurring prompt (session-scoped) │
│    /verify              Build + launch app to confirm a change       │
│    /run                 Launch and drive the app                     │
│                                                                      │
│  Personal (~/.claude/skills/)                                        │
│    summarize-changes    Summarize uncommitted changes                │
│    pr-summary           Summarize a pull request using gh CLI        │
│    backend-interview-fb Write rigorous interview feedback            │
│                                                                      │
│  Project (.claude/skills/)        scope: this project only           │
│    coupang-api-conventions   REST patterns for Marketplace services  │
│    coupang-test-strategy     What "done" means for tests in this org │
│                                                                      │
│  Marketplaces                                                        │
│    [ Add marketplace from URL ]                                      │
│    ▸ anthropics/skills          54 skills                            │
│    ▸ Antigravity Awesome        1,234 skills · most installed        │
│    ▸ alirezarezvani/claude-     329 skills                           │
│                                                                      │
│  Selected:  pr-summary                                               │
│  Used 4 times in this project.  Last used: 2h ago (bach)            │
│  [✓] Auto-invoke when relevant                                       │
│  [ ] Hide from / menu                                                │
│  [ Edit SKILL.md ]  [ Test in sandbox ]  [ Uninstall ]               │
└──────────────────────────────────────────────────────────────────────┘
```

### 11.3 Slash commands appear here too

Slash commands stored at `.claude/commands/<name>.md` (which Claude Code now treats as skills with default frontmatter) appear in the same explorer alongside skills, since they share the same model.

### 11.4 Marketplace management

A marketplace is a Git URL that points to a directory containing a `marketplace.json` (Claude Code's plugin marketplace format). Concerto can:

1. Add a marketplace by URL.
2. Pin a specific version (commit SHA, tag, or branch).
3. Update marketplaces on a schedule.
4. Show diff between installed and upstream when an update is available.
5. Sign-verify marketplaces (optional, when the marketplace publishes a public key).

### 11.5 Enterprise-managed skills

A platform team can pin a curated set of skills via Concerto's managed settings (`~/.concerto/managed.json`). Those skills:

- Cannot be uninstalled by the user.
- Always appear in the explorer with an "Org" badge.
- Override personal skills with the same name.
- Are versioned with the org's skill repository, and the explorer shows which version is active.

### 11.6 Privacy when browsing marketplaces

Marketplace browsing fetches remote content. When `enterpriseDataPrivacy` is on, marketplace browsing is disabled and only locally-cached or org-managed skills are available. The explorer surfaces this state explicitly so the user understands why the marketplace tab is hidden.

---

## 12. Workflow Explorer (loops and scheduled tasks)

Claude Code now has two scheduling mechanisms:

- **`/loop`** — a session-scoped recurring task. Lives in the current session, expires after 3 days, vanishes when the terminal closes. Implemented under the hood as `CronCreate` / `CronList` / `CronDelete` tools.
- **Scheduled tasks** — Desktop App-level recurring tasks. Survive terminal close and machine restart (when Claude Desktop is running). Each run spawns a fresh Claude instance.

Both are powerful and both are currently invisible until the user actively asks. Concerto exposes them as a Workflow Explorer that lists every loop and scheduled task across all projects and lets the user create, pause, edit, and delete them through the UI.

### 12.1 What the Workflow Explorer shows

```
Workflow Explorer
┌──────────────────────────────────────────────────────────────────────┐
│  [ Active | Paused | History ]              [ + New schedule ]       │
│                                                                      │
│  Today                                                               │
│    08:30  ●  Morning briefing                Daily · Claude 4.7      │
│              Check PRs awaiting my review, CI failures, deploys      │
│              Project: coupang-monorepo       Next: tomorrow 08:30    │
│                                                                      │
│    09:00  ◐  Deployment guardrails          Hourly · Codex GPT-5.5   │
│              Watch staging error rate; ping me if > 0.5%             │
│              Project: marketplace-api        Next: 14:00 (12m)       │
│                                                                      │
│  This week                                                           │
│    Mon-Fri 17:30  Weekday wrap-up           Weekdays · Sonnet 4.6    │
│                   Summarize commits, open PRs, list tomorrow's work  │
│                                                                      │
│  Session-scoped (/loop)                     terminates 2026-05-27    │
│    /loop 15m  in bach  "check subagent task completions"            │
│    /loop 1h   in mozart  "rerun /code-review and post to chat"        │
│                                                                      │
│  History (last 24h)                                                  │
│    07:30  Morning briefing       success    3 PRs awaiting · 1 CI red│
│    06:00  Dependency CVE scan    success    0 new                    │
│    02:00  Tests on dirty branch  failed     1 flake, 0 real failures │
└──────────────────────────────────────────────────────────────────────┘
```

### 12.2 Creating a new scheduled task

The "+ New schedule" button opens a wizard:

1. Name (free text).
2. Prompt (free text, supports skill / slash-command references).
3. Frequency (Manual / Hourly / Daily / Weekdays / Weekly / Custom cron).
4. Model (Opus, Sonnet, Haiku, GPT-5.5, etc., per agent).
5. Project (which project should this run against).
6. Worktree mode (run in the latest worktree, or spin up a fresh isolated worktree per run).
7. Permission mode (autonomous, prompt-on-tool, plan-only).
8. On failure (notify, retry once, retry exponential, ignore).

### 12.3 Distinction between /loop and persistent schedules

The Workflow Explorer surfaces this distinction clearly with separate sections. A loop is "this session, this 3-day window, lightweight"; a scheduled task is "this survives reboots and machine restarts." Concerto can also offer to "promote" a loop to a scheduled task: the user clicks promote and the same prompt is registered persistently. This is the right path for any `/loop` that has proved useful and shouldn't be lost on terminal close.

### 12.4 Practical patterns shipped as examples

On first install, the Workflow Explorer offers a gallery of starter templates:

| Template | Frequency | What it does |
|---|---|---|
| Morning briefing | Daily 08:30 | Open PRs awaiting your review, CI status, overnight deploys |
| Dependency CVE scan | Daily 06:00 | Check for new CVEs in your dependency tree; open patch PRs as drafts |
| Stale PR sweeper | Weekly Mon 09:00 | List PRs over 14 days old; suggest closing or refreshing |
| Flaky test report | Daily 03:00 | Run the flaky-test detector; post results to a Slack thread via MCP |
| Doc drift detector | Weekly Fri 14:00 | Compare README to code; propose updates as a PR |
| On-call digest | Hourly during shift | Summarize new alerts, link incident docs, suggest first-look files |

### 12.5 Notifications from scheduled tasks

Each scheduled task can opt into: silent (results visible in Workflow Explorer history), notify-on-output (push notification when the run produces a chat message), notify-on-action-needed (push only when the agent paused awaiting input). The default is notify-on-action-needed — the same as Anthropic's "Push when Claude decides" model.

### 12.6 Cost guardrails

Scheduled tasks can quietly consume tokens. The Workflow Explorer shows:

- Tokens per run (input/output, last 7 runs).
- Tokens per day, summed across schedules.
- A per-schedule budget cap (e.g., "pause if I cross 1M tokens/day on this schedule").
- Sub-account routing — schedules can be tagged to bill against a separate Anthropic / OpenAI workspace so personal experimentation doesn't cross into work spend.

### 12.7 Cron schedules sync with cloud scheduled tasks when available

When a user is on Claude Pro/Max with cloud-scheduled-tasks enabled, Concerto can register the schedule both locally (on its scheduler) and in the cloud (via `/schedule`). This means a task survives even if the user's Mac is off. Concerto displays which schedules are cloud-backed and warns when a schedule depends on a local-only resource (e.g., reads a file in a path that doesn't exist in the cloud sandbox).

---

## 13. Smart suggestions and best-practice prompts

> **Problem in one line.** Even when the developer knows exactly what they want the agent to do next, typing it (and remembering the right slash command, the right skill name, the right flag) is friction. Sometimes they don't even know what's optimal next — the agent paused, and they have to decide between "run the test," "commit and PR," "ask Claude to refactor," "compact the context," and a dozen other plausible next steps. Concerto can lift most of that decision off the developer's shoulders.

### 13.1 What the feature is

Beneath every workspace chat composer, Concerto shows two to four **suggestion chips**. Each chip is a one-tap action — clicking it sends a prompt (or runs a workflow) without the user typing anything. The chips are context-aware: they change with every agent turn, every checkpoint, every CI event. A typical sequence:

```
Composer:  type a message…    🎙
   ┌──────────────────────────────────────────────────────────────────┐
   │  [ ✓ Approve plan ]  [ Add a test ]  [ Run the test ]  [ More ▾ ]│
   └──────────────────────────────────────────────────────────────────┘
```

Suggestions are not a separate UI surface; they live where the user is already looking. They never autoplay — every chip requires an explicit tap.

### 13.2 Where suggestions come from

Three sources, in priority order:

#### 13.2.1 Agent-state heuristics (deterministic, always-on)

A rule engine inspects the latest agent turn, the workspace state, and recent events. Some examples (these are illustrative, not exhaustive):

| Trigger | Suggestion shown |
|---|---|
| Agent ends a turn with "Should I proceed?" | "Yes, proceed" + "Edit plan" |
| Agent paused awaiting permission for a tool | "Approve" + "Approve once" + "Deny" |
| Tests just failed in the run | "Fix the failing tests" + "Show the failure log" |
| Diff is dirty but no test files changed | "Add a test for this change" |
| PR is open in draft and CI green for 10+ minutes | "Mark ready for review" |
| Reviewer left an unresolved comment | "Address all open comments" |
| Context window > 50% used | "Compact the context" + "Summarize this session so far" |
| Context window > 80% used | "Compact and continue" (now red/urgent styling) |
| Merge conflict appeared on the workspace branch | "Resolve conflicts with main" + "Open conflict resolver" |
| Plan mode is on but the agent has been editing for > 5 turns without committing | "Commit progress so far" + "Save checkpoint" |
| Agent error or crash | "Restart the agent" + "Show last command" |
| No activity for 30+ minutes during a working session | "Resume where I left off" + "Summarize state" |
| Run script is configured but not started | "Start the dev server" |
| Spotlight tests are configured for the changed path | "Run Spotlight tests for this directory" |

The exhaustive rule set lives in a versioned `suggestions.toml` shipped with the Core. It's editable by the user and (for orgs) overridable by managed settings.

#### 13.2.2 Learned suggestions (per-user, per-project)

The Core records, locally:

- Every accepted suggestion (which trigger fired, which chip the user tapped).
- Every typed prompt and the agent state it was typed against.
- Every chip dismissed without acceptance.

Over time it ranks rule-based suggestions by historical acceptance and surfaces user-frequent custom prompts as chips. Example: if the user always types "use kotlinx-serialization, not Jackson" after every "Should I serialize this?" pause in their Android project, after the fifth time Concerto proposes that exact prompt as a chip the next time the same trigger fires in the same project.

Learning is entirely local. No telemetry leaves the machine. No LLM training is done on the user's prompts. The "model" is a simple frequency-and-recency counter per (project × trigger × prompt) tuple, with a small embedding-based similarity check (using a local sentence-encoder, optional) so prompts that mean the same thing but read differently collapse together.

#### 13.2.3 Org-shared best practices (V2)

A platform team can curate a shared `org-suggestions.toml` and push it to Concerto instances via managed settings. Examples a Coupang platform team might ship:

- "On any PR that touches `services/payments/`, suggest 'Add an idempotency key test'."
- "On any workspace using `coupang-sdk` v3.x, suggest 'Upgrade to v4.x'."
- "On any first commit to a `feat/` branch, suggest 'Open a Linear issue for this work'."

Shared suggestions are clearly labelled in the UI with an "Org" badge so the user knows they're not Concerto defaults.

### 13.3 The auto-compact and other safety-net prompts

A specific class of suggestions deserves its own callout: prompts that fire automatically because Concerto has identified a known *anti-pattern* the developer is about to walk into. These are styled differently (subtle warning border, never red unless urgent) and are dismissable:

| Anti-pattern | Auto-suggestion |
|---|---|
| Context window > 50% used | "It might be a good time to compact. Want me to summarize so far and continue?" |
| Context window > 80% used | "Context is nearly full. Compacting now will protect the rest of this session." (chip styled red) |
| Same agent has been in a tight loop for > 20 turns without a checkpoint | "This session has been long. Save a checkpoint before continuing?" |
| File over 1000 lines is being edited inline (vs. patched) | "This is a large file. Consider asking the agent to extract a helper first." |
| Agent's last 5 turns all errored on the same command | "Try a different approach? You can paste a `Stop` then a fresh prompt." |
| Workspace's branch is 50+ commits behind main | "Rebase on main first? Stale branches cause merge conflicts." |
| Two workspaces are editing the same file on different branches | "Check the diffs — there is overlap with the workspace `bach`." |
| Agent suggested running a destructive shell command (`rm -rf`, `DROP TABLE`, `force push`) | "This command is destructive. Review carefully before approving." (this one cannot be auto-accepted; it's an emphasis chip, not a one-tap action) |

These are not unique to Claude or Codex — they are user-side observations about the workspace and conversation. They fire regardless of agent.

### 13.4 How the user controls suggestions

In Settings → Suggestions:

- **Disable suggestions entirely** (some power users will want a clean composer; we respect that).
- **Disable auto-prompts only** (keep regular chips, drop the warning-style ones).
- **Reset learning data** (forget all user-specific frequency counters).
- **Edit the rule set** (open `suggestions.toml` in the user's editor).
- **Allow or disallow org-shared suggestions.**

The `enterpriseDataPrivacy` toggle does **not** disable suggestions — they're local and need no external service. But it does disable learned suggestions if the platform team has flagged them as a data-residency concern (rare, but configurable).

### 13.5 What suggestions deliberately don't do

- **They never auto-execute.** A chip is always one tap.
- **They never modify code directly.** A chip composes a prompt; the agent does the work.
- **They never use a remote LLM call** to rank or generate. (V1.5 may add an optional "ranked-by-Claude" mode for org-shared suggestions.)
- **They never replace the composer.** The text composer is always there; chips are an additive UI affordance.

### 13.6 Suggestions on mobile

On the iOS / Android workspace detail, suggestion chips appear in the same place — above the keyboard, below the chat. Because mobile typing is more expensive, **suggestions are even more valuable on mobile** and we expect the acceptance rate to be higher there. The chip styling on mobile is slightly larger (more touch-friendly) and the "More ▾" overflow opens a vertical sheet rather than a dropdown.

### 13.7 Suggestions and notifications

When an agent pauses awaiting input and Concerto sends a push notification, the notification can include up to three suggestion chips as actionable buttons (iOS supports up to four action buttons per notification; Android similar). A tap on the action button sends the suggestion's prompt directly to the agent without opening the app. This closes the loop on the "I'm on the train, the agent asked a question, I can resolve it without taking the phone off the lock screen" scenario.

---

## 14. Concerto chat — the central maestro

> **Problem in one line.** With five or eight workspaces running, the developer's bottleneck shifts from "writing code" to "remembering what's happening in each workspace and deciding which one to attend to next." Concerto should be an agent that helps the developer drive their workspaces — not just a board that lists them.

### 14.1 The pain it addresses

A senior engineer at 11 AM with six workspaces in flight is performing five jobs at once:

1. **A dispatcher** — deciding which workspace to look at when notifications pile up.
2. **A historian** — re-acquiring context on a workspace they last touched 90 minutes ago.
3. **A code reviewer** — actually looking at the diffs.
4. **A prompt-writer** — composing the next message for each agent.
5. **A planner** — deciding what work to start next.

Jobs 3 and 4 are the work. Jobs 1, 2, and 5 are tax. **The Concerto chat is the feature that absorbs jobs 1, 2, and 5 so the developer can spend their attention on 3 and 4.**

### 14.2 What it is

A single, persistent chat at the top of the Concerto UI, distinct from any workspace chat. Behind the chat is the Maestro agent — a Concerto-managed LLM session (Claude or Codex, user-configurable) running with a limited toolset that lets it inspect workspaces, route prompts, and propose next steps. The user talks to "Concerto" through this chat the same way they talk to the workspace-level agents — natural language, suggestion chips, voice — but the conversation is **about the orchestration layer**, not about any single workspace's code.

### 14.3 Where it lives in the UI

On the desktop, the Concerto chat lives as a persistent bar at the top of every screen (collapsible). Clicking it opens a chat panel that overlays the right two-thirds of the window:

```
┌─────────────────────────────────────────────────────────────────────┐
│  Concerto                                            ▾  collapse     │
│  ────────────────────────────────────────────────────────────────── │
│  ●  6 workspaces · 2 awaiting you · 1 ready to merge                │
│                                                                     │
│  Welcome back. While you were in your meeting:                      │
│   • bach (feat/scroll-btn) finished. All checks green. Ready PR.    │
│   • mozart (fix/NPE) is asking which logger pattern you prefer.     │
│   • chopin (refactor/auth) hit 2 test failures; agent is fixing.    │
│   • grieg (test/flaky) merged at 10:42.                             │
│                                                                     │
│  Suggested next steps:                                              │
│   [ Merge bach's PR ]   [ Answer mozart's question ]   [ Skip ]     │
│                                                                     │
│   you ›                                                             │
│   ┌───────────────────────────────────────────────────────────────┐ │
│   │  ask Concerto something or @workspace to route…   🎙            │ │
│   └───────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────┘
```

On mobile, it's the default screen when the app opens (replacing the Inbox in priority): the user lands in the Concerto chat first, sees the digest, and taps through to any workspace from there.

### 14.4 What the Concerto chat can do

The Concerto agent has a defined toolset, mirroring what a competent assistant would do for a busy engineer:

#### 14.4.1 Route prompts to specific workspaces

The `@workspace` syntax routes a prompt to a workspace's agent. Examples:

- `@bach run the linter` — sends "run the linter" to bach's active agent.
- `@mozart use the ContextualLogger pattern from libs/observability` — answers mozart's pending question.
- `@chopin,@grieg start the dev server` — fans out to both.
- `@all status` — broadcasts a "give me a one-line status" prompt to every active workspace and returns their replies inline.

#### 14.4.2 Cross-workspace queries

Natural-language questions about the state of the entire system:

- "What's blocking each of my workspaces?" → Concerto inspects each, summarizes the blockers.
- "Which workspaces have CI failing?" → list with one-tap jumps.
- "Did any workspace touch `libs/payments-sdk` today?" → cross-workspace search over commits + diffs.
- "Show me everything ready to merge." → filtered list with merge actions.

#### 14.4.3 Resume-from-context reminders

The killer use case. When the user returns to Concerto after a break:

- Concerto generates a 3–5 sentence digest of what changed in every active workspace.
- It highlights what needs the user's attention now versus later.
- It proposes a concrete next action and a chip to take it.

This is what makes a 90-minute meeting tolerable for a developer running six agents: when they sit back down, they don't have to reload state in their head — Concerto tells them.

#### 14.4.4 Spawn new workspaces from natural language

- "Open a workspace to fix the bug from the staging incident at 3 AM" → Concerto reads recent Slack/Linear/incident-tool MCP, infers the issue, opens a workspace from a new branch with a starter prompt.
- "Spin up a workspace for ENG-4827" → reads the Linear issue, creates a workspace, picks sparse cones via plan-mode autosuggest (see §9), pre-fills the chat composer with the issue body.

#### 14.4.5 Hand-off and switch

- "I'm going to lunch — pause everything non-critical" → Concerto identifies workspaces it considers safe to pause and lets the user confirm.
- "Switch to mobile mode" — Concerto reduces notification volume to action-required only, useful when stepping away.

#### 14.4.6 Surface completed work and propose the next thing

When a workspace finishes (whether the user is at the desk or not), Concerto:

1. Sends a notification with one-tap suggestion chips (e.g., "Mark ready for review").
2. Adds a line to the Concerto chat: "bach finished — 14 files, 482 lines added, all checks green."
3. Proposes a next step contextually appropriate for that workspace: "Open the PR for review?" or "Start a follow-up workspace for the integration tests this PR will need?"

### 14.5 What it deliberately doesn't do

- **It does not write code.** The Concerto agent has no edit-file or run-shell tools. It cannot touch a workspace's branch directly. It can only inspect state, send prompts, and create workspaces.
- **It does not replace workspace chats.** All actual code-level conversation still happens inside the workspace where the agent has the working directory and the relevant context loaded.
- **It does not decide for the user.** Every action it proposes is a chip the user must tap. The auto-approval principle from §4.7 ("the dashboard never lies") and §13.5 (suggestion chips never auto-execute) applies here too.
- **It does not have full chat content from every workspace.** By default it has summaries (the agent's own end-of-turn summary plus a Concerto-generated rolling digest) — not the raw conversation. This protects token budget and reduces the blast radius of any prompt-injection a workspace agent might be tricked into emitting.
- **It does not run if the user disables it.** Some users will prefer to drive workspaces directly. The Concerto chat is collapsible to a thin status bar on desktop and can be turned off entirely.

### 14.6 How it works under the hood

The Concerto chat is implemented as the Maestro agent — another agent process supervised by the Core, alongside the workspace-level agents. It's just a special workspace with a different toolset and no working directory:

- **Tools available**: `list_workspaces`, `get_workspace_summary`, `route_prompt_to_workspace`, `create_workspace`, `set_workspace_paused`, `read_workspace_checks`, `read_workspace_schedule`, `read_inbox_summary`, `notify_user`. (Roughly fifteen tools total in V1.)
- **No filesystem access**, no shell, no edit tools.
- **Read-only access to workspace state**: status, branch, last commit, summary of agent's last 3 turns, CI / PR / deploy status, todos.
- **Write-side actions are limited**: it can compose a prompt and send it to a workspace, create a new workspace, pause/resume workspaces, schedule a task. Each action surfaces as a chip in the chat for the user to confirm (no silent execution).
- **Memory**: a rolling per-day chat history, a per-week summary, and a permanent "known patterns" log of user-confirmed routings.

### 14.7 Privacy posture for the Concerto agent

This is important enough to spell out. The Concerto agent is a Claude (or Codex) instance running with the user's API credentials. By default:

- It gets **workspace-level summaries**, generated by the workspace agent at the end of each turn, not the full chat.
- It gets **commit messages and PR titles**, not full diffs.
- It gets **CI status names and pass/fail**, not log content.
- The user can opt the Concerto agent into **full chat access** per project ("the Concerto chat needs more context, give it everything for this project"). This is off by default.
- Enterprise data privacy mode disables the Concerto chat's ability to make any external LLM call (i.e., the central chat goes inert unless an on-prem LLM is configured).

### 14.8 Routing notifications through the Concerto chat

Every push notification, every "agent is awaiting input" alert, and every completed-run digest appears in the Concerto chat as a chronological line, with a tap-to-jump action. This makes the Concerto chat double as the **Inbox** — closing two surfaces into one. Users who prefer a strict Inbox view can still find it as a tab; the Concerto chat just becomes the default landing.

### 14.9 What the user types vs. what Concerto reads

A small lexicon to make the chat feel native:

- `@<workspace>` — route the rest of the message to that workspace's chat composer.
- `@all` — broadcast to every active workspace.
- `@idle` — broadcast to workspaces that aren't currently working.
- `@blocked` — broadcast (or address) workspaces awaiting input.
- `/digest` — generate a fresh state-of-the-world summary.
- `/pause` — pause all workspaces.
- `/resume` — resume all paused workspaces.
- `/new <description>` — create a new workspace from a description.
- Anything else without an `@` or `/` is a question or instruction to the Concerto agent itself (e.g., "what should I look at first?").

### 14.10 Mobile considerations

On mobile, the Concerto chat is the **default open screen**. The Workspaces board and Inbox become reachable via tabs at the bottom. This is the inversion of the desktop layout (where the workspace list is the primary surface and Concerto chat is a top bar) — and it's intentional, because on a phone, the user almost never wants to scroll through workspaces themselves. They want Concerto to tell them what's up and what to do, with one-tap chips to act.

Voice input is particularly powerful here. Hold the mic, say "what's bach doing right now," and Concerto reads back a short digest.

### 14.11 Concrete example conversations

These are illustrative of the kind of exchange the Concerto chat enables.

#### 14.11.1 Returning from a meeting

```
you ›  what happened while I was away?

Concerto ›
You stepped away 78 minutes ago. Here's what changed:

• bach (feat/scroll-btn) finished. Diff is +482 / -96 across 14 files.
  All checks green. PR #4821 is in draft.
• mozart (fix/NPE-checkout) paused 6 minutes ago — it needs you to pick
  between a one-off override and patching the codegen. (Same question
  you got on the phone earlier.)
• chopin (refactor/auth) had 2 test failures at 11:14 and fixed them
  by 11:21. No further action needed.
• grieg merged at 10:42 — PR #888.
• gershwin (your android workspace) didn't move. Last activity was you
  starting it before the meeting.

Suggested next:
[ Merge bach ]  [ Answer mozart's question ]  [ Restart gershwin ]  [ Skip ]
```

#### 14.11.2 Routing a prompt

```
you ›  @bach run the e2e suite

Concerto ›
Routing to bach. The agent picked it up.

(One line back from bach:)
bach: starting e2e suite (npm run test:e2e). I'll report when it
finishes — expected ~3 minutes.

[ Open bach ]  [ Dismiss ]
```

#### 14.11.3 Spawning a workspace

```
you ›  open a workspace for the Linear issue ENG-4827

Concerto ›
ENG-4827: "Add idempotency keys to payment endpoints." Touches
services/payments, marketplace-android, and marketplace-ios.

Looks like a multi-repo session. Want me to create one?

Sparse cones I'd pick:
  marketplace-api      services/payments, libs/http-client
  marketplace-android  app/payments, lib/HttpClient.kt
  marketplace-ios      Sources/Payments, Sources/Networking

[ Create session ]  [ Just create marketplace-api ]  [ Edit cones ]
```

#### 14.11.4 Cross-workspace check

```
you ›  did anything touch libs/auth today?

Concerto ›
Two workspaces edited libs/auth in the last 24h:

• chopin — 3 files: TokenStore.ts, OAuthFlow.ts, index.ts (committed 1h ago)
• mozart — 1 file: TokenStore.ts (uncommitted as of right now)

There's overlap on TokenStore.ts. Want me to show a comparison?

[ Compare TokenStore.ts ]  [ Open chopin ]  [ Open mozart ]
```

### 14.12 Cost model

The Concerto agent is a long-running LLM session that uses user tokens. Three guardrails:

- **Quota and budget per day**. Defaults: 200K input / 50K output tokens per day. User-configurable, billed against the same provider (Anthropic, OpenAI, Bedrock) as workspace agents.
- **Idle pruning.** Concerto condenses its own history every N turns to a rolling summary. The chat the user sees is unabridged; the chat the agent reads is summarized.
- **Cheap-model option.** The Concerto agent defaults to Sonnet (or GPT-5-mini equivalent) rather than Opus. Most of its job is summarization and routing — it does not need the most expensive model. Configurable per user.

---

## 15. Mobile and web clients in detail

### 15.1 Mobile information architecture

Two top-level tabs at the bottom of the iOS / Android app: Workspaces (the kanban-style status board) and Inbox (the chronological list of agent messages, action-required items, and completed runs). Settings is a screen reachable from the profile icon top-right.

#### 15.1.1 Workspaces tab

Lists every workspace across every project, with color-coded status dots. Projects can be expanded or collapsed. Pull-to-refresh forces a fresh state pull from the Core. Long-press a workspace to open a context menu: archive, rename branch, open in IDE on desktop (deep link), copy share link.

#### 15.1.2 Inbox tab

A unified chronological feed of:

- Agent messages where the agent paused awaiting input.
- Completed runs (e.g., a `/loop` iteration that produced output worth seeing).
- Status changes that crossed a threshold (CI just turned red, a PR was approved, a deploy failed).
- Schedule outputs the user opted into notifications for.

Each item is one tap deep into the relevant workspace.

#### 15.1.3 Workspace detail

Four tabs at the top of the workspace detail: Chat, Diff, Checks, Terminal.

- **Chat** — the primary surface. Touch-optimized message bubbles, swipe-to-reply, long-press on agent messages to reveal raw tool calls (collapsed by default), voice input button on the composer.
- **Diff** — mobile-native viewer. Red for deletions, green for additions, with a small column number gutter. Swipe between files. Pinch to zoom into a hunk. Long-press a line to comment, which sends the line and a typed/dictated comment back as a composer attachment.
- **Checks** — the same merge-readiness panel as the desktop, condensed for mobile. Each blocker has a single tap action (resolve, send to agent, dismiss).
- **Terminal** — a read-only view of the agent's last terminal output, with a "send command" button that opens a separate full-screen composer. Touch terminal is intentionally restricted; full terminal editing belongs on desktop.

#### 15.1.4 Voice mode

Hold-to-talk on the composer button. Speech transcribes locally where the OS provides it (Apple Speech Recognition on iOS 15+ or Android SpeechRecognizer on Android 11+). The transcribed text appears in the composer and the user can confirm before sending. A separate "Voice conversation" mode (full duplex, agent speaks back via TTS) is a V2 feature; V1 ships dictation-only.

#### 15.1.5 Localhost preview

When a workspace's `npm run dev` (or equivalent) is running, the Core has a port allocated for it. The mobile app opens that port through a secure tunnel that does not require third-party services for the data plane (only the relay's NAT punch). The phone displays the dev server in an in-app WebView with browser-style chrome (back, forward, reload). This is the "is the homepage broken?" check on the commute.

#### 15.1.6 Push notifications

iOS uses APNs, Android uses FCM. Both Apple and Google deliver only the wakeup. The actual notification payload is fetched from the Core directly over the encrypted tunnel after wakeup. This means Apple and Google cannot see what your agent said. The Core encrypts the payload with the device's pairing key; the relay (if used) sees ciphertext only.

### 15.2 Apple Watch (V2)

A small companion app surfacing the same Inbox as the iPhone app, with a one-line status of "N agents working / M waiting on you." Tap a wait-on-you item to open the iPhone app for action. Push notifications are routed to the watch when the phone's screen is locked. This is V2, not V1.

### 15.3 Web client information architecture

Same components as the desktop client, hosted at a URL on the user's Core (e.g., `https://concerto.local.acme.com` when the Core is reachable via mDNS, or via the relay otherwise). Authentication is the same device pairing flow as mobile (QR code scanned once). The web client does not store credentials in browser storage — pairing keys live in indexedDB and are cleared on logout.

### 15.4 Cross-device handoff

Borrowed from Apple's Handoff playbook. When two clients are paired to the same Core and both online:

- A "Continue on…" banner appears on the desktop client when the mobile user has been actively editing a composer message for >5 seconds.
- Tapping it transfers the unsent composer text to the desktop client.
- Same behavior in reverse: an in-progress composer on desktop can be sent to mobile when the desktop is about to sleep.

### 15.5 Bandwidth considerations

Mobile networks are not always great. The Core sends a "lite" stream by default when the client identifies itself as mobile: tool calls collapsed by default, file contents not pre-fetched, syntax highlighting omitted on initial render. When the connection is healthy and on Wi-Fi, the client opts into the "rich" stream. A "data saver" toggle in Settings forces lite mode regardless.

---

## 16. Security and remote transport

> **Threat model** — The user runs Concerto Core on a machine they trust. They want to control it from devices they also trust (their phone, tablet, a borrowed laptop they've paired). They do NOT want any third party — including Concerto's own servers — to be able to read their code, their prompts, or their agent's output. They are willing to tolerate a minimal relay if and only if that relay sees ciphertext only.

### 16.1 Identity, devices, and pairing

The Concerto Core has a long-lived identity: an Ed25519 keypair generated on first launch, with the private key stored in the OS keychain. Each client device (phone, web browser, second laptop) is paired to the Core through a one-time QR code:

1. Open the Core's tray menu and click "Pair new device." It displays a QR code containing the Core's public key and a short-lived pairing token (60-second expiry).
2. On the device, open the app and tap "Pair with my computer." Scan the QR.
3. The device generates its own Ed25519 keypair, sends its public key signed with the pairing token, and gets back a signed device certificate from the Core.
4. From here on, the device authenticates to the Core with its key. The pairing token is destroyed.

No account creation. No email. No third-party sign-in. The Core is the identity provider.

### 16.2 Local-network transport

When the client and the Core are on the same Wi-Fi (or same machine), they connect directly over TLS 1.3. The Core's certificate is the device-paired one; the client pins it. mDNS broadcasts a `_concerto._tcp` service so the client can find the Core without a server hop.

### 16.3 Remote transport — direct first, relay second

When the client is off the local network, two paths are tried in order:

#### 16.3.1 Direct connection via NAT traversal (preferred)

The Core registers with a minimal relay that holds only one piece of state per Core: a current public IP and port. When a remote client wants to connect, it asks the relay for the Core's current endpoint and attempts a direct hole-punched UDP connection (ICE + STUN style). About 70-80% of consumer networks will allow this. Successful direct connections go end-to-end with QUIC + TLS 1.3; the relay never sees the data plane.

#### 16.3.2 Relay fallback (only when NAT traversal fails)

If hole-punching fails (symmetric NAT, restrictive corporate firewalls), the connection falls back to a TURN-like relay over QUIC. The data plane goes through the relay. Because the client and Core hold pairing keys the relay never saw, all relayed traffic is encrypted with a per-session key established via Noise IK protocol. The relay sees only ciphertext and the routing metadata (which Core is talking to which device).

### 16.4 What the relay can do, and what it cannot

| Capability | Can | Cannot |
|---|---|---|
| See your code or prompts |  | ✗ (E2EE) |
| See agent output |  | ✗ (E2EE) |
| See which Core talks to which device | ✓ |  |
| See connection metadata (timestamps, sizes) | ✓ |  |
| Impersonate your Core to a client |  | ✗ (per-Core keypair) |
| Impersonate your client to a Core |  | ✗ (per-device keypair) |
| Trigger push notifications via APNs/FCM | ✓ |  |
| Read push notification bodies |  | ✗ (fetched directly after wakeup) |

### 16.5 Self-hosted relay

The relay is open source and small enough that a platform team can run their own. A Concerto Core can be pointed at a private relay URL in managed settings. This is the path for enterprises that want zero external dependencies.

### 16.6 Push notifications

iOS push goes through APNs; Android through FCM. There is no way to send to a phone without going through Apple or Google. We minimize the leak:

- **Wakeup only.** The push payload contains a notification ID and nothing else. Apple/Google see "Concerto Core has something for device X."
- **Body fetched after wakeup.** The phone wakes up, opens its E2EE channel to the Core, and pulls the actual notification body. Apple/Google never see the body.
- **Opt-out per project.** A project flagged as enterprise-private suppresses notifications entirely and surfaces a generic "you have unread updates" only.

### 16.7 Secrets and tokens

Provider API tokens (Anthropic, OpenAI, GitHub PATs) are stored only in the OS keychain on the machine running the Core. They are never sent to clients. When an agent needs a token, the Core injects it into the agent process's environment and the agent communicates with the provider directly from the Core machine.

### 16.8 Tool approvals from mobile

When an agent requests a tool that requires user approval (file write, shell command, web fetch), the request lands in the Inbox of every paired device. The first device to approve wins; other devices are notified the request is resolved. This solves the "I'm on my phone, the agent wants to run a shell command, I need to see it and approve it in seconds" problem. The mobile approval UI shows the exact command, the working directory, and the agent's reasoning for asking.

### 16.9 Sandboxing the agent

Each agent process runs under the user's UID, in the workspace's working directory, with environment variables limited to what the workspace declares. The Core enforces a per-workspace filesystem allow-list (default: the workspace directory plus the `.context` folder); paths outside trigger an approval prompt. This is not a full sandbox (it is not seccomp/sandbox-exec/AppContainer) but it prevents accidental writes outside the workspace.

### 16.10 Optional Docker isolation

Inspired by Sculptor: an opt-in mode where the agent runs inside a Docker container that mounts only the workspace directory. Pros: dependency installs don't pollute the host; recovery from a misbehaving agent is `docker rm`. Cons: heavier resource usage, slower workspace creation, requires Docker installed. Off by default; surface in repository settings as "Container isolation."

### 16.11 Audit log

The Core writes an append-only audit log to disk for every event that mattered:

- Agent process started or stopped.
- Tool approval granted or denied (by which device).
- PR created, merged, or closed.
- Sparse cone changed.
- Skill installed or uninstalled.
- Schedule created, fired, or deleted.
- Remote device paired or revoked.

The audit log is human-readable JSON Lines. It can be exported. For regulated industries, the Core can be configured to forward audit events to a syslog endpoint or SIEM.

### 16.12 Managed settings (enterprise data privacy)

Concerto's managed-settings format is a `~/.concerto/managed.json` file written by the org, which overrides local settings and disables UI controls for the managed fields. Supported managed fields include:

- `enterpriseDataPrivacy` (disables marketplace browsing, AI-generated titles, third-party MCP).
- `defaultModel`.
- `claudeExecutablePath` / `codexExecutablePath` / `geminiExecutablePath`.
- Allowed and denied skills.
- Allowed and denied MCP servers.
- Allowed remote-pairing devices (whitelist).
- Relay URL (force self-hosted relay).
- Audit forwarding endpoint.
- Maximum number of paired devices per user.

---

## 17. UI screen catalog

This section catalogs the major screens with wireframe-style sketches. **Visual language.** Warm off-white background, restrained accent color (deep blue), small status dots instead of badges, generous whitespace, monospaced sans-serif type for code, plain humanist sans for chrome.

### 17.1 Desktop · Home / project list

```
┌──────────────────────────────────────────────────────────────────────────┐
│  ◐ Concerto                                                       ⌘K   ⚙  │
├──────────────────────────────────────────────────────────────────────────┤
│  Welcome back, Amin                                                      │
│                                                                          │
│  Recently active                                                         │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │ ●  coupang-monorepo                                  3 workspaces  │ │
│  │    Last activity 4m ago · 1 needs your attention                   │ │
│  │    branches:  feat/scroll-btn · fix/NPE · refactor/auth            │ │
│  └────────────────────────────────────────────────────────────────────┘ │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │ ●  marketplace-android                               2 workspaces  │ │
│  │    Last activity 12m ago · all running                             │ │
│  └────────────────────────────────────────────────────────────────────┘ │
│  ┌────────────────────────────────────────────────────────────────────┐ │
│  │ ○  bigbangprice                                      0 workspaces  │ │
│  │    Last activity 3 days ago                                        │ │
│  └────────────────────────────────────────────────────────────────────┘ │
│                                                                          │
│  + New project    Add GitHub repo                                        │
│                                                                          │
│  Today                                                                   │
│   ◐  3 PRs awaiting your review across 2 projects                        │
│   ●  Morning briefing ran successfully at 08:30                          │
│   !   Staging error rate crossed 0.5% (deployment guardrail)             │
└──────────────────────────────────────────────────────────────────────────┘
```

### 17.2 Desktop · Workspace detail (Chat view)

```
┌──────────────────────────────────────────────────────────────────────────┐
│  ◐ Concerto    ●  6 workspaces · 2 awaiting you · 1 ready    ▾ ask Concerto│
├──────────────────────────────────────────────────────────────────────────┤
│  ◐ bach / feat/scroll-to-bottom-btn                             ⌘K   ⚙  │
├──────────────┬───────────────────────────────────────────────────────────┤
│ ▾ coupang    │ bach  · feat/scroll-to-bottom-btn   ▶ Run  ⌘⇧D Diff  ⌘⇧P│
│   chopin  ●  │ Claude 4.7 · plan mode · 3 cones · sparse                 │
│   bach   ◐ │ ──────────────────────────────────────────────────────────│
│   mozart   ◐  ├──────────────────────────────────────────────────┬────────┤
│   grieg    ✓  │  User:  add a "scroll to bottom" button to the   │ Schedl │
│              │         chat                                      │ Skills │
│ ▾ android    │                                                   │ Todos  │
│   gershwin      │  Claude (plan): I'll add a button anchored to the │ Files  │
│              │  bottom-right of the chat panel. It appears only │ MCP    │
│ + new        │  when the user has scrolled away from the bottom │        │
│              │  by > 200px. Tap to scroll smoothly to the latest│ ─────  │
│              │  message. I'll add it in apps/checkout-web/      │ Coun: 8│
│              │  components/Chat.tsx and a CSS module next to it.│ Used:14│
│              │  Should I proceed?                                │        │
│              │                                                   │        │
│              │  Suggestions:                                     │        │
│              │  [ ✓ Approve plan ]  [ Add a test first ]         │        │
│              │  [ Make it iOS-style ]  [ More ▾ ]                │        │
│              │                                                   │        │
│              │  ───────────────────────────────────────────────  │        │
│              │  Composer (⌘↩ to send)                            │        │
│              │  ┌───────────────────────────────────────────────┐│        │
│              │  │  type a message… 🎙                            ││        │
│              │  └───────────────────────────────────────────────┘│        │
├──────────────┴───────────────────────────────────────────────────┴────────┤
│ ● coupang-monorepo synced 2m ago   ●●●● 4 agents · 1 awaiting   Loop·2   │
└──────────────────────────────────────────────────────────────────────────┘
```

The **top bar** is the Concerto chat collapsed — one line showing the global state and a `▾ ask Concerto` affordance to expand the chat panel. Suggestion chips appear above the composer for one-tap actions.

### 17.3 Desktop · Diff viewer

```
┌──────────────────────────────────────────────────────────────────────────┐
│  bach · diff vs main · 14 files changed · 482 +  · 96 -      ⌘⇧P Create │
├─────────────────────────┬────────────────────────────────────────────────┤
│ Files                   │  apps/checkout-web/components/Chat.tsx         │
│ ▾ apps/checkout-web/    │                                                │
│   ▾ components/         │   102  ┃ const Chat = ({ messages }) => {     │
│     ● Chat.tsx +52 -12  │   103  ┃   const [showScroll,                  │
│     ● Chat.module.css   │   104+ ┃     setShowScroll]   <- new state    │
│       +8                │   105+ ┃     = useState(false);                │
│ ▾ libs/auth/            │   106  ┃                                       │
│   ● token.ts   +3 -2    │   107  ┃   const onScroll = (e) => {           │
│ ▾ libs/payments-sdk/    │   108+ ┃     const dist = e.target             │
│   ● client.ts  +94      │   109+ ┃       .scrollHeight - …               │
│                         │   110+ ┃     setShowScroll(dist > 200);        │
│ View:                   │   111  ┃   };                                   │
│  (•) Unified            │   112  ┃                                       │
│  ( ) Split              │   113  ┃   return (                            │
│  ( ) By commit          │   114  ┃     <div onScroll={onScroll}>…       │
│                         │   115+ ┃     {showScroll && (                  │
│ Filter:                 │   116+ ┃       <ScrollToBottomBtn />)}         │
│ ☐ Unrelated edits       │   117  ┃     )}                                │
│ ☐ Generated files       │                                                │
│                         │   ☐ comment on this line                       │
│                         │                                                │
│ [ ▼ Review by Claude ]  │                                                │
└─────────────────────────┴────────────────────────────────────────────────┘
```

### 17.4 Desktop · Checks tab

```
┌──────────────────────────────────────────────────────────────────────────┐
│  bach · Checks                                                          │
├──────────────────────────────────────────────────────────────────────────┤
│  Git status                                                              │
│  ● Clean. No conflicts with main.                                        │
│                                                                          │
│  Pull request #4821                                                      │
│  ● Open · draft · 14 files · +482 / -96                                  │
│  Title: feat(chat): add scroll-to-bottom button                          │
│  Base: main         Head: feat/scroll-to-bottom-btn                      │
│  [ Mark ready for review ]   [ Open on GitHub ↗ ]                        │
│                                                                          │
│  CI checks                                                               │
│  ● lint-frontend          passed   12s                                   │
│  ● test-unit              passed   1m 14s                                │
│  ◐ test-e2e               running  2m 03s                                │
│  ● build-staging-image    passed   4m 22s                                │
│                                                                          │
│  Deployment                                                              │
│  ● staging                deployed by github-actions  21m ago            │
│                                                                          │
│  Review comments                                                         │
│  3 unresolved   1 from @ahmed   2 from CodeRabbit                        │
│   ◐ Chat.tsx:115  consider extracting the threshold to a const           │
│      [ Send to agent ]                                                   │
│                                                                          │
│  Workspace todos                                                         │
│  ✓ Implement onScroll                                                    │
│  ✓ Add ScrollToBottomBtn component                                       │
│  ☐ Add unit test for threshold logic                                     │
│  ☐ Add visual regression snapshot                                        │
│                                                                          │
│  Merge gate:  2 blockers                                                 │
│  [ Merge anyway ]   (disabled — fix blockers first)                      │
└──────────────────────────────────────────────────────────────────────────┘
```

### 17.5 Desktop · Multi-repo session

```
┌──────────────────────────────────────────────────────────────────────────┐
│  Session · Idempotency keys for payments                       PR set ↗  │
├──────────────────────────────────────────────────────────────────────────┤
│  Repos                                                       [ + Add ]   │
│                                                                          │
│  ▾ marketplace-api · feat/idempotency-keys              ● running        │
│     Diff: 14 files · +280 / -42      PR #4821 (draft)                    │
│     CI:   ● lint  ● unit  ◐ e2e                                          │
│  ▾ marketplace-android · feat/idempotency-headers      ◐ blocked         │
│     Diff: 3 files · +47 / -2          PR #1207 (draft)                   │
│     CI:   ● lint  ● unit                                                 │
│  ▾ marketplace-ios · feat/idempotency-headers          ✓ done            │
│     Diff: 5 files · +92 / -3          PR #882  (ready)                   │
│     CI:   ● lint  ● unit  ● ui                                           │
│                                                                          │
│  ── Shared chat (across all 3 repos) ───────────────────────────────     │
│   Claude 4.7 (Plan):                                                     │
│   I've drafted the X-Idempotency-Key header in both clients and a       │
│   matching validator in services/payments/api.py. The Android side is   │
│   blocked: the HttpClient wrapper is generated from a code generator    │
│   that doesn't expose a header customization point. Should I (a)        │
│   patch the generator or (b) add a one-off override in the consumer?    │
│                                                                          │
│   [ Patch the generator ]  [ One-off override ]  [ Plan more ]           │
│  ──────────────────────────────────────────────────────────────────     │
│                                                                          │
│  Merge plan                                                              │
│  Step 1: merge marketplace-api  → wait green CI                          │
│  Step 2: bump SDK version in clients                                     │
│  Step 3: merge marketplace-ios   → wait green CI                         │
│  Step 4: merge marketplace-android → wait green CI                       │
│  [ Merge set ]   [ Edit plan ]   [ Coordinated revert ]                  │
└──────────────────────────────────────────────────────────────────────────┘
```

### 17.6 Mobile · Workspaces list (iOS)

```
╭──────────────────────────╮
│  Concerto          ⌘   ⚙   │
│                          │
│  ●  coupang-monorepo     │
│     4 workspaces  • 1 ▲  │
│                          │
│    ●  chopin             │
│       refactor:auth-flow │
│       Claude 4.7 · plan  │
│                          │
│    ◐  bach    awaiting  │
│       feat:scroll-btn    │
│       1 question for you │
│                          │
│    ●  mozart              │
│       fix:NPE-checkout   │
│                          │
│    ✓  grieg               │
│       test:flaky-fixes   │
│       Ready to merge     │
│                          │
│  +  new workspace        │
│                          │
│  ▾  marketplace-android  │
│  ▾  bigbangprice         │
│                          │
│ ─────────────────────── │
│  Workspaces    Inbox     │
╰──────────────────────────╯
```

### 17.7 Mobile · Workspace detail (Chat) — awaiting input

```
╭──────────────────────────╮
│  ← bach   ●          ⋮  │
│  feat/scroll-btn         │
│                          │
│  Chat  Diff Checks  Term │
│  ────                    │
│                          │
│  Claude 4.7 (plan mode)  │
│                          │
│  ╭──────────────────────╮│
│  │ I'll add a button    ││
│  │ anchored to the      ││
│  │ bottom-right of the  ││
│  │ chat. Visible only   ││
│  │ when the user has    ││
│  │ scrolled away by     ││
│  │ > 200 px. Should I   ││
│  │ proceed?             ││
│  ╰──────────────────────╯│
│                          │
│  [ Approve plan ]        │
│  [ Edit ]   [ Discard ]  │
│                          │
│  ────────────────────── │
│  🎙  type or hold to talk│
│  ┌────────────────────┐  │
│  │                    │  │
│  └────────────────────┘  │
╰──────────────────────────╯
```

### 17.8 Mobile · Diff (touch-optimized)

```
╭──────────────────────────╮
│  ← bach · diff          │
│  14 files · +482 / -96   │
│                          │
│  Chat  Diff Checks  Term │
│        ────              │
│                          │
│  ◀ Chat.tsx ▶  3 / 14    │
│                          │
│   105+ setShowScroll]    │
│   106+   = useState(     │
│   107+     false);       │
│   108                    │
│   109  const onScroll =  │
│   110    (e) => {        │
│   111+    const dist =   │
│   112+      e.target     │
│   113+      .scrollHeight│
│        ⌖ tap a line to   │
│          comment         │
│   114+    setShowScroll  │
│   115+      (dist > 200);│
│                          │
│  ──────────────────────  │
│  jump to:                │
│  ▾ Chat.tsx              │
│    Chat.module.css       │
│    token.ts              │
│    client.ts             │
╰──────────────────────────╯
```

### 17.9 Mobile · Inbox

```
╭──────────────────────────╮
│  Inbox             ⚙     │
│                          │
│  Today                   │
│                          │
│  ◐  bach · awaiting     │
│     Claude is asking 1   │
│     question.            │
│     4m ago               │
│                          │
│  !   marketplace-api     │
│     CI test-e2e failed   │
│     on idempotency tests │
│     12m ago              │
│                          │
│  ✓  grieg · PR #888 ready │
│     All checks green.    │
│     Tap to merge.        │
│     25m ago              │
│                          │
│  Earlier                 │
│                          │
│  ●  morning briefing ran │
│     3 PRs awaiting, 1 CI │
│     08:30                │
│                          │
│ ─────────────────────── │
│  Workspaces    Inbox     │
╰──────────────────────────╯
```

### 17.10 Mobile · Pair a new device

```
╭──────────────────────────╮
│ ← Settings · Devices     │
│                          │
│  This device             │
│  iPhone 17 (Amin)        │
│  paired Apr 12, 2026     │
│  [ Revoke ]              │
│                          │
│  Other paired devices    │
│  •  MacBook Pro 16"      │
│     online · 14 min ago  │
│  •  iPad Pro             │
│     offline · 2 days ago │
│  •  Linux desktop        │
│     online · now         │
│                          │
│  + Pair a new device     │
│                          │
│  Show pairing QR for     │
│  your laptop / phone /   │
│  browser to scan         │
│                          │
│  ╭────────────────╮      │
│  │ ░░  ░  ░░ ░░░ │      │
│  │ ░░  ░░ ░  ░░░ │      │
│  │ ░░░ ░░ ░ ░░░ │      │
│  ╰────────────────╯      │
│  expires in 0:53         │
╰──────────────────────────╯
```

### 17.11 Settings · Repository (sparse cones)

```
┌──────────────────────────────────────────────────────────────────────────┐
│  Settings · coupang-monorepo · Repository                                │
├──────────────────────────────────────────────────────────────────────────┤
│  Clone strategy                                                          │
│  ( ) Full clone                                                          │
│  (•) Blobless clone — defer file contents until needed   recommended     │
│  ( ) Treeless clone — advanced                                           │
│                                                                          │
│  Sparse defaults for new workspaces                                      │
│  ┌──────────────────────────────────────────────────────────────────┐    │
│  │ Default cones (inherited by new workspaces):                     │    │
│  │  apps/checkout-web                                               │    │
│  │  libs/auth                                                       │    │
│  │  libs/payments-sdk                                               │    │
│  │  [ + add ]                                                       │    │
│  └──────────────────────────────────────────────────────────────────┘    │
│  [✓] Auto-suggest cones from issue text when creating workspaces         │
│  [✓] Pre-fetch blobs for sparse cones on idle                            │
│                                                                          │
│  Performance                                                             │
│  [✓] Sparse index                                                        │
│  [✓] Filesystem monitor (core.fsmonitor)                                 │
│  [✓] Untracked cache                                                     │
│  [✓] Run git maintenance weekly in background                            │
│                                                                          │
│  Scripts                                                                 │
│  Setup:    pnpm install --filter ./apps/checkout-web…                   │
│  Run:      pnpm --filter ./apps/checkout-web dev --port $CONCERTO_PORT   │
│  Archive:  docker compose down                                           │
│                                                                          │
│  Run script mode: ( ) Non-concurrent  (•) Concurrent                     │
│                                                                          │
│  Enterprise data privacy [ ☐ ]                                           │
└──────────────────────────────────────────────────────────────────────────┘
```

### 17.12 New workspace dialog

```
┌──────────────────────────────────────────────────────────────────────────┐
│  New workspace                                                           │
├──────────────────────────────────────────────────────────────────────────┤
│  Project:  coupang-monorepo                                              │
│                                                                          │
│  Start from:                                                             │
│  (•) Branch         feat/_____________________                           │
│  ( ) Existing branch   ▾ pick…                                           │
│  ( ) Pull request      ▾ #__                                             │
│  ( ) GitHub issue      ▾ #__                                             │
│  ( ) Linear issue      ▾ ENG-____                                        │
│                                                                          │
│  Sparse cones                                                            │
│  ( ) All files                                                           │
│  (•) Inherit from project defaults  (3 cones)                            │
│  ( ) Pick custom…    →  see tree view                                    │
│  ( ) Suggest from issue text                                             │
│                                                                          │
│  Agent                                                                   │
│  Default agent: Claude Code · Claude 4.7 · plan mode                     │
│  [ Change ]                                                              │
│                                                                          │
│  Auto-run setup script on create: [✓]                                    │
│                                                                          │
│  [ Cancel ]                                          [ Create workspace ]│
└──────────────────────────────────────────────────────────────────────────┘
```

### 17.13 Tray / menu bar

```
╭──────────────────────────────╮
│  ◐ Concerto                   │
│  ● Core running              │
│                              │
│  Pending approvals       1 ▲ │
│   ▶ bach wants to run a     │
│     shell command            │
│                              │
│  Active workspaces       4   │
│   ● chopin   refactor:auth   │
│   ◐ bach    feat:scroll-btn │
│   ● mozart    fix:NPE         │
│   ✓ grieg     test:flaky      │
│                              │
│  Scheduled                  2│
│   ●  Morning briefing 08:30  │
│   ●  Deploy guardrail hourly │
│                              │
│  ───────────                 │
│  Open Concerto                │
│  Pair a new device           │
│  Settings                    │
│  Quit                        │
╰──────────────────────────────╯
```

### 17.14 Desktop · Concerto chat (expanded)

The central maestro chat, expanded over the right two-thirds of the window. Used right after returning to the desk, or whenever the user wants a system-wide view.

```
┌──────────────────────────────────────────────────────────────────────────┐
│  ◐ Concerto    ●  6 workspaces · 2 awaiting you · 1 ready    ▴ close      │
├──────────────────┬───────────────────────────────────────────────────────┤
│  Workspaces      │   Concerto                                             │
│                  │  ────────────────────────────────────────────────     │
│  ● coupang       │   Welcome back. While you were in your 1:1:           │
│   ● chopin       │                                                       │
│   ◐ bach        │   • bach finished. 14 files, +482 / -96, all checks  │
│   ◐ mozart        │     green. PR #4821 is in draft.                      │
│   ✓ grieg         │   • mozart paused 6 min ago — needs you to pick        │
│   ✓ gershwin        │     between one-off override and patching codegen.    │
│                  │   • chopin had 2 test failures at 11:14 and fixed     │
│                  │     them by 11:21.                                    │
│  ● mp-android    │   • grieg merged at 10:42 (PR #888).                   │
│   ● gershwin        │                                                       │
│                  │   Suggested next:                                     │
│                  │   [ Open bach's PR ]   [ Answer mozart ]              │
│                  │   [ Show chopin diff ]  [ Dismiss ]                   │
│                  │                                                       │
│                  │   ──                                                  │
│                  │   you ›   what touched libs/auth today?               │
│                  │                                                       │
│                  │   Concerto ›                                           │
│                  │   Two workspaces edited libs/auth in the last 24h:    │
│                  │   • chopin — 3 files (committed 1h ago)               │
│                  │   • mozart — 1 file (uncommitted)                      │
│                  │   Overlap on TokenStore.ts.                           │
│                  │                                                       │
│                  │   [ Compare TokenStore.ts ]  [ Open chopin ]          │
│                  │                                                       │
│                  │  ───────────────────────────────────────────────────  │
│                  │   ask Concerto or @workspace to route…    🎙           │
│                  │   ┌─────────────────────────────────────────────────┐ │
│                  │   │                                                 │ │
│                  │   └─────────────────────────────────────────────────┘ │
└──────────────────┴───────────────────────────────────────────────────────┘
```

### 17.15 Mobile · Concerto chat (default landing)

On mobile, the Concerto chat is the default screen. Workspaces and Inbox are reachable as bottom tabs but the user almost always lands here first.

```
╭──────────────────────────╮
│  Concerto          ⌘   ⚙   │
│                          │
│  ●  6 wkspaces           │
│  2 awaiting you · 1 ready│
│  ─────────────────────── │
│                          │
│  Welcome back. While you │
│  were in your 1:1:       │
│                          │
│  • bach finished.       │
│    All checks green.     │
│    PR #4821 ready.       │
│                          │
│  • mozart paused. Needs   │
│    you to pick logger    │
│    pattern.              │
│                          │
│  • chopin fixed 2 test   │
│    failures on its own.  │
│                          │
│  Suggested next:         │
│  ╭──────────────────╮    │
│  │ Open bach PR    │    │
│  ╰──────────────────╯    │
│  ╭──────────────────╮    │
│  │ Answer mozart     │    │
│  ╰──────────────────╯    │
│                          │
│ ─────────────────────── │
│  🎙 ask Concerto or       │
│     @workspace to route  │
│                          │
│ ─────────────────────── │
│  Concerto Wkspaces Inbox  │
│  ──────                  │
╰──────────────────────────╯
```

### 17.16 Settings · Suggestions

```
┌──────────────────────────────────────────────────────────────────────────┐
│  Settings · Suggestions                                                  │
├──────────────────────────────────────────────────────────────────────────┤
│  Suggestion chips                                                        │
│  (•) Show beneath the composer (recommended)                             │
│  ( ) Show on the side                                                    │
│  ( ) Disable suggestions                                                 │
│                                                                          │
│  Best-practice prompts (auto-suggestions)                                │
│  [✓] Context window full         > 50%  warn,  > 80%  emphasize         │
│  [✓] Long session without checkpoint                                     │
│  [✓] Tests not modified when code changed                                │
│  [✓] PR stalled in draft > 24h                                           │
│  [✓] Merge conflicts on the workspace branch                             │
│  [✓] Destructive shell command requested                                 │
│  [✓] Branch is 50+ commits behind main                                   │
│                                                                          │
│  Learning                                                                │
│  [✓] Learn from my accepted suggestions (local only, no telemetry)       │
│  [ ] Share learned patterns with my organization (off)                   │
│  [ Reset learning data ]   Total learned patterns: 47                    │
│                                                                          │
│  Organization-shared                                                     │
│  [✓] Allow org-shared best practices  ·  Source: coupang-eng / v3.2     │
│       12 active rules — view, override, or disable per-project below.    │
│                                                                          │
│  Per-project overrides                                                   │
│  ▾ coupang-monorepo   12 rules active, 1 overridden                      │
│  ▾ marketplace-android  9 rules active                                   │
│  ▾ bigbangprice         5 rules active                                   │
│                                                                          │
│  [ Edit suggestions.toml ]                                               │
└──────────────────────────────────────────────────────────────────────────┘
```

### 17.17 Mobile · Workspace detail with suggestion chips on push

When an agent pauses on a phone-locked screen, the push notification includes the top suggestion chips as action buttons so the user can resolve without unlocking.

```
╭──────────────────────────╮
│ 🔒 11:42 AM              │
│                          │
│  ─────────────────────── │
│  Concerto                 │
│  bach is asking         │
│                          │
│  "Should I add a unit    │
│   test for the threshold │
│   logic?"                │
│                          │
│  ╭─────────╮  ╭────────╮ │
│  │   Yes   │  │   No   │ │
│  ╰─────────╯  ╰────────╯ │
│                          │
│  ╭─────────╮  ╭────────╮ │
│  │  Open   │  │ Later  │ │
│  ╰─────────╯  ╰────────╯ │
│  ─────────────────────── │
│                          │
│                          │
╰──────────────────────────╯
```

---

## 18. User journeys

Concrete narratives describing the experience from the user's perspective. These double as acceptance criteria for the design and engineering work.

### 18.1 The morning kickoff

Amin sits down at 8:50 AM. He opens Concerto on his MacBook. The Home view shows him three workspaces are still alive from yesterday — Concerto's Core kept them suspended cleanly across machine sleep. One workspace has a PR ready for self-review.

He starts two new workspaces. The first is from a Linear issue (one click), and Concerto's plan-mode auto-suggest picks the sparse cones based on the issue text. The second is from a free-text prompt: "patch the SDK upgrade across all three marketplace clients." Concerto recognizes this is multi-repo and offers to create a session spanning marketplace-api, marketplace-android, and marketplace-ios.

Both agents start. Amin opens his terminal in Cursor for his own code review, occasionally glancing at Concerto's status bar to see how the agents are doing.

### 18.2 The coffee shop

At 10:30 AM Amin steps out for coffee. His MacBook stays on his desk. The phone shows three green-dot workspaces. Halfway through his coffee, his phone vibrates: bach is awaiting input.

He opens the Concerto iOS app. The Inbox shows the question: "I patched the API but the Android client's codegen breaks because the type changed. Should I (a) patch the codegen, (b) update the schema, or (c) skip Android for now?"

He taps "Patch the codegen" and adds a voice note: "use the existing kotlinx-serialization adapter pattern." The agent resumes. Amin returns to his coffee.

### 18.3 The commute home

At 6:00 PM Amin closes his MacBook lid. The Core continues running in the background as a launchd agent. On the train, he opens the iOS app and reviews the diff for chopin. Two files look fine; the third has a method that calls an old deprecated logger. He taps the line, dictates "switch to the new ContextualLogger pattern from libs/observability." The agent picks up the comment and runs.

At Bay Ridge, he gets a push: PR ready, all checks green. He approves and merges from his phone, walking from the train to the apartment.

### 18.4 The Linux dev box

Saturday morning, Amin works on his personal Ubuntu desktop. He opens Concerto on Linux. Same UI, same projects. He creates a workspace on his BigBangPrice project, which is a small repo and uses full clone. The agent runs. Everything works identically to macOS.

### 18.5 The borrowed laptop

Amin is at a customer site and needs to check on a workspace. The customer's loaner laptop has no software installed and isn't a machine he wants to install on. He opens his Core's URL in Chrome, scans the pairing QR on his phone (the phone authorizes the web session because his phone is already paired), and gets the full Concerto UI in the browser. He never paired the laptop itself; the session ends when he closes the tab.

### 18.6 The platform-team rollout

At Coupang Marketplace, the platform team decides to standardize on Concerto for the 100 engineers on the team. They run Concerto Core on a dedicated Linux box in their VPC. Engineers connect to it from their MacBooks via the desktop app or their phones via mobile. The Core is configured with managed settings that:

- Force `enterpriseDataPrivacy = true`.
- Pin the relay URL to the org's self-hosted relay (so no Anthropic-hosted infra is in path).
- Whitelist three marketplaces of org-approved skills.
- Forward audit events to the org's Splunk.
- Limit each engineer to 4 paired devices.
- Ship a curated `org-suggestions.toml` with 12 Coupang-specific best-practice prompts.

Engineers run their agents on the shared Core (compute is cheaper to pool than to give every engineer a fast Mac). Their MacBooks stay light.

### 18.7 The 90-minute meeting

Amin starts six workspaces at 10:30 AM and then goes into a customer escalation that lasts 90 minutes. He doesn't touch Concerto the whole time. The Core keeps running. Three workspaces finish, one stalls on a question, two are still working.

At 12:05 PM he opens Concerto on his desk. The Concerto chat at the top is glowing. He clicks it and gets a digest in two sentences: "bach and grieg finished and are ready for review; mozart has a pending question about logger choice; chopin and gershwin are still working but on track." Below the digest are four suggestion chips: `Open bach PR`, `Open grieg PR`, `Answer mozart`, `Skip`.

He taps "Answer mozart", a sub-prompt appears asking which pattern, he picks one, and mozart resumes. Two seconds later he taps "Open bach PR" and merges. Total time to re-acquire context: 45 seconds. Before Concerto chat, it would have been a 10-minute click-through-every-workspace exercise to figure out where things stood.

### 18.8 Suggestion chips during a long session

Amin spends Tuesday afternoon in mozart — a fast-moving refactor session with Claude. Around 4 PM the chip area beneath the composer changes:

```
[ ✓ Looks good, continue ]  [ Compact the context first ]  [ Save checkpoint ]
```

He hadn't been thinking about context window usage. Concerto noticed it had crossed 50%. He taps "Save checkpoint" then "Compact the context first" — the agent compacts, the session continues fresh. He'd have forgotten to do this on his own and would have hit a hard cap an hour later.

### 18.9 Routing across workspaces from one chat

Amin notices chopin produced a particularly clean migration pattern for a SQL schema change. He'd like the same pattern applied in mozart and gershwin. Rather than opening each workspace and copying prompts, he types into the Concerto chat:

```
@mozart,@gershwin apply the same migration pattern chopin just used
(see chopin's last commit on the auth schema)
```

Concerto reads chopin's recent commit, generates a per-repo prompt that references the relevant file by path in each target workspace, and routes. Two workspaces start working in parallel without Amin context-switching once.

---

## 19. Implementation notes and technology choices

Recommendations, not mandates. The engineering team has discretion when prototyping reveals better options.

### 19.1 Core daemon

| Concern | Recommendation | Rationale |
|---|---|---|
| Language | Rust | Single static binary on all three desktop platforms; predictable memory; mature git library (gitoxide / libgit2); strong concurrency. |
| Git | gitoxide where complete, libgit2 fallback | gitoxide is pure Rust, async-friendly, and gaining sparse / partial-clone support; libgit2 covers gaps today. |
| DB | SQLite via sqlx | Single file, no external service, perfect for a desktop app. |
| IPC | gRPC over UDS + WebSocket-like streaming via Tonic | Strongly typed schemas, automatic client gen, supports streaming. |
| Remote transport | QUIC via quinn | Faster than WSS over TCP; built-in TLS 1.3; one stack for direct and relayed connections. |
| Crypto | Noise Protocol via snow crate (Noise IK pattern) | Well-audited E2EE pattern; same primitives WireGuard uses. |
| Process supervision | tokio + custom supervisor | Async-first, robust restart semantics. |
| Telemetry | tracing + OpenTelemetry; off by default | Honors the local-first principle; opt-in only. |

### 19.2 Desktop client

| Concern | Recommendation | Notes |
|---|---|---|
| Shell | Tauri 2 (Rust + system webview) | Bundle ~15 MB, low memory; share types/protobuf with Core |
| UI | React + TypeScript + Vite | Same component tree powers the web client |
| State | Zustand or Jotai, not Redux | Lighter, fits small client-side state model |
| Diff renderer | monaco-editor (read-only) with custom diff layer | Battle-tested, supports inline comments |
| Terminal | xterm.js with conpty/pty bridge through Core | Industry standard |

### 19.3 iOS client

| Concern | Recommendation | Notes |
|---|---|---|
| Language | Swift + SwiftUI | Native feel, push integration first-class |
| Networking | URLSession + a Swift QUIC library (or native APIs as they ship) | Bridges to Core over QUIC |
| Persistence | SwiftData | Modern stack; only caches non-sensitive UI state |
| Push | APNs | Wakeup only; bodies fetched from Core |
| Speech | Apple Speech Recognition (on-device on iOS 15+) | Voice input works offline |
| Diff renderer | Custom SwiftUI | monaco is desktop-only; a touch-first diff is a bespoke build |

### 19.4 Android client

| Concern | Recommendation | Notes |
|---|---|---|
| Language | Kotlin + Jetpack Compose | Modern Android stack |
| Networking | OkHttp + Cronet (QUIC support) | Reliable QUIC on Android |
| Push | FCM | Wakeup only |
| Speech | SpeechRecognizer + Whisper.cpp on-device fallback | On-device for privacy; Whisper for languages SpeechRecognizer doesn't cover |

### 19.5 Relay

| Concern | Recommendation | Notes |
|---|---|---|
| Language | Rust | Sharable codebase with Core |
| Hosting (default) | Anthropic-style: a small fleet behind anycast | Low-latency NAT punch; commodity |
| Hosting (enterprise) | Single-binary docker image | Platform teams run their own |
| Data | Stateless except current Core public endpoint per ID | Privacy by minimization |

### 19.6 Build and CI

- Cargo workspace for Rust components; turbo / nx for the TypeScript packages.
- GitHub Actions on every PR with platform-matrix builds (macOS, Windows, Linux, iOS, Android).
- Code-signed and notarized releases on macOS; signed releases on Windows; flatpak / appimage / deb on Linux.
- Auto-update via Sparkle (macOS), Squirrel (Windows), AppImageUpdate (Linux), and App Store / Play Store for mobile.

---

## 20. Phasing and roadmap

Three releases. Each release ships a coherent slice of value, not a feature dump.

### 20.1 V0.1 — Internal alpha (8 weeks)

- Concerto Core on macOS only.
- Desktop app on macOS.
- Workspace, worktree, diff viewer, checkpoints, checks, slash commands, MCP, agent modes.
- Claude Code and Codex integration.
- **Suggestion chips (basic rule set only)** — the deterministic agent-state heuristics from §13.2.1. No learning yet, no org-shared.
- No Concerto chat yet — workspace-only.
- No remote / mobile / web.
- No sparse / blobless yet.
- **Goal:** Pass the dogfood test — a senior engineer can use Concerto for a full week's work, and notices that the suggestion chips already save typing.

### 20.2 V1.0 — Public beta (additional 12 weeks)

- Core on macOS, Windows, Linux.
- Desktop on all three platforms.
- iOS and Android apps with push, voice input, diff viewer, inbox, pairing.
- Web client.
- Remote transport (direct + relay).
- Sparse and blobless clone in repository settings.
- Skill Explorer and Workflow Explorer.
- Multi-repo sessions and PR sets.
- **Concerto chat** — central maestro with routing, digests, suggested next steps, and a defined toolset (§14.6). Default LLM: Claude Sonnet.
- **Suggestion learning** — per-user, per-project frequency-and-recency model running locally.
- **Push notification action buttons** — suggestions surface as actionable items on lock-screen.
- **Goal:** A 100-engineer org can adopt Concerto as their primary tool, and engineers report the Concerto chat as the "feature I didn't know I needed until I had it."

### 20.3 V2.0 — Concerto Cloud, enterprise, polish (additional 16 weeks)

- Self-hosted Core in VPC, accessed remotely by an entire team.
- Managed Concerto Cloud (opt-in hosted execution; explicit pricing tier).
- Apple Watch glance.
- Team-shared sessions (read-only spectate).
- Audit log and SIEM forwarding.
- Sparse-checkout learning mode.
- Voice conversation mode (full duplex).
- Cross-repo coherence checks for contract-level dependencies.
- **Org-shared suggestion rules** with versioned distribution and per-project override UI.
- **Concerto chat advanced toolset** — can read PR comments, run cross-workspace searches over commits, call out to MCP servers for context (e.g. Linear, Slack), spawn parallel multi-repo plans from one prompt.
- **Concerto chat on Apple Watch** — voice-first interaction with one-tap chip actions.
- SOC 2 Type 2.

### 20.4 Sequencing rationale

Multi-repo and sparse-checkout are pulled to V1, not V2, because they are the features that justify switching. Without them, Concerto is just another desktop orchestrator. With them, it is a meaningfully different product across devices and repository shapes. The mobile, web, and remote work are bundled in V1 because they are the user-visible reason a person would switch; they need to ship together.

---

## 21. Success metrics

A small set of metrics, ranked by truthfulness. We want to be honest about which of these proxy for product value and which are easy-to-game vanity numbers.

### 21.1 Activation (week 1)

- % of users who create a workspace within 24 hours of install.
- % of users who pair a mobile device within 7 days.
- % of users who create a multi-repo session within 30 days (only at orgs with >1 repo).

### 21.2 Engagement (after activation)

- Median number of workspaces active at any point during a working day.
- Median number of agent runs per user per week.
- % of agent runs where the user took action from mobile (this is the key proof that the mobile surface is real value, not novelty).
- % of scheduled tasks that the user keeps active for >30 days (proxy for "this thing is useful, not a toy").
- **% of prompts sent via a suggestion chip vs. typed** (target: > 30% after 30 days of use — chips are pulling weight).
- **% of working days where the user opens the Concerto chat at least once** (target: > 70% after activation — proxy for "the maestro is part of the daily workflow").
- **Time-to-first-action after returning to the app** following a > 30-minute absence (target: < 60 seconds median, with the Concerto chat digest doing the work).
- **% of "auto-compact" suggestions accepted** when they fire (target: > 50% — if it's below that, the heuristic is firing too often).

### 21.3 Performance (technical health)

- p50 workspace creation time on a 40 GB monorepo (target: < 30 seconds with sparse + blobless).
- p50 round-trip from mobile to Core for a chat message (target: < 250 ms on a healthy LTE connection).
- p50 Concerto chat digest generation time after a > 30-minute absence (target: < 5 seconds).
- Crash-free session % (Core daemon).
- % of remote connections that go direct (vs. fall back to relay). Target: > 70%.

### 21.4 Trust

- Number of unique users who turn on `enterpriseDataPrivacy`.
- Number of customer-paid security audits passed.
- Time-to-revoke for a stolen-phone scenario (target: < 60 seconds).

### 21.5 Anti-metrics (things we deliberately do not optimize)

- Time-in-app. We are explicitly trying to reduce this.
- Notification volume.
- % of edits typed on mobile. Typing on mobile is a failure mode of desktop access, not a success.

---

## 22. Open questions and risks

### 22.1 Naming

"Concerto" was chosen as the project name because it captures the product's defining metaphor: a concerto is a piece for a soloist *and* an orchestra — exactly the dynamic between a developer and their fleet of AI agents. The developer is the soloist; the agents are the orchestra; the central maestro chat conducts. Trademark clearance is still pending — the word appears in several adjacent industries (financial software, hospitality) but no direct collision in developer tooling has surfaced. Final clearance and domain acquisition should happen before public beta.

### 22.2 Anthropic's Remote Control

Anthropic shipped Remote Control in early 2026 for free with Pro/Max. It is the official, "in the Claude app" path. Our differentiation is that we orchestrate, where Remote Control just mirrors. But the bar is now that we have to be obviously better at orchestration than a generic remote terminal, which is a higher bar than it was a year ago.

### 22.3 Will engineers actually pair a phone to a work laptop?

In enterprise environments, pairing a personal device to a work machine can violate device-management policies. Mitigation: design pairing such that the device certificate can be issued by an org-managed CA, so the org can control which devices can pair without losing the E2EE properties.

### 22.4 Sparse-checkout discoverability

Sparse-checkout is a feature most developers do not know exists. Even surfacing it as a setting will leave many users on full clone simply because they don't know to choose otherwise. Mitigation: Concerto detects repo size at clone time and proactively recommends sparse + blobless for repos over 10 GB. The default for new projects on large repos is sparse, not full.

### 22.5 The relay business model

Running the relay costs money. Possible models: free for personal use up to a small bandwidth cap; paid tier for individuals who go over; flat per-seat fee for organizations who want SLA-backed relay; free for self-hosted relays. Decision deferred to V2.

### 22.6 Open source posture

Strong reasons to open-source the Core: it builds trust ("we can't exfiltrate your code, we promise — and you can read the code to prove it"); platform teams expect to read and audit anything that runs in their infra. Strong reasons to keep clients closed-source initially: clients are where the polish lives, and polish is a competitive moat. Likely answer: open-source the Core under Apache 2.0 at V1; keep clients dual-licensed under a source-available license that allows non-commercial fork but reserves commercial rights.

### 22.7 Pricing

Concerto is free for individuals using their own model subscriptions, to lower switching cost. Revenue comes from: enterprise self-hosted (per-seat license), Concerto Cloud (managed execution tier), and possibly paid relay for individuals who exceed the free tier. No ads. No data sale.

### 22.8 Risks we are knowingly accepting

- Building three platforms (desktop, mobile, web) plus a server is a lot of surface area for a small team. We accept this is the cost of meeting the bar we've set for ourselves.
- Sparse-checkout has rough edges in some workflows (large vendored binaries, generated files). We accept that V1 will need clear documentation about which patterns work and which need full clone.
- Mobile UX for code is fundamentally limited. We accept that the phone is not a place to write code, only to steer agents that write code.

### 22.9 What we are not yet sure about

- Whether to ship Linux desktop alongside macOS/Windows in V1, or push it to V1.5. Leaning toward shipping all three at V1 because the Core is already cross-platform, so it is mostly a UI port.
- Whether to invest in Xcode integration ahead of JetBrains. iOS users skew toward Xcode; the rest skew toward JetBrains.
- Whether the Workflow Explorer should also surface non-AI cron jobs (system cron, GitHub Actions schedules) for a unified scheduling view, or stay AI-only.

### 22.10 Concerto chat token cost and model selection

The Concerto chat runs a long-lived LLM session that consumes tokens even when the user isn't actively asking questions (because it digests workspace state in the background). Open questions:

- Is the cost acceptable when defaulted to Sonnet, or should the default be Haiku and only escalate to Sonnet/Opus on user-typed messages?
- Should the Concerto agent share an Anthropic / OpenAI account with workspace agents, or have its own budget bucket so a runaway summarizer doesn't burn workspace quota?
- For enterprises with on-prem LLMs (Bedrock, Vertex, custom), do we ship a "Concerto chat works with any model" abstraction, or pin it to Anthropic and require Bedrock/Vertex configuration to route there?

Default for V1.0: Sonnet, shared account, configurable model per user. Revisit after 30 days of beta data.

### 22.11 Suggestion-chip false-positive rate

Auto-firing best-practice prompts is great when they're relevant and terrible when they're spammy. Open questions:

- What's the right threshold for the "context > 50%" trigger? (50% may be too eager for long-context models; we may want to tune to 60–70%.)
- Should the system learn per-user to suppress chips that user repeatedly ignores? (Yes, almost certainly — but we need to define how aggressively.)
- Are the org-shared suggestions a place users will resent ("the platform team is telling me what to do") or appreciate ("this captures hard-won team wisdom")? Depends entirely on how the platform team curates them.

V1 ships with conservative thresholds. We expect to retune based on beta data, possibly to per-user adaptive thresholds.

### 22.12 Privacy of the Concerto chat agent

The Concerto chat agent reads workspace summaries by default. Some users may consider even the summaries sensitive — for example, if a workspace is doing a security investigation, the summary itself could leak the investigation. Mitigation options:

- A per-workspace "exclude from Concerto chat" toggle. Disabled workspaces are listed by name only; no summary, no diff, no last-turn.
- A "private workspaces" project tier that's invisible to the Concerto chat entirely.

V1.0 includes the per-workspace toggle. V1.5 may add the private-project tier if enterprise pilots ask for it.

---

## 23. Appendix · Glossary

| Term | Meaning |
|---|---|
| Core | The Concerto server daemon. One per user per machine. Holds canonical state. |
| Client | A desktop, mobile, or web UI for the Core. Stateless renderer. |
| Project | A grouping of one or more repositories under a single Concerto identity. |
| Workspace | An isolated copy of a repository on its own branch, mapped to one shippable unit of work. |
| Session | A grouping of one or more workspaces (possibly across repositories) doing one piece of work. |
| PR set | A linked group of pull requests that should land together. |
| Cone | A directory listed in a sparse-checkout configuration, defining which files are materialized. |
| Worktree | A git working tree backed by a shared object database. Concerto's isolation primitive. |
| Blobless clone | A partial clone that defers file contents until needed; reduces initial clone size dramatically. |
| Sparse index | A git index that only contains entries for files inside the sparse cones. |
| Skill | A folder containing a SKILL.md and optional supporting files; extends what an agent can do. |
| Loop | A session-scoped recurring task created by `/loop`. Expires when the session ends or after 3 days. |
| Scheduled task | A persistent recurring task that survives session and machine restarts. |
| Relay | A minimal Concerto-operated server used for NAT traversal and push notification delivery; sees ciphertext only. |
| Pairing | The one-time QR-code exchange that establishes an E2EE channel between the Core and a client device. |
| Managed settings | A JSON file written by an organization that locks specific Concerto settings for an engineer. |
| Concerto Cloud | A future hosted-execution tier where agents run on Concerto-operated infrastructure (V2). |
| Suggestion chip | A one-tap button beneath a chat composer that sends a pre-composed prompt to the agent. Driven by agent state, learned from user behavior, or contributed by org-shared best practices. |
| Best-practice prompt | An auto-generated suggestion chip that fires when Concerto detects a known anti-pattern (e.g. context full, branch stale, destructive command). Always one-tap, never auto-executed. |
| Concerto chat | The central maestro chat at the top of the app, distinct from any workspace chat. Routes prompts, summarizes state, proposes next steps. Backed by its own LLM session with read-only access to workspace summaries. |
| Maestro agent | The LLM session behind the Concerto chat. Has tools for `route_prompt_to_workspace`, `list_workspaces`, `create_workspace`, etc. — but no shell, no file edits, no direct code access. |
| `@workspace` routing | Syntax in the Concerto chat to address a specific workspace's agent (e.g. `@bach run the linter`). Also `@all`, `@idle`, `@blocked`. |
| Digest | A short Concerto-generated summary of what every active workspace did in a given time window. Shown when the user returns to the app after being away. |

---

*End of document. Draft for internal review · 2026*
