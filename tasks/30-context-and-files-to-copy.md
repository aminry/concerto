# Task 30 — `.context/` Lifecycle and Files-to-Copy

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 20 |
| Touches subsystem(s) | 03 (Workspace Manager) |
| Smoke gate | unchanged |

## Goal
Flesh out the `.context/` directory the agent uses for scratch + todos + preamble + checkpoint metadata (Task 20 created a skeleton; this task makes it functional). Add the files-to-copy/symlink/exclude resolver from `design/03 §3.10` so workarea creation honors project-level patterns. After this task, creating a workarea reads `<project>/.concerto/.worktreeinclude` (if present), applies its rules into the workarea's repo worktrees, and the agent sees a fully-populated `.context/`.

## Inputs to read before starting
- `design/03_Workspace_Session_Manager.md` §3.10 (files-to-copy with copy/symlink/exclude modes, `.worktreeinclude` syntax), §4.2 (`.context/` directory contents).
- `design/03_Workspace_Session_Manager.md` §3.13 (project / repo settings precedence — touch lightly; full precedence is a V1.0 polish).

## Scope — in
- Implement files-to-copy resolver in `crates/core/src/workspace_manager/files_to_copy.rs`:
  - Parse `.worktreeinclude` syntax (gitignore-like with `!` for symlink suffix and leading `!` for exclude).
  - Apply rules to each repo's worktree by walking the project's reference worktree and copying/symlinking matching files.
  - Each rule's `mode`: `copy` / `symlink` / `exclude` per design.
  - Safety: symlinks rejected if they'd escape the project root; broken symlinks surface a per-workarea warning chip.
- Add a project's reference repo: V0.1 simplification — the reference is the workspace's only repo (since V0.1 is single-repo). When multi-repo arrives (V1.0), the reference is project setting.
- Apply files-to-copy at workarea creation (extend Task 20's `create_workarea` flow). The resolver runs after `git worktree add` and before the workarea status transitions to `active`.
- Expand `.context/` skeleton in `crates/core/src/workspace_manager/context_dir.rs`:
  - `PROMPT.md` (a placeholder with V0.1 minimal preamble — full templated preamble is Task 33 or later).
  - `todos.md` (empty checklist scaffold).
  - `scratch/` (empty dir).
  - `checkpoints/` (empty dir; populated by Task 34).
  - `concerto.log` (created when sessions start writing).
- Ensure every repo in the workarea has `.context/` added to its `.git/info/exclude` (already done in Task 20; verify it survives).
- Persist a `workareas.settings_json.files_to_copy_applied: true` flag once done so reruns are idempotent.
- Add tests:
  - `.worktreeinclude` parser: each syntactic case (copy, symlink, exclude, comments, blank lines).
  - End-to-end: workarea creation with a fixture project containing `.worktreeinclude`; assert files appear in the workarea's repo worktree per the rules.

## Scope — out
- Multi-repo file-to-copy targets (V1.0).
- Project settings checked-in vs local-DB vs managed.json precedence (V1.0 — design §3.13).
- Per-repo `action_prefs.toml` (V1.0).
- Symlink fallback on Windows (V1.0 — junctions/hardlinks).

## Public interface this task locks
- File format: `.worktreeinclude` syntax exactly as in `design/03 §3.10`.
- Path: `<project_root>/.concerto/.worktreeinclude` for the project-level file. `<project_root>` is interpreted as the project's reference repo worktree.
- `.context/` layout: as listed above. Other tasks add files but don't rename the four canonical entries.

## Implementation notes
- For symlink: use `std::os::unix::fs::symlink` on Unix, `std::os::windows::fs::symlink_file` on Windows (which requires developer-mode or admin — fall back to copy on permission error with a warning).
- Computing the relative symlink target: use `pathdiff::diff_paths` (`pathdiff = "0.2"`).
- The walker uses `walkdir` or `ignore` (the `ignore` crate already understands gitignore syntax — useful here).
- For path-escape safety: `dunce::canonicalize` the resolved path and check it's within the project root.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core files_to_copy` → all syntactic + end-to-end tests pass.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual: create a fixture project with a `.worktreeinclude` containing one of each rule type; create a workarea; verify files/symlinks/exclusions in the new worktree.
5. Symlink-escape test: a rule that would create a `..`-escaping symlink → rejected with error.
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] `.worktreeinclude` parser handles all four syntactic forms.
- [ ] Workarea creation applies the rules idempotently (running twice doesn't duplicate).
- [ ] Symlink path-escape rejection verified.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/core/src/workspace_manager/files_to_copy.rs` (new)
- `crates/core/src/workspace_manager/context_dir.rs` (new)
- `crates/core/src/workspace_manager/workarea.rs` (modified — calls the new resolvers)
- `crates/core/tests/files_to_copy.rs` (new)

## Commit message
```
phase-3: .context/ lifecycle + files-to-copy resolver

Implements .worktreeinclude parsing (copy/symlink/exclude modes) and
applies rules during workarea creation per design/03 §3.10. .context/
skeleton expanded with PROMPT.md, todos.md, scratch/, checkpoints/.

Refs: tasks/30-context-and-files-to-copy.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** Windows symlink fallback (junctions/hardlinks) deferred; project-settings precedence stack is V1.0.
- **Smoke-gate state:** unchanged.
