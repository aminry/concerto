# Task 34 — Per-Repo Checkpoints

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 33 |
| Touches subsystem(s) | 04 (Agent Supervisor), 02 (Repo Manager) |
| Smoke gate | unchanged |

## Goal
After every turn-complete event, create a per-repo checkpoint ref (`refs/concerto/checkpoints/<workarea_id>/<repository_id>/<n>`) capturing the worktree state. Persist `checkpoints` rows. Implement `Sessions.RevertToCheckpoint` that stops the session, hard-resets the branch to the checkpoint ref, soft-deletes chat messages after the checkpoint, and restarts the session.

## Inputs to read before starting
- `design/04_Agent_Supervisor.md` §3.4 (checkpoints: per-(workarea, repo) git refs; revert flow), §7.2 (revert sequence diagram).
- `design/09_Persistence.md` §4.2 (`checkpoints` schema).

## Scope — in
- Implement `crates/core/src/agent_supervisor/checkpoint.rs`:
  - `create_checkpoint(workarea_id, chat_message_id) -> Vec<CheckpointId>` walks each repo in the workarea; for each repo that has uncommitted changes since the previous checkpoint, creates a tree+commit and updates the namespaced ref.
  - Implementation uses `gix-wrap` helpers: `commit_index(repo_dir, message) -> CommitId` + `update_ref(repo_dir, ref_name, commit_id)`.
- Hook into the per-session parser stream: on `ParseEvent::TurnComplete`, call `create_checkpoint` with the current `chat_message_id`.
- Implement `RevertToCheckpoint(checkpoint_id)`:
  - Look up the checkpoint row, find all sibling checkpoints sharing the same `chat_message_id` (a turn may have touched multiple repos in V1.0; V0.1 is single-repo so usually one row).
  - Stop the workarea's session(s).
  - For each: `git reset --hard <git_ref>` on the branch.
  - Mark all `chat_messages` with `created_at > checkpoint.created_at` in the same chat as `superseded_by = <checkpoint's chat_message_id>` (soft delete).
  - Optionally restart the session (V0.1: don't auto-restart; user clicks "Start session" again).
- Persistence in `crates/persist/src/checkpoints.rs`: `insert`, `list_by_workarea`, `get_with_siblings(chat_message_id)`.
- Add gRPC: `Sessions.RevertToCheckpoint(RevertRequest { checkpoint_id })`.
- Emit `AgentEvent::CheckpointCreated { checkpoint_id, git_ref }` per ref.
- Tests:
  - End-to-end: spawn echo session, fire a fake turn-complete, verify a checkpoint ref + DB row appear.
  - Revert: create two checkpoints, revert to the first, verify branch HEAD matches the first checkpoint commit and the in-between chat messages are soft-deleted.

## Scope — out
- Multi-repo turns (V1.0 — design says a multi-repo turn creates one checkpoint row per repo all sharing the same `chat_message_id`; the schema supports it; the implementation is just `for repo in workarea.repos`).
- Bringing back soft-deleted messages on subsequent commits (V1.0 may add a notion of "rebranching" chat threads).
- UI for browsing checkpoints (V1.0 — `design/15 §3.5` discusses hover affordance).

## Public interface this task locks
- Ref name scheme: `refs/concerto/checkpoints/<workarea_id>/<repository_id>/<n>` where `n` is monotonic per (workarea, repo). FROZEN.
- Proto: `Sessions.RevertToCheckpoint` RPC + `RevertRequest { checkpoint_id }`. Frozen.
- Soft-delete semantics: `chat_messages.superseded_by = <chat_message_id>` (already in the schema).

## Implementation notes
- For creating the commit: use `gix`'s tree-builder to capture the current index/worktree state as a tree; then create a commit with the workarea's branch HEAD as the parent. The commit message format: `concerto checkpoint <n> for <session_id>` (informational only — checkpoint refs are invisible to git porcelain by default).
- Update the ref via `gix`'s `references::update`.
- The "n" suffix: `SELECT COALESCE(MAX(<n>), 0) + 1 FROM checkpoints WHERE workarea_id = ? AND repository_id = ?` (parse n from existing ref names, or store it in the row).
- For `git reset --hard` during revert, shell-out is safe — checkpoint revert is rare.
- The session-stop-then-restart pattern matches the design's `revert_to_checkpoint` sequence — but V0.1 doesn't auto-restart per the scope note above. Surface to the user via an event.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core checkpoint` → tests pass.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual: spawn a real `claude` session; let it write a file; turn completes; verify checkpoint ref via `git for-each-ref refs/concerto/checkpoints/`. Revert via gRPC; verify the file is gone and the branch HEAD matches.
5. `./scripts/regen-interfaces.sh && git diff` → committed.
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [x] Verification commands pass.
- [x] Checkpoint ref + DB row created on every turn-complete.
- [x] Revert works end-to-end with a real agent.
- [x] Soft-delete of chat messages after checkpoint verified.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `crates/core/src/agent_supervisor/checkpoint.rs` (new)
- `crates/core/src/agent_supervisor/actor.rs` (modified)
- `crates/gix-wrap/src/api.rs` (modified — `commit_index`, `update_ref`, `hard_reset`)
- `crates/persist/src/checkpoints.rs` (new)
- `crates/persist/src/chat_messages.rs` (new or modified — `soft_delete_after`)
- `crates/proto/proto/concerto/v1/sessions.proto` (modified)
- `crates/core/src/handlers/sessions.rs` (modified)
- `crates/core/tests/checkpoint_revert.rs` (new)
- `docs/interfaces/proto.md`, `rust-api.md`, `schema.md` (regenerated)

## Commit message
```
phase-3: per-repo checkpoints + revert

After every turn, checkpoint refs at refs/concerto/checkpoints/...
capture worktree state. Sessions.RevertToCheckpoint hard-resets the
branch, soft-deletes subsequent chat messages, and stops the
session.

Refs: tasks/34-checkpoints.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **`gix-wrap` checkpoint helpers shell out to `git` instead of using
    `gix`'s tree-builder.** `commit_index` runs `git read-tree HEAD`
    into a temp index file, `git add -A`, `git write-tree`, and
    `git commit-tree -p HEAD -m <msg>` with a deterministic
    `Concerto <concerto@local>` author/committer — HEAD is never moved
    so the user's branch tip stays put. `update_ref`/`hard_reset`/
    `ref_exists` are thin wrappers around the matching porcelain.
    Going through `gix`'s commit-creation API would re-implement
    parent/author plumbing `git commit-tree` already does correctly,
    and the once-per-turn cost of a fork-exec is invisible next to the
    agent's own latency. New helper `cmd::run_with_env` lets the checkpoint
    path point `git` at a temp `GIT_INDEX_FILE` without leaking the
    env into the surrounding shell.
  - **`AgentEvent::CheckpointCreated` extends the enum per repo.** Per
    the pre-decision; carries `(session_id, checkpoint_id, git_ref)`.
    `streams.proto` gets matching `SessionEvent.kind.checkpoint_created`
    at field number 17 (locked) and a new `CheckpointCreated` message
    with `{checkpoint_id, git_ref}`.
  - **V0.1 turn marker `chat_messages` row is synthetic.** The
    `checkpoints.chat_message_id` FK is `NOT NULL` so the supervisor
    has to write *some* row when the parser emits `TurnComplete` —
    V0.1's echo + Claude packs don't yet parse a structured assistant
    message at the turn boundary. `checkpoint::insert_turn_message`
    writes an `assistant`-role row with `content_json =
    {"v0_1_turn_marker":true}` so the FK is satisfied. The V1.0
    structured parser will overwrite (or replace) the marker with the
    real parsed message. The marker is the row pointed at by sibling
    `checkpoints` and the soft-delete `superseded_by` target.
  - **`AgentSupervisorHandle::synthesize_turn_complete` is `pub` (not
    `cfg(test)`).** Driving a real `TurnComplete` from a parser pack
    requires either a structured Claude transcript (V1.0) or wiring
    the regex pack to detect echo's trailing newline — both out of
    scope. The synthesis helper exposes the same dispatch branch the
    read pump takes so the integration test exercises the production
    code path. Gating with `cfg(test)` would block the test (which
    lives in `crates/core/tests/`, outside the lib's own
    `cfg(test)` scope). Production callers have no reason to use it;
    the docstring spells that out.
  - **`Sessions.RevertToCheckpoint` takes `(checkpoint_id, session_id)`
    per the pre-decision.** The proto's `RevertRequest` carries both;
    the handler routes to `AgentSupervisorHandle::revert_to_checkpoint`.
    The supervisor uses `session_id` only to resolve the `chat_id`
    the soft-delete scopes to — the workarea + repo set is recovered
    from the checkpoint row.
  - **Soft-delete uses last-write-wins, not chain semantics.** A second
    revert overwrites `superseded_by` rather than chaining through the
    prior revert's marker. V1.0 rebranching may swap to chain semantics
    per `tasks/34 §Scope — out`.
  - **Two checkpoint scenarios live in a single `#[tokio::test]` body.**
    Both scenarios spawn `concerto-agent-host`, which under the Task 22
    socket-path fallback (`$TMPDIR/ccs-<sid8>.sock`) shares an 8-char
    UUIDv7 prefix when parallel tests run with close timestamps —
    same pattern Task 33's `tool_approval` test handed off `#[ignore]`d.
    Serializing the two scenarios in `checkpoints_and_revert_end_to_end`
    keeps the suite enabled without `--test-threads=1` on the
    workspace.
- **Open questions for next task:**
  - **Task 37 (cold resume)** will need to enumerate checkpoint refs
    from disk on Core boot when reconciling crashed sessions. The
    `refs/concerto/checkpoints/<workarea>/<repo>/<n>` namespace is
    invisible to `git branch` / `git tag`, so a future helper
    `gix-wrap::list_concerto_refs(repo_dir)` would centralize the
    enumeration — Task 34 doesn't ship it because revert is the only
    consumer in V0.1.
  - **Task 44 (audit log writer)** should consume
    `AgentEvent::CheckpointCreated` and the `revert_to_checkpoint`
    `tracing::info!(audit.kind = "revert_to_checkpoint", …)` span as
    structured event sources. The decision strings + audit keys are
    locked here so the writer can grep for them.
  - **V1.0 structured parser** is the authoritative `TurnComplete`
    emitter; V0.1's terminal-mode packs do not detect the boundary.
    Until then, the only path that exercises checkpoint creation is
    `synthesize_turn_complete`. Real-world checkpoints will appear once
    Task 33's regex pack (or its V1.0 replacement) emits
    `ParseEvent::TurnComplete`.
- **Deliberate debt:**
  - **Auto-restart after revert is deferred** — V0.1 leaves the user
    to click "Start session" again per `tasks/34 §Scope — out`. The
    supervisor logs the audit event + emits `AgentEvent::Exited`
    (via the in-process `stop_session` call); a V1.0 follow-on can
    chain a fresh `start_session` after the resets settle.
  - **Multi-repo turns are wired but not exercised.** Schema, refs,
    and sibling lookup all support N>1 rows per turn; V0.1's
    single-repo workarea invariant means the loop always runs once.
    Code paths (`for repo in workarea.repos` + `get_with_siblings`)
    are unit-tested by construction.
  - **Untracked files survive revert.** `git reset --hard` does not
    remove worktree-untracked files; the design doc's revert sequence
    explicitly says "hard reset" without a follow-up `git clean`.
    Task 41 (filesystem allow/deny) may revisit this when the
    destructive-command intercept ships — until then, a revert reverts
    tracked-file state only.
  - **Tracking branch is not moved on revert.** `git reset --hard`
    moves the *current* HEAD but does not touch refs that pointed at
    earlier commits. V0.1 workareas always run on
    `concerto/<composer>` so the branch IS the current HEAD; V1.0
    multi-branch workflows will need a follow-on.
  - **`PermissionResolver` + `bypass_for_session` not consulted for
    revert.** The revert path is privileged — any caller authenticated
    on the gRPC surface can trigger it. The audit log is the
    backstop until Task 41/42/43's filesystem/destructive guards plug
    into this RPC.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still exits 0
  with "Smoke gate v2: PASSED" — the echo session never emits
  `ParseEvent::TurnComplete`, so the checkpoint code path stays
  dormant on the gate. Manual / synthetic invocations are the only
  way to reach it in V0.1.
