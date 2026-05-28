# Task 27 — Smoke Gate v2 (End-to-End Agent Spawn)

| Field | Value |
|---|---|
| Phase | 2 |
| Size | medium (1–3d) |
| Depends on | 18, 19, 20, 22, 23 |
| Touches subsystem(s) | 01 (Runtime), 04 (Agent Supervisor), 10 (Local API) |
| Smoke gate | v2 |

## Goal
Extend `scripts/smoke.sh` to a full end-to-end Phase 2 check: boot Core → create project (direct SQL) → add a local bare-repo + clone → create workspace → create workarea → spawn an `echo` agent session → receive its output via `Streams.Subscribe(session.io.<sid>)` → stop session → clean shutdown of Core. Also verifies that the workarea worktree on disk is well-formed.

## Inputs to read before starting
- `tasks/15-smoke-gate-v1.md` (current state of `scripts/smoke.sh`).
- `tasks/26-desktop-session-terminal.md` → "Handoff Notes" (confirms Phase 2 is otherwise complete).

## Scope — in
- Extend `tools/smoke-client/` to support multiple subcommands (use `clap` with subcommands):
  - `caps` — calls `GetServerCapabilities` (existing).
  - `add-project --name <s>` — direct SQL insert via a sidechannel? **NO** — V0.1 still has no `Projects` service, so the smoke client uses `sqlx` to insert directly into the DB at `$CONCERTO_DB_PATH`. Document this as the V0.1 workaround.
  - `add-repo --project-id <id> --url <url>` — calls `Repositories.AddRepository`.
  - `clone --repo-id <id>` — calls `Repositories.Clone` and consumes the stream until done.
  - `new-workspace --project-id <id> --name <s> --repo-id <id>` — calls `Workspaces.CreateWorkspace`.
  - `new-workarea --workspace-id <id>` — calls `Workareas.CreateWorkarea`.
  - `start-session --workarea-id <id> --agent-kind echo` — calls `Sessions.CreateSession`.
  - `stream-session-io --session-id <id> --timeout 10` — subscribes to `session.io.<sid>`, prints chunks to stdout, exits when `AgentExited` arrives or timeout.
  - `stop-session --session-id <id>` — calls `Sessions.StopSession`.
- Update `scripts/smoke.sh` Phase 2 block:
  ```sh
  echo "Smoke gate v2: creating bare test repo..."
  BARE="$CONCERTO_HOME/bare-repo.git"
  mkdir -p "$BARE"
  git init --bare --quiet "$BARE"
  git -C "$BARE" symbolic-ref HEAD refs/heads/main
  
  # Push an initial commit via a temp clone
  TMP="$CONCERTO_HOME/seed"
  git clone --quiet "$BARE" "$TMP"
  echo "# smoke test" > "$TMP/README.md"
  git -C "$TMP" add -A
  git -C "$TMP" -c user.email=smoke@test -c user.name=Smoke commit -m "seed" --quiet
  git -C "$TMP" push --quiet origin main
  
  PROJECT_ID=$(cargo run --quiet --bin smoke-client -- add-project --name "smoke")
  REPO_ID=$(cargo run --quiet --bin smoke-client -- add-repo --project-id "$PROJECT_ID" --url "file://$BARE")
  cargo run --quiet --bin smoke-client -- clone --repo-id "$REPO_ID" || fail "clone"
  WS_ID=$(cargo run --quiet --bin smoke-client -- new-workspace --project-id "$PROJECT_ID" --name "wsp" --repo-id "$REPO_ID")
  WA_ID=$(cargo run --quiet --bin smoke-client -- new-workarea --workspace-id "$WS_ID")
  
  # Verify worktree on disk
  WT_ROOT="$CORE_DATA_DIR/workspaces/wsp/$(ls "$CORE_DATA_DIR/workspaces/wsp" | head -1)"
  [ -d "$WT_ROOT/.context" ] || fail ".context/ missing"
  [ -d "$WT_ROOT"/*/.git ] || fail "repo .git missing in worktree"
  
  # Spawn echo session and verify output
  SID=$(cargo run --quiet --bin smoke-client -- start-session --workarea-id "$WA_ID" --agent-kind echo)
  cargo run --quiet --bin smoke-client -- stream-session-io --session-id "$SID" --timeout 10 > "$CONCERTO_HOME/session-out.log"
  grep -q . "$CONCERTO_HOME/session-out.log" || fail "no session output captured"
  
  echo "Smoke gate v2: PASSED"
  ```
- The smoke script's existing Phase 1 checks remain at the top.

## Scope — out
- Desktop in the smoke gate (still GUI-blocked).
- Workspaces with multi-repo (V1.0).
- Tool approvals (Phase 3 — smoke gate updates in Task 42 or 44).
- PR set / VCS smoke (Phase 3 — Task 45).

## Public interface this task locks
- Smoke gate version `v2` means: full Phase 2 happy path executes end-to-end via gRPC.
- `tools/smoke-client/` becomes the canonical subcommand-based test client.

## Implementation notes
- The smoke client's `add-project` subcommand uses `sqlx` directly. This is a documented V0.1 workaround because no `Projects` gRPC service exists. (Task 24 may have added a `Projects.ListProjects` RPC; if it added `Projects.CreateProject`, prefer that.) Reflect the choice in Handoff Notes.
- All gRPC calls should have explicit 30s deadlines so the script fails fast if the Core misbehaves.
- The `stream-session-io` subcommand exits 0 on success; if the timeout fires before `AgentExited`, exit 1 (the agent should finish in well under 10s for `echo`).
- Make sure to remove the agent socket and any test artifacts on cleanup (the existing `trap` in `scripts/smoke.sh` handles `$CONCERTO_HOME` removal).

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo clippy --workspace -- -D warnings` → clean.
3. `scripts/smoke.sh` locally → exits 0, prints "Smoke gate v2: PASSED" within ~60 seconds.
4. CI runs the smoke workflow green.
5. Force-failure check: break `Sessions.CreateSession` temporarily; verify the smoke script fails with a clear message; revert.
6. Force-failure check: simulate a hung session (extend echo to a sleep); verify the 10s timeout in `stream-session-io` kicks in.
7. `shellcheck scripts/smoke.sh` → clean.
8. `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → no unintended drift.

## Definition of Done
- [ ] Verification commands pass.
- [ ] Smoke gate v2 green locally and in CI.
- [ ] On-disk worktree structure verified by the script.
- [ ] All force-failure cases verified.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Single commit created.

## Outputs
- `tools/smoke-client/Cargo.toml` (modified — clap, sqlx)
- `tools/smoke-client/src/main.rs` (modified — subcommand dispatch)
- `tools/smoke-client/src/cmd/*.rs` (new — one file per subcommand)
- `scripts/smoke.sh` (modified — Phase 2 block)

## Commit message
```
phase-2: smoke gate v2 — end-to-end agent spawn

scripts/smoke.sh now creates a project, clones a bare local repo,
creates a workspace + workarea, spawns an echo session via
Sessions.CreateSession, and verifies output arrives via
Streams.Subscribe. Worktree on-disk structure verified.

Refs: tasks/27-smoke-gate-v2.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** smoke uses sqlx to insert a project (no Projects.Create RPC in V0.1).
- **Smoke-gate state:** **v2 active.** Covers: Phase 1 + project/repo/workspace/workarea creation + agent spawn + stream + stop + cleanup.
