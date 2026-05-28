# Task 31 — Archive Lifecycle FSM (Workspace + Workarea + Session)

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 19, 20, 22 |
| Touches subsystem(s) | 03 (Workspace Manager), 04 (Agent Supervisor) |
| Smoke gate | unchanged |

## Goal
Implement the full archive + restore semantics from `design/03 §3.7`: archiving a workarea stops its sessions cleanly, optionally removes its worktree, and sets `archived_at`. Archiving a workspace cascades to all its workareas. Restore reverses the operation but resets `permission_mode` to the workspace default (security stance against silent yolo). The workarea FSM (`design/03 §3.1`) becomes enforced.

## Inputs to read before starting
- `design/03_Workspace_Session_Manager.md` §3.1 (workarea FSM), §3.7 (archive semantics), §6.5 (crash adoption on Core restart).

## Scope — in
- Implement workarea FSM in `crates/core/src/workspace_manager/fsm.rs`:
  - States: `created`, `active`, `running`, `awaiting`, `paused`, `finished`, `crashed`, `archived`.
  - A transition table that maps `(current, event) → new_state` with explicit allowed transitions.
  - Events derived from `AgentEvent`s — when a session in the workarea emits `Started`, transition `active → running`; when it emits `AwaitingApproval`, transition `running → awaiting`; etc.
- Implement `archive_workarea(id, ArchiveOpts)`:
  1. Stop every session in the workarea cleanly via `AgentSupervisorHandle::stop_session`.
  2. Optionally run a per-repo `scripts.archive` if defined in project settings (V0.1: stub — skip if absent; full project-settings precedence is V1.0).
  3. If `ArchiveOpts.remove_worktree`, run `git worktree remove --force` per repo and remove the `worktree_root` directory; default is keep (per design R-5).
  4. Set `workareas.archived_at`.
  5. Emit `workarea.events: archived`.
- Implement `restore_workarea(id)`:
  1. If worktree was removed, re-create it via `git worktree add` with the original branch.
  2. Clear `archived_at`.
  3. Reset `permission_mode` to NULL (inherits from workspace) per design §3.7's security stance.
  4. Emit `workarea.events: restored`.
- Implement `archive_workspace(id)`:
  1. List all workareas with `archived_at IS NULL`; archive each (cascading).
  2. Set `workspaces.archived_at`.
  3. Emit `workspace.events: archived`.
- `restore_workspace(id)` clears the workspace's `archived_at` only; workareas remain archived until individually restored.
- Add session-level archive: `stop_session(id, reason=archive)` already exists in Task 22 — but ensure it persists `sessions.ended_at` cleanly.
- Crash adoption on Core start: for each non-archived workarea, probe `worktree_root` existence; if missing, transition to `crashed` (do not auto-restore — user decides).
- Tests:
  - Archive workarea with running session: confirm session is stopped, archived_at set, worktree-removal opt honored.
  - Restore a workarea whose worktree was removed: confirm worktree re-created.
  - Archive workspace with 3 workareas: confirm all 3 archived in one transaction.
  - Restore workspace: workspace.archived_at cleared, workareas still archived.
  - Permission mode reset on restore verified.

## Scope — out
- Archive script execution per repo (V1.0 — depends on full project-settings precedence).
- Hard-delete UI (V1.5+).
- `scripts.archive` hang detection / kill (V1.0 — design §8 mentions 60s timeout; we can stub for V0.1).

## Public interface this task locks
- Rust: `archive_workarea(id, ArchiveOpts { remove_worktree: bool })`, `restore_workarea(id)`, `archive_workspace(id)`, `restore_workspace(id)`. Frozen.
- Proto: extends `Workareas` and `Workspaces` services with `ArchiveWorkarea(req)` / `RestoreWorkarea(id)` / `RestoreWorkspace(id)`.
- FSM transition table in `fsm.rs` is the authoritative state graph.

## Implementation notes
- The FSM is a pure function; test it exhaustively with a table-driven test.
- For `git worktree remove --force`, run it via `gix_wrap::cmd::run` — there's no clean `gix` API for worktree removal yet.
- The cascading archive uses a transactional sweep: lock-step in a single SQLite write transaction so partial cascades don't leave inconsistent state. SQLite serializes via the writer pool from Task 08.
- `workareas.archived_at IS NOT NULL` becomes the soft-delete predicate; every list-query in Tasks 19/20 should already respect `include_archived: bool`. Verify and update if not.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core archive` → all cases pass.
3. `cargo test -p concerto-core fsm` → table-driven FSM tests pass.
4. Manual via gRPC: create workarea, spawn session, archive workarea with `remove_worktree=true`; verify session ended + worktree gone + DB rows set; restore; verify worktree re-created + permission_mode reset.
5. `./scripts/regen-interfaces.sh && git diff` → committed.
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] FSM table-driven tests cover every state × event.
- [ ] Cascading archive is transactional (verified by injecting a failure mid-cascade and checking rollback).
- [ ] Permission-mode reset on restore verified.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/core/src/workspace_manager/fsm.rs` (new)
- `crates/core/src/workspace_manager/archive.rs` (new)
- `crates/persist/src/workspaces.rs` (modified — `restore`)
- `crates/persist/src/workareas.rs` (modified — `restore`, `update_status`)
- `crates/proto/proto/concerto/v1/workareas.proto` (modified)
- `crates/proto/proto/concerto/v1/workspaces.proto` (modified)
- `crates/core/src/handlers/workareas.rs`, `workspaces.rs` (modified)
- `crates/core/tests/archive_lifecycle.rs` (new)
- `crates/core/tests/fsm_table.rs` (new)
- `docs/interfaces/proto.md`, `rust-api.md` (regenerated)

## Commit message
```
phase-3: archive + restore lifecycle for the 3-level hierarchy

Workarea FSM enforced via a typed transition table. Archive
cascades workspace→workareas→sessions cleanly. Restore resets
permission_mode to workspace default per design/03 §3.7.

Refs: tasks/31-archive-lifecycle.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** archive scripts deferred; hard-delete UI is V1.5.
- **Smoke-gate state:** unchanged.
