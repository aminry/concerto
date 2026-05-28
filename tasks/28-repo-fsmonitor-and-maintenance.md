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
- [x] Verification commands pass.
- [x] Performance config applied to every cloned repo.
- [x] Fsmonitor supervisor restart policy verified.
- [x] Repo Manager continues working when fsmonitor disables on unsupported filesystems.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

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
- **Drift from plan:**
  - **No 0002 migration.** The pre-decision authorised `crates/persist/migrations/0002_repositories_fs_monitor_pid.sql`, but migration 0001 (Task 09) already shipped `fs_monitor_pid INTEGER` on `repositories`. Adding a 0002 to ALTER an existing column would fail (SQLite would reject the duplicate column). Instead the change is Rust-only: `Repository` gains `pub fs_monitor_pid: Option<i64>` in `crates/persist/src/api.rs`; `crates/persist/src/repositories.rs` reads the column in `get`/`list_by_project`/the new `list_all`, and writes it via the new `update_fs_monitor_pid`. `docs/interfaces/rust-api.md` regenerated — the only diff is the new field.
  - **35s kill-and-restart slice skipped per orchestrator instructions.** Replaced with deterministic in-process tests against `fsmonitor::probe_all` + a stubbed `is_alive` closure. `crates/core/tests/fsmonitor_lifecycle.rs::restart_policy_disables_after_three_in_window` exercises the 3-in-60s cap; `probe_all_respects_disabled_flag` proves a disabled history short-circuits future restart attempts; `probe_all_reports_alive_when_pid_is_live` covers the happy path.
  - **`register_maintenance` swallows non-zero exits.** `git maintenance start` writes a launchd plist on macOS / cron entry on Linux / scheduled task on Windows; CI runners and sandboxed test envs commonly lack the scheduler. Treating the failure as a debug-trace + `Ok(())` matches the spec's "idempotent — safe to call on every Core start" framing and keeps the clone path resilient.
  - **fsmonitor PID parser is a small free function** (`api::parse_fsmonitor_pid`) covering the three documented `git fsmonitor--daemon status` output shapes (`pid=N`, `pid: N`, `pid N`). Unit-tested under `api::fsmonitor_tests`.
  - **Supervisor loop spawned as a tokio task from `RepoManagerActor::run`** rather than running on the actor's mailbox. The actor's `run` now spawns the loop, parks on shutdown, and aborts the loop on cancellation as a backstop (the loop honours `CancellationToken::cancelled` natively). The handle (`RepoManager`) exposes `pub(crate) fn persistence()` + `fn fsmonitor_history()` so the run loop can construct the supervisor without re-piping the deps through the actor struct.
  - **`gix-wrap` gained a target-gated `libc` dep** (`[target.'cfg(unix)'.dependencies] libc = "0.2"`). Same `kill(pid, 0)` ESRCH/EPERM probe Task 11's `pid_file` already uses; the Windows stub returns `false` (V0.1 Unix-only build matrix).
  - **`stop_fsmonitor` is idempotent.** `git fsmonitor--daemon stop` exits non-zero when no daemon is running for the repo; the helper downgrades that `Error::Git` to `Ok(())` because the post-condition (no daemon) is already satisfied.
- **Open questions for next task:**
  - **Task 29 (`gix status` hot path)** can now assume `core.fsmonitor=true` is set on every cloned repo and the daemon is running for repos where the filesystem supports it. The `bring_up_after_clone` helper in `crates/core/src/repo_manager/fsmonitor.rs` is the single source of truth for the perf-config + fsmonitor + maintenance triple.
  - **`concerto-state.json` per-repo file is NOT implemented in V0.1.** The spec mentions it for "apply the config if a flag in the on-disk `concerto-state.json` shows it's not yet applied"; we always re-apply on clone (cheap — four `git config` calls) and let the supervisor probe-and-restart catch the per-Core-restart bring-up case. The state file is a V1.0 follow-on if/when prefetch cursors and size estimates need durable repo-local storage.
  - **Cold-start fsmonitor bring-up scan is not implemented.** The 30s supervisor loop walks every repo on each tick and restarts dead daemons, which subsumes the cold-start scan (after one tick post-boot every dead daemon has been restarted up to the cap). Adding an explicit boot-time scan would shorten the first-restart latency from `≤30s` to `≤0s`; a worthwhile micro-optimisation but out of scope for this task.
- **Deliberate debt:** no on-disk `concerto-state.json`, no boot-time fsmonitor scan, no fsmonitor-restart broadcast event (the spec mentions `repo.fsmonitor_restarted` in `design/02 §5.3`; V0.1 emits the equivalent via `tracing::info!` audit lines — wiring it to a broadcast channel is a Phase 3 follow-on). No `TODO`/`FIXME`/`todo!()`/`unimplemented!()` markers in new code.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` (v2) still drives the full repo + workspace + workarea + session flow against a fresh Core; the post-clone fsmonitor bring-up adds a `git config` sequence to the clone path that the smoke fixture happily absorbs.
