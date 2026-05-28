# Task 28 — Repo: fsmonitor, Maintenance, and Performance Config

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 18 |
| Touches subsystem(s) | 02 (Repository Manager) |
| Smoke gate | unchanged |

## Goal
Auto-apply the git performance settings from `design/00 §6.3` to every repository the Repo Manager owns: `core.fsmonitor=true`, `core.untrackedCache=true`, `feature.manyFiles=true`, `core.commitGraph=true`. Start and supervise `git fsmonitor--daemon` per repo. Register `git maintenance start` so background maintenance happens. After this task, `gix status` on a large repo is fast (Task 29 measures it).

## Inputs to read before starting
- `design/02_Repository_Manager.md` §3.4 (fsmonitor lifecycle), §3.5 (size auto-recommendation — V0.1 just reports), §6 (concurrency model), §8 (fsmonitor failure modes).
- `design/00_Architecture_Overview.md` §6.3 (locked git config: fsmonitor + untracked cache + commit-graph + manyFiles).
- `tasks/18-repository-cloning.md` → "Handoff Notes".

## Scope — in
- Extend `crates/gix-wrap/src/api.rs` with:
  - `apply_perf_config(repo_dir: &Path) -> Result<()>` — runs `git config` for each of the four settings.
  - `start_fsmonitor(repo_dir: &Path) -> Result<u32>` — spawns `git fsmonitor--daemon start`; returns the daemon pid.
  - `is_fsmonitor_alive(pid: u32) -> bool`.
  - `stop_fsmonitor(repo_dir: &Path) -> Result<()>`.
  - `register_maintenance(repo_dir: &Path) -> Result<()>` — runs `git maintenance start`.
- In `RepoManagerActor`:
  - Call `apply_perf_config` + `start_fsmonitor` + `register_maintenance` at end of `clone` (so every newly-cloned repo gets the settings).
  - On Core startup, scan all `repositories` rows and:
    - Apply the config if a flag in the on-disk `concerto-state.json` (per design §4) shows it's not yet applied.
    - Start/restart fsmonitor daemons; persist new PIDs to `repositories.fs_monitor_pid`.
- Add an fsmonitor supervisor loop: every 30s, check `is_fsmonitor_alive` per repo; restart on death up to 3 times in 60s, then disable for that repo with a `repo.fsmonitor_restarted` audit log entry.
- Test integration:
  - Clone a small repo via Task 18's flow.
  - Verify `git config --get core.fsmonitor` returns `true`.
  - Verify the daemon PID is alive.
  - Kill the daemon externally; wait 35s; assert it's restarted (new PID in DB).
  - Kill it 4 times rapidly; assert the supervisor gives up and emits the audit event.

## Scope — out
- Sparse checkout / blobless (V1.0).
- Pre-fetch policy (V1.0).
- Size auto-recommendation UI (V1.0).
- Submodules / LFS (V1.0).

## Public interface this task locks
- Rust: `crates/gix-wrap/src/api.rs` gains the four helpers above. Signatures frozen.
- `repositories.fs_monitor_pid` column is the authoritative record of the running daemon.
- Restart policy: 3 restarts in 60s, then disable.

## Implementation notes
- `git fsmonitor--daemon` exits if it can't bind its IPC socket; treat that as a "fsmonitor not supported on this filesystem" case (NFS, tmpfs sometimes) and disable gracefully with an info log.
- `git maintenance start` registers OS-level scheduled tasks (launchd plist on macOS, cron on Linux, scheduled task on Windows). It's idempotent — safe to call on every Core start.
- The 30s supervisor loop can be a single tokio task that iterates `repositories` rows and calls `is_fsmonitor_alive` for each; cheap.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core fsmonitor` → tests pass (mock filesystem or real git daemon).
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual: clone a repo; verify `git config -l` shows the four settings; verify fsmonitor process via `ps`.
5. Manual: `pkill -f fsmonitor--daemon`; wait 35s; verify it restarts.
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] Performance config applied to every cloned repo.
- [ ] Fsmonitor supervisor restart policy verified.
- [ ] Repo Manager continues working when fsmonitor disables on unsupported filesystems.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/gix-wrap/src/api.rs` (modified)
- `crates/gix-wrap/src/cmd.rs` (modified)
- `crates/core/src/repo_manager/fsmonitor.rs` (new — supervisor loop)
- `crates/core/src/repo_manager/actor.rs` (modified)
- `crates/persist/src/repositories.rs` (modified — `update_fs_monitor_pid`)
- `crates/core/tests/fsmonitor_lifecycle.rs` (new)

## Commit message
```
phase-3: repo fsmonitor + maintenance + perf config

Auto-applies core.fsmonitor / untrackedCache / commitGraph /
feature.manyFiles on clone. Supervises git fsmonitor--daemon with
3-in-60s restart policy. Registers git maintenance start per repo.

Refs: tasks/28-repo-fsmonitor-and-maintenance.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** —
- **Smoke-gate state:** unchanged.
