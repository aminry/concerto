# Task 11 — Core Runtime Skeleton

| Field | Value |
|---|---|
| Phase | 1 |
| Size | small (≤4h) |
| Depends on | 05, 08 |
| Touches subsystem(s) | 01 (Runtime) |
| Smoke gate | unchanged |

## Goal
Build the single-instance guard, signal handling, and graceful-shutdown plumbing for `concerto-core`. After this task, `concerto-core` (a) refuses to start if another instance is already running, (b) handles SIGTERM/SIGINT by initiating clean shutdown, (c) cleans up its PID file and UDS socket on exit. The supervision-tree actor pattern is set up in Task 12.

## Inputs to read before starting
- `design/01_Core_Daemon_Runtime.md` §3.1 (daemonization strategy — OS-managed), §3.3 (single-instance guard — flock PID file), §3.4 (config model — defer to V1.0 for hot-reload; in V0.1 just read once at startup), §4.1 (on-disk: `~/.concerto/core.pid`, `~/.concerto/core.sock`), §6.4 (graceful shutdown sequence).
- `design/00_Architecture_Overview.md` §7.4 (observability — logs at `~/concerto/logs/`).
- `tasks/10-keychain-wrapper.md` → "Handoff Notes".

## Scope — in
- Add deps to `crates/core/Cargo.toml`: `fs2 = "0.4"` (or `fs4`) for flock, `tokio` (already present), `signal-hook = "0.3"` (Unix only — gate with `cfg`), `tokio-util` for `CancellationToken`.
- Implement `crates/core/src/runtime.rs` with:
  ```rust
  pub struct Runtime {
      pid_file: PidFile,
      shutdown: CancellationToken,
      persistence: Persistence,
  }
  
  pub struct RuntimeConfig {
      pub data_dir: PathBuf,    // default ~/concerto
      pub config_dir: PathBuf,  // default ~/.concerto
  }
  
  impl Runtime {
      pub async fn start(config: RuntimeConfig) -> Result<Self>;
      pub fn shutdown_token(&self) -> CancellationToken;
      pub async fn wait_for_shutdown(&self) -> Result<()>;
      pub async fn stop(self) -> Result<()>;
  }
  ```
- Implement `crates/core/src/pid_file.rs`:
  - `PidFile::acquire(path)` opens the file with `O_RDWR | O_CREAT`, attempts an exclusive non-blocking `flock`, writes current PID + version + start-epoch as JSON, returns the guard.
  - `PidFile::drop` releases the lock and removes the file.
  - If the lock is held, read the existing PID; check if the process still exists (`kill(pid, 0)` on Unix, `OpenProcess` on Windows); if it does, return `AlreadyRunning(pid)`; if not, the lock is stale — break it and retake.
- Implement signal handling in `crates/core/src/signals.rs`:
  - On Unix: install SIGTERM, SIGINT, SIGHUP handlers via `signal-hook-tokio`; emit to a channel.
  - On Windows: `tokio::signal::ctrl_c` only (no SIGHUP equivalent in V0.1).
  - SIGHUP triggers a config reload event (placeholder; actual reload arrives in V1.0).
- Wire into `crates/core/src/main.rs`:
  1. `logging::init()`.
  2. `Runtime::start(config)`.
  3. Install signal handlers.
  4. `runtime.wait_for_shutdown().await`.
  5. `runtime.stop().await`.
- The shutdown sequence (V0.1 minimum):
  1. Log "shutdown requested" with cause.
  2. Cancel the shutdown token (broadcasts to all subscribers).
  3. Wait up to 5 seconds for tasks to finish (no actors yet; this is plumbing for Task 12).
  4. Persistence shutdown.
  5. PidFile drop (releases flock, removes file).
- Add integration test: spawn `concerto-core` in a subprocess with `CONCERTO_DATA_DIR=$TEMPDIR`; verify `core.pid` is created; spawn a second instance; verify it exits 0 (with appropriate log) and does NOT corrupt the existing file; send SIGTERM to the first; verify clean exit and pid file removal.

## Scope — out
- Supervision tree / actor pattern (Task 12).
- Tray sidecar process (V1.0).
- Watchdog (V1.0).
- OTLP exporter (V1.0; opt-in).
- Config layering / hot-reload via SIGHUP (V0.1 reads config once; reload is V1.0).

## Public interface this task locks
- Rust: `crates/core/src/runtime.rs` — `pub struct Runtime`, `pub struct RuntimeConfig`, `Runtime::start/wait_for_shutdown/stop`.
- PID file path: `<config_dir>/core.pid` (default `~/.concerto/core.pid`).
- Exit codes: 0 on clean shutdown OR another instance present; non-zero only on a real error (persistence failure, etc.).

## Implementation notes
- `flock` semantics: hold the lock for the program's lifetime. Releasing the file descriptor releases the lock. Do NOT call `flock(..., LOCK_UN)` explicitly — let `Drop` close the FD.
- The lock check on Unix: `kill(pid, 0)` returns 0 if the process exists, ESRCH if not, EPERM if it exists but you don't have permission (treat EPERM as "exists").
- Don't use `nix::unistd::getpid()` — `std::process::id()` is portable.
- On Windows, the equivalent of flock is `LockFileEx(LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY)`. The `fs2` crate abstracts both.
- The `CancellationToken` from `tokio-util` is the canonical broadcast for shutdown. Pass clones to downstream tasks; they call `.cancelled().await` in `select!`.
- `wait_for_shutdown` returns when EITHER a signal fires OR the token is cancelled programmatically.

## Verification
1. `cargo build -p concerto-core` → succeeds.
2. `cargo test -p concerto-core runtime` → all tests pass, including the second-instance test.
3. `cargo clippy -p concerto-core -- -D warnings` → clean.
4. Manual:
   ```
   cargo run --bin concerto-core &
   CORE_PID=$!
   ls -l ~/.concerto/core.pid    # exists, contains PID+version
   cargo run --bin concerto-core  # exits cleanly with log "another instance running, pid=X"
   kill $CORE_PID                 # graceful shutdown
   ls ~/.concerto/core.pid 2>&1   # file should be gone
   ```
5. SIGINT (ctrl-C) also triggers clean shutdown.
6. `./scripts/regen-interfaces.sh && git diff docs/interfaces/rust-api.md` → updated.
7. `cargo deny check` → clean.

## Definition of Done
- [x] Verification commands pass.
- [x] Second-instance handling verified (exits 0, no log spam).
- [x] PID file cleanup on SIGTERM verified.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `crates/core/Cargo.toml` (modified — fs2, signal-hook, tokio-util)
- `crates/core/src/runtime.rs` (new)
- `crates/core/src/pid_file.rs` (new)
- `crates/core/src/signals.rs` (new)
- `crates/core/src/lib.rs` (modified — `pub mod runtime; pub mod pid_file; pub mod signals;`)
- `crates/core/src/main.rs` (modified — Runtime::start path)
- `crates/core/tests/runtime_lifecycle.rs` (new)
- `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-1: core runtime skeleton

Adds single-instance flock guard, Unix/Windows signal handling, and
graceful shutdown via CancellationToken per design/01 §3.3, §6.4.
Actor supervision lands in Task 12.

Refs: tasks/11-runtime-skeleton.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **`signal-hook = "0.3"` substituted with `tokio::signal::unix`.** The orchestrator prompt explicitly authorized this swap: `tokio` already had the `signal` feature enabled (Task 08), and `tokio::signal::unix::signal(SignalKind::{terminate,interrupt,hangup})` covers every signal V0.1 needs (SIGTERM, SIGINT, SIGHUP). No new dep, no `signal-hook-tokio`, identical observable behaviour. The `signals::install` API surface (`(JoinHandle, Receiver<ReloadEvent>)`) is what Task 12 will plug actors into.
  - **`fs2 = "0.4"` and `tokio-util = "0.7"` added as workspace deps**, not crate-local. Matches Task 05/08/10's pattern of pinning shared deps at the workspace root so Tasks 12/22 can reuse them without re-pinning.
  - **`libc = "0.2"` added as a `[target.'cfg(unix)']` dep** for `kill(pid, 0)` liveness probes. Not in the task's Scope-in list, but the spec calls for "`kill(pid, 0)` on Unix" and the alternatives (raw `nix` or hand-rolled `syscall!`) are heavier. libc is already a transitive dep, so this is essentially a re-export. Permissive (MIT/Apache-2.0); cargo-deny clean.
  - **`RuntimeConfig` gained a third field, `shutdown_grace: Duration`.** Spec sketch only listed `data_dir` and `config_dir`, but the task body also says "wait up to 5 seconds for tasks to finish". I plumbed that as a config field (default 5s) instead of a magic constant so Task 12's actor-supervision tests can shorten it. The field is `pub` — additive only; calls using struct-update syntax keep compiling. If the locked interface is meant to be strictly two-field, this would need to move to a private const + setter.
  - **`StartOutcome` enum added** alongside the locked `Runtime::start` signature. The task says `start` returns `Result<Self>`; in practice we need a three-way result (success / already-running / error) because the spec's "second instance exits 0" path is not an error. The enum is `Started(Runtime) | AlreadyRunning { pid }`, and the `main.rs` wiring maps `AlreadyRunning` to `Ok(())` with an info log. This is the only way I see to satisfy both "exit codes: 0 on clean shutdown OR another instance present" and Rust's type system without abusing `Result`.
  - **PID-file path is `<config_dir>/core.pid` exactly as locked.** The split between `data_dir` (`~/concerto`) and `config_dir` (`~/.concerto`) is honoured: DB stays at `data_dir/concerto.db`; pid lock at `config_dir/core.pid`. `CONCERTO_DB_PATH` (the Task 08 override) is still honoured by `RuntimeConfig::db_path()` so the smoke gate's existing wiring did not need to change.
  - **`main.rs` no longer carries its own SIGTERM/SIGINT block** — that logic is now inside `signals::install`, which `Runtime::start` calls. The orchestrator note about consolidating Task 08's handler into `signals.rs` is realised: the listener cancels the shared `CancellationToken`, and `runtime.wait_for_shutdown().await` is what `main.rs` blocks on. Diff against Task 08's `main.rs` shows ~50 lines removed.
  - **`docs/interfaces/rust-api.md` was NOT updated.** The interface generator only scrapes `crates/<crate>/src/api.rs`; `concerto-core` does not (and the task did not ask it to) follow that convention. `git diff docs/interfaces/` is empty after `scripts/regen-interfaces.sh`, which satisfies verification step 6 (`--exit-code`) — but it does mean Task 11's public types (Runtime/RuntimeConfig/StartOutcome) are not surfaced in `rust-api.md`. If the project wants them visible, a follow-up could re-export them from a new `crates/core/src/api.rs`. Flagging for the orchestrator.
  - **PID-file integration test polls via `wait_until` instead of fixed sleeps.** Spec said "verify pid file is created". I used a 20s poll loop (50 ms cadence) because cold-build CI on slower runners can take a few seconds to reach `runtime ready`; stdout is captured to `Stdio::null` so the test never blocks on a full pipe.
  - **The `logging::tests::rejects_invalid_level` from Task 05 still flakes intermittently in parallel** (the `RUST_LOG` env-var race the orchestrator warned about). All ten unit tests + the integration test pass with `cargo test -p concerto-core -- --test-threads=1`. I did NOT touch the test; per orchestrator directive, it stays as-is.
- **Open questions for next task:**
  - Task 12's supervisor will need to subscribe to `Runtime::shutdown_token()` (clone-on-spawn) and own the panic-isolation `catch_unwind` harness per `design/01 §3.2`. `Runtime::persistence()` returns `Option<&Persistence>` so the supervisor can hand it to children; if Task 12 wants to consume the persistence handle into a `PersistenceHandle` (the `ActorContext` field), I should refactor `Runtime` to expose an `into_parts()` method instead of `persistence()` — easier than reworking `stop()`'s consumption order.
  - The `ReloadEvent` receiver from `signals::install` is exposed via `Runtime::take_reload_rx()`. V0.1 has no consumer; Task 12 (or the future config-reload task in V1.0) should `take` it once and drive it. Calling `take` more than once returns `None` — that's the only guard against multiple-consumer races.
  - Stale-lock breaking is currently best-effort: if process A crashed mid-write and left a partially-serialized JSON, we treat that as "stale + unknown PID" and break the lock. That's the right call for a crash recovery, but it means a malicious local process could DoS the legitimate Core by writing garbage into `core.pid`. The mitigation is "the user's home directory is the trust boundary" — same as keychain, same as logs. Task 12's audit log should record every `breaking stale pid lock` event.
  - `Runtime::start` currently logs a single `acquired single-instance lock` line. The structured fields (`pid`, `version`, `pid_file`) match the `tracing::info!` posture Task 16 will codify. No drift.
  - The integration test uses `env!("CARGO_BIN_EXE_concerto-core")` — only works for tests in the same crate as the binary. If Task 12 moves the integration test to a separate `dev-deps` harness crate (Task 17 is planned to add one), the harness will need a more elaborate binary-path resolver. Leaving as-is for now.
- **Deliberate debt:** SIGHUP handler is a placeholder; full hot-reload of config is V1.0 (`design/01 §3.4 R-4`). On every HUP we log "SIGHUP received; config reload is V1.0 (no-op in V0.1)" and emit a `ReloadEvent::SighupReceived` on a bounded mpsc — no subscriber yet. Windows liveness probe in `pid_file::process_alive` is hardcoded to `true` (conservative); real `OpenProcess` lookup arrives when V1.0 lights up the Windows port. No `TODO`/`FIXME` markers in code.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still prints "Smoke gate: PASSED (no checks active yet — Phase 0)". The first real smoke assertion lands in Task 15 (gRPC `GetCapabilities` round-trip).
