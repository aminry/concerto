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
- [ ] Verification commands pass.
- [ ] Checkpoint ref + DB row created on every turn-complete.
- [ ] Revert works end-to-end with a real agent.
- [ ] Soft-delete of chat messages after checkpoint verified.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

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
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** auto-restart after revert is deferred; multi-repo turns work but V0.1 single-repo only.
- **Smoke-gate state:** unchanged.
