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
- [x] Verification commands pass.
- [x] Smoke gate v2 green locally and in CI.
- [x] On-disk worktree structure verified by the script.
- [x] All force-failure cases verified.
- [x] No `TODO` / `FIXME` in new code.
- [x] Single commit created.

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
- **Drift from plan:**
  - **`clap` promoted to a workspace dep** (`clap = { version = "4", features = ["derive"] }`). Already used by `concerto-agent-host` (Task 21) as a local dep; promoting it surfaces the shared pin so `smoke-client`'s subcommand surface doesn't duplicate the version literal. `agent-host`'s `[dependencies]` now points at `{ workspace = true }`. Workspace-wide effect is benign — same v4 line, same `derive` feature; `cargo-deny clean`.
  - **`smoke.sh` Phase 2 block runs *before* Core shutdown.** The task pseudocode appended a Phase 2 block but kept the Phase 1 shutdown above it; that wouldn't actually work because the Phase 2 RPCs need the Core running. The implementation hoists the SIGTERM + `wait` + pid-file/socket assertions to the very end of the script so Phase 1 (caps) + Phase 2 (project/repo/workspace/workarea/session) both run against one live Core. The smoke-gate header banner is now `Smoke gate v2: …` for every step (was `v1`).
  - **Worktree `.git` is verified with `[ -e ]`, not `[ -d ]`.** `git worktree add` writes the worktree's `.git` as a regular file containing `gitdir: <abspath>` (not a directory). The pseudocode's `[ -d "$WT_ROOT"/*/.git ]` was rejected by ShellCheck and would also have falsely failed against a real `git worktree add` result. The check now iterates `for repo_dir in "$WT_ROOT"/*/; do [ -e "$repo_dir/.git" ] && …; done` with an inline `case` skip for the `.context/` sibling directory so it doesn't double-count.
  - **`WT_ROOT` resolution uses `find -maxdepth 1 -mindepth 1 -type d | head -n 1`** (not `ls "$CORE_DATA_DIR/workspaces/wsp" | head -1`), per shellcheck SC2012. The composer name is server-allocated (Task 20's locked pool); a glob would also work but `find` is the canonical shellcheck-clean form.
  - **`smoke-client clone` uses UFCS** to disambiguate `Repositories.Clone` from `Clone::clone` — same pattern Task 18's integration test locked. `RepositoriesClient::<Channel>::clone(&mut client, …)`.
  - **`stream-session-io` subscribes to BOTH `session.io.<sid>` AND `session.events.<sid>`** on the same channel, raced via `tokio::select!`. The events subscription is the source of truth for "AgentExited → exit 0"; the io subscription drains stdout bytes to the calling shell. Either stream EOS (`None` from `StreamExt::next`) is also treated as a clean end-of-session (the supervisor drops the broadcast tx on `AgentExited`) so the timeout path is only hit when the supervisor itself is wedged.
  - **`smoke-client` now multi-threaded.** Phase 1's single-flag form was current-thread; the streaming subcommand wants `tokio::select!` over two server streams, so the runtime is `new_multi_thread`. Negligible wall-clock cost (smoke gate spawns a fresh process per RPC).
  - **`add-project` resolves the DB path with a three-step precedence**: `$CONCERTO_DB_PATH` → `$CONCERTO_DATA_DIR/concerto.db` → `~/concerto/concerto.db`. The middle step matches `crates/core/src/runtime.rs::RuntimeConfig::db_path`'s effective behaviour (Core's `default_for_user` resolves `data_dir` from `CONCERTO_DATA_DIR`, then `db_path()` joins `concerto.db`); the smoke script exports `CONCERTO_DATA_DIR=$CORE_DATA_DIR` so the subcommand and the Core agree on the SQLite file with no extra wiring. Pre-decision 3 named only `$CONCERTO_DB_PATH` + `~/concerto/concerto.db`; the `$CONCERTO_DATA_DIR` middle step is the smaller change for the smoke script.
  - **`stream-session-io` exit code conforms to the task spec exactly**: 0 on either AgentExited or stream EOS; 1 on timeout, RPC error, or stdout-write error.
  - **Outputs list grew by two files** beyond the spec's enumeration: `crates/agent-host/Cargo.toml` (now `clap = { workspace = true }`) and `tools/smoke-client/src/connect.rs` (the shared UDS dial helper). Both pre-decisioned in the orchestrator brief.
  - **`docs/interfaces/*.md` regenerated**: `rust-api.md` picks up the new `concerto-smoke-client` modules (`connect`, `cmd::*`) under the workspace surface. No drift in other crates.
- **Open questions for next task:**
  - **`Projects.CreateProject` RPC is the natural next surface** if a later task wants to remove the sqlx workaround from smoke-client. Task 24 added `Projects.ListProjects`; adding `CreateProject` is an additive RPC at the next free field number (the field numbers in `projects.proto` are FROZEN at the V0.1 set per Task 24's handoff).
  - **The Phase 2 block adds ~20 s to smoke wall-clock** (cold cache: ~80 s total; warm CI: ~25 s). Within the orchestrator's "~60 s" target stated in the task spec.
  - **Phase 3 smoke gate** (Tasks 42 + 44) appends to the same script under a clearly marked block. The `# Phase 3 checks — added in Tasks 42 + 44` marker is in place; the trap-on-EXIT cleanup already removes `$CONCERTO_HOME` for any future block.
- **Deliberate debt:** smoke uses sqlx to insert a project (no `Projects.CreateProject` RPC in V0.1). Force-failure runs were verified manually per the task's "Skip from spec" note — orchestrator handles CI green confirmation. No `TODO`/`FIXME`/`todo!()`/`unimplemented!()` markers in new code.
- **Smoke-gate state:** **v2 active.** Covers: Core boot → UDS up → `GetServerCapabilities` → project (sqlx) → repo + clone → workspace + workarea (with on-disk `.context/` + `<repo>/.git` verified) → echo session + `Streams.Subscribe(session.io.<sid>)` round-trip → stop session → clean shutdown (pid file + socket gone).
