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
- [ ] Verification commands pass.
- [ ] Second-instance handling verified (exits 0, no log spam).
- [ ] PID file cleanup on SIGTERM verified.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

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
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** SIGHUP handler is a placeholder; full hot-reload of config is V1.0.
- **Smoke-gate state:** unchanged.
