# Task 51 — README + Getting-Started Documentation

| Field | Value |
|---|---|
| Phase | 4 |
| Size | small (≤4h) |
| Depends on | 49 |
| Touches subsystem(s) | 18 (Distribution) |
| Smoke gate | unchanged |

## Goal
Write the user-facing `README.md` at the repo root and a short `docs/getting-started.md` walking a new developer from clone-to-running-agent in under 10 minutes. After this task, anyone landing on the repo can understand what Concerto is and have it running.

## Inputs to read before starting
- `design/00_Architecture_Overview.md` §1–§4 (system at a glance — basis for the README's "what is this").
- `design/Concerto_PRD.md` §1–§3 (positioning, audience — keep accurate).
- `design/18_Distribution_and_Operations.md` (skim — licensing posture, contribution model, trademark).
- `LICENSE`, `CONTRIBUTING.md`, `SECURITY.md`, `TRADEMARKS.md` (already in repo).

## Scope — in
- `README.md` at the repo root:
  - Short tagline (one sentence) — derive from PRD.
  - 5-bullet feature list (what's in V0.1).
  - Install snippet for macOS (uses `scripts/install-macos.sh` from Task 49).
  - "Run your first agent" 3-step walkthrough.
  - Architecture overview link (`design/00_Architecture_Overview.md`).
  - License + status (alpha) + how to contribute (link to `CONTRIBUTING.md`).
  - Badges: CI status, license, version (V0.1 = `0.0.1` after Task 53).
- `docs/getting-started.md`:
  - **Prerequisites**: macOS 13+, Rust 1.78+, Node 20+, pnpm, `gh` CLI, `claude` CLI (Claude Code) authenticated.
  - **Install** section: clone, `scripts/install-macos.sh`, verify with `launchctl print`.
  - **First workspace**:
    1. Install + start Core.
    2. Start Desktop: `cd apps/desktop && pnpm install && pnpm tauri dev`.
    3. Add a repository (point to a public GitHub URL).
    4. Create a workspace + workarea.
    5. Start a Claude session; have a 3-message conversation.
  - **Troubleshooting**: common issues (Core not running, `gh` not authenticated, `claude` missing).
  - **Where things live**: `~/concerto/`, `~/.concerto/`.
- Update repo-root README with project status — V0.1 is **alpha, macOS only, single-repo workspaces**.
- Add a `CHANGELOG.md` at the repo root with an initial `## 0.0.1 — V0.1 alpha` entry summarizing the V0.1 feature set (cross-reference the task list).
- Update `CONTRIBUTING.md` only if needed to mention the task-based development workflow under `tasks/`.

## Scope — out
- Per-subsystem user docs (V1.0).
- Video walkthroughs (V1.5+).
- Marketing landing page (Concerto Inc operates separately per `design/00 §6.11`).

## Public interface this task locks
- README structure: tagline, features, install, walkthrough, architecture link, license, contribute.
- `docs/getting-started.md` path. Frozen.
- `CHANGELOG.md` at repo root. Frozen.

## Implementation notes
- Don't over-promise: V0.1 is alpha. Use phrases like "early alpha — expect rough edges."
- Link to the design docs liberally — they're the source of truth.
- The CI badge URL depends on GitHub Actions workflow names from Task 02; verify they match.

## Verification
1. `markdownlint README.md docs/getting-started.md CHANGELOG.md` → clean (or document accepted exceptions).
2. Manual: follow `docs/getting-started.md` on a fresh Mac; reach a working Claude session in < 10 minutes.
3. Every CLI command in the docs is copy-pasteable and works (one bad command = failure).
4. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] Fresh-Mac walkthrough verified by following the doc.
- [ ] No outdated paths or commands.
- [ ] `CHANGELOG.md` lists V0.1 features clearly.
- [ ] No `TODO` / `FIXME` in docs.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `README.md` (new)
- `docs/getting-started.md` (new)
- `CHANGELOG.md` (new)

## Commit message
```
phase-4: README + getting-started

Project README, ~10-minute developer walkthrough from clone to
running Claude session. CHANGELOG.md kicks off with V0.1 alpha.

Refs: tasks/51-readme-and-getting-started.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** per-subsystem user docs deferred to V1.0.
- **Smoke-gate state:** unchanged.
