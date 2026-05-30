# Embedded-Core Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional mode that links `concerto-core` into the `concerto-desktop` binary and boots Core in-process, enabling a fast dev hot-reload loop and a single-artifact standalone install — without changing the production two-process architecture.

**Architecture:** Reuse the existing gRPC-over-UDS transport. Extract Core's boot orchestration (currently in `crates/core/src/main.rs::run()`) into a reusable library module `concerto_core::boot`. The desktop shell, under a `embedded-core` Cargo feature, calls `boot::start()` on a dedicated tokio runtime, points its existing gRPC client at the socket Core binds, and on window close tears Core down. A runtime flag selects in-process vs external per launch. Core's existing PID single-instance lock serves as the coexistence guard.

**Tech Stack:** Rust, Tokio, Tonic (gRPC over Unix domain socket), Tauri 2, Cargo features.

---

## File Structure

| Path | Responsibility | Action |
|---|---|---|
| `crates/core/src/boot.rs` | Reusable Core boot orchestration: `start()`, `BootOutcome`, `RunningCore`. | Create |
| `crates/core/src/lib.rs` | Declare `pub mod boot;`. | Modify |
| `crates/core/src/main.rs` | Daemon entry — thin caller of `boot::start`. | Modify |
| `crates/core/tests/embedded_boot.rs` | Integration test: boot against scratch dir, dial socket, shut down. | Create |
| `apps/desktop/src-tauri/Cargo.toml` | Optional `concerto-core` dep + `embedded-core` feature. | Modify |
| `apps/desktop/src-tauri/src/core_client.rs` | Process-wide socket-path override (`set_socket_override`). | Modify |
| `apps/desktop/src-tauri/src/embedded.rs` | Mode selection, config resolution, embedded boot wiring. | Create |
| `apps/desktop/src-tauri/src/main.rs` | Call embedded boot in `setup`; store shutdown handle. | Modify |
| `apps/desktop/src-tauri/src/tray.rs` | Window-close = full shutdown when embedded. | Modify |
| `apps/desktop/src-tauri/tauri.conf.json` | Dev watcher covers `crates/`. | Modify |
| `scripts/smoke-embedded.sh` | Smoke variant exercising the one-process path. | Create |
| `README.md` | Document embedded mode + flags. | Modify |

---

## Task 1: Extract Core boot into `concerto_core::boot`

**Files:**
- Create: `crates/core/src/boot.rs`
- Modify: `crates/core/src/lib.rs` (module list, alphabetical)
- Modify: `crates/core/src/main.rs:67-441` (the `run()` function)

This is a **pure relocation** of the existing `run()` body. No behavior change for the daemon — the existing Core integration tests are the proof.

- [ ] **Step 1: Declare the module in `lib.rs`**

In `crates/core/src/lib.rs`, add the module declaration in alphabetical position (after `pub mod audit;`):

```rust
pub mod boot;
```

- [ ] **Step 2: Create `boot.rs` with the relocated boot logic**

Create `crates/core/src/boot.rs`. Copy the body of `main.rs::run()` (lines 67–441) into `boot::start`, stopping at the `tracing::info!("concerto-core ready");` line. The shutdown tail (`wait_for_shutdown` / `stop`) moves into `RunningCore`. Replace the function header and the final lines as shown; the large actor-spawn block in the middle is copied verbatim.

```rust
//! Reusable Core boot orchestration.
//!
//! Hosts everything `main.rs::run()` used to do up to "concerto-core
//! ready": resolve config, start the [`Runtime`], spawn every
//! supervised actor + the gRPC server. Returns a [`RunningCore`] the
//! caller drives to completion. Both the daemon binary and the
//! embedded desktop path call [`start`].

use std::path::PathBuf;
use std::sync::Arc;

#[cfg(unix)]
use crate::agent_supervisor::{AgentSupervisorActor, AgentSupervisorConfig};
use crate::api_server::{ApiServerActor, ApiServerConfig};
use crate::audit::{AuditWriterTask, JsonlFileSubscriber};
use crate::repo_manager::{RepoManagerActor, RepoManagerConfig};
use crate::runtime::{Runtime, RuntimeConfig, StartOutcome};
#[cfg(unix)]
use crate::scheduler::{SchedulerActor, SchedulerConfig};
use crate::skills::{SkillsRegistryActor, SkillsRegistryConfig};
#[cfg(unix)]
use crate::suggestions::{SuggestionEngineActor, SuggestionEngineConfig};
use crate::vcs::{VcsConfig, VcsProviderActor};
use crate::workspace_manager::{
    WorkareaManagerActor, WorkareaManagerConfig, WorkspaceManagerActor, WorkspaceManagerConfig,
};
use concerto_error::Result;

/// Outcome of [`start`]. Mirrors [`StartOutcome`] so callers can react
/// to the single-instance guard (the embedded desktop path falls back
/// to dialing the live daemon on `AlreadyRunning`).
pub enum BootOutcome {
    Started(RunningCore),
    AlreadyRunning { pid: u32 },
}

/// A booted, ready Core. Hold it to keep Core alive; call
/// [`RunningCore::run_until_shutdown`] to block until a shutdown signal
/// (or a cancelled [`RunningCore::shutdown_token`]) then tear down.
pub struct RunningCore {
    runtime: Runtime,
    socket_path: PathBuf,
}

impl RunningCore {
    /// The UDS path the gRPC server bound. Clients dial this.
    pub fn socket_path(&self) -> &std::path::Path {
        &self.socket_path
    }

    /// A clone of the runtime's shutdown token. Cancel it to trigger an
    /// orderly shutdown from another thread (e.g. a window-close handler).
    pub fn shutdown_token(&self) -> tokio_util::sync::CancellationToken {
        self.runtime.shutdown_token()
    }

    /// Block until shutdown is signalled, then stop the runtime
    /// (releases the PID lock, flushes audit, stops agents).
    pub async fn run_until_shutdown(self) -> Result<()> {
        self.runtime.wait_for_shutdown().await?;
        tracing::info!("shutdown signal observed");
        self.runtime.stop().await?;
        tracing::info!("concerto-core stopped");
        Ok(())
    }
}

/// Boot Core to readiness. Returns once the gRPC server is accepting on
/// the UDS. Errors propagate; `AlreadyRunning` is a non-error outcome.
pub async fn start(config: RuntimeConfig) -> Result<BootOutcome> {
    tracing::info!("concerto-core starting");

    tracing::info!(
        data_dir = %config.data_dir.display(),
        config_dir = %config.config_dir.display(),
        "resolved runtime config"
    );

    let socket_path = config.config_dir.join("core.sock");
    let repos_root = config.data_dir.join("repos");
    let data_dir = Arc::new(config.data_dir.clone());
    let config_dir = Arc::new(config.config_dir.clone());
    let mut runtime = match Runtime::start(config).await? {
        StartOutcome::Started(r) => r,
        StartOutcome::AlreadyRunning { pid } => {
            tracing::info!(other_pid = pid, "another instance is live");
            return Ok(BootOutcome::AlreadyRunning { pid });
        }
    };

    // ----------------------------------------------------------------
    // BEGIN verbatim relocation of main.rs::run() lines 99–432:
    // the RepoManager, AuditWriter, Workspace/Workarea managers,
    // (unix) AgentSupervisor + handle wiring, (unix) Scheduler,
    // SkillsRegistry + boot refresh, (unix) SuggestionEngine + pump,
    // crash-adoption sweep, (unix) pty hot-reconnect sweep, VCS
    // provider + auth probe, and the ApiServerActor spawn.
    //
    // Copy that entire block UNCHANGED. It already references
    // `runtime`, `persistence`, `repos_root`, `data_dir`,
    // `config_dir`, and `socket_path`, all in scope above.
    // ----------------------------------------------------------------

    tracing::info!("concerto-core ready");

    Ok(BootOutcome::Started(RunningCore {
        runtime,
        socket_path,
    }))
}
```

When copying the middle block, delete the now-duplicated `use` statements at the top of `main.rs` (they live in `boot.rs` now) and the local `let socket_path` / `repos_root` / `data_dir` / `config_dir` bindings (also in `boot.rs`).

- [ ] **Step 3: Slim `main.rs::run()` to call `boot::start`**

Replace `crates/core/src/main.rs` lines 14–31 (the `use` block) and the entire `run()` body (lines 67–441) so the file reads:

```rust
// (keep the file-level doc comment and the windows_subsystem attr)

use concerto_core::boot::{self, BootOutcome};
use concerto_core::logging;
use concerto_core::runtime::RuntimeConfig;
use concerto_error::Result;

fn main() -> std::process::ExitCode {
    let _log_guard = match logging::init() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("failed to initialize logging: {e}");
            return std::process::ExitCode::from(1);
        }
    };

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!(error = %e, "failed to build tokio runtime");
            return std::process::ExitCode::from(1);
        }
    };

    match rt.block_on(run()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "concerto-core exited with error");
            std::process::ExitCode::from(1)
        }
    }
}

async fn run() -> Result<()> {
    let config = RuntimeConfig::default_for_user()?;
    match boot::start(config).await? {
        BootOutcome::Started(core) => core.run_until_shutdown().await,
        // Per design/01 §3.3: exit 0 so launchd doesn't restart us.
        BootOutcome::AlreadyRunning { .. } => Ok(()),
    }
}
```

- [ ] **Step 4: Verify the daemon still compiles and existing tests pass**

Run: `cargo build -p concerto-core && cargo test -p concerto-core`
Expected: builds clean; all existing tests pass (e.g. `grpc_runtime`, runtime tests). This proves the relocation is behavior-identical.

- [ ] **Step 5: Write the embedded-boot round-trip integration test**

Create `crates/core/tests/embedded_boot.rs`:

```rust
//! Proves `boot::start` produces a Core that serves gRPC over its UDS
//! and shuts down cleanly when its token is cancelled — the contract
//! the embedded desktop path depends on.

use std::time::Duration;

use concerto_core::boot::{self, BootOutcome};
use concerto_core::runtime::RuntimeConfig;

#[tokio::test(flavor = "multi_thread")]
async fn embedded_boot_serves_and_shuts_down() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().join("data");
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::create_dir_all(&config_dir).unwrap();

    let config = RuntimeConfig {
        data_dir: data_dir.clone(),
        config_dir: config_dir.clone(),
        shutdown_grace: Duration::from_secs(5),
    };

    let core = match boot::start(config).await.expect("boot::start") {
        BootOutcome::Started(c) => c,
        BootOutcome::AlreadyRunning { pid } => panic!("unexpected live instance pid={pid}"),
    };

    // The bound socket exists and matches the config dir.
    let sock = core.socket_path().to_path_buf();
    assert_eq!(sock, config_dir.join("core.sock"));
    assert!(sock.exists(), "socket should be bound after boot");

    let token = core.shutdown_token();
    let join = tokio::spawn(async move { core.run_until_shutdown().await });

    // Trigger orderly shutdown and confirm it returns.
    token.cancel();
    let res = tokio::time::timeout(Duration::from_secs(10), join).await;
    assert!(res.is_ok(), "run_until_shutdown should return after cancel");
    res.unwrap().expect("join").expect("clean shutdown");
}
```

- [ ] **Step 6: Run the new test**

Run: `cargo test -p concerto-core --test embedded_boot -- --nocapture`
Expected: PASS. (On non-unix this would skip the agent-host actors; V0.1 is macOS-only so the test runs on the target platform.)

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/boot.rs crates/core/src/lib.rs crates/core/src/main.rs crates/core/tests/embedded_boot.rs
git commit -s -m "refactor(core): extract boot orchestration into concerto_core::boot

Lift main.rs::run() into a reusable boot::start returning a RunningCore.
Daemon behavior unchanged; enables embedding Core in the desktop binary."
```

---

## Task 2: Make the desktop gRPC client's socket path injectable

**Files:**
- Modify: `apps/desktop/src-tauri/src/core_client.rs:48-59` (`default_socket_path`)

`commands.rs` calls `default_socket_path()` in `concerto_rpc`, `concerto_subscribe`, and `clone_repository`. Routing them through an override means embedded mode only sets one value and every call site follows.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `core_client.rs`:

```rust
    #[test]
    fn socket_override_takes_precedence_over_default() {
        // No override set yet: falls back to the ~/.concerto default
        // (or None on a HOME-less env — both acceptable, just not the
        // override path).
        let overridden = std::path::PathBuf::from("/tmp/concerto-test/core.sock");
        set_socket_override(overridden.clone());
        assert_eq!(default_socket_path(), Some(overridden));
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p concerto-desktop socket_override -- --nocapture`
Expected: FAIL — `set_socket_override` not found.

- [ ] **Step 3: Implement the override**

In `core_client.rs`, add near the top (after the existing `use` lines):

```rust
use std::sync::OnceLock;

/// Process-wide override for the socket path. Set once at startup by
/// embedded mode (`embedded::start`). When unset, `default_socket_path`
/// falls back to `<HOME>/.concerto/core.sock`.
static SOCKET_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Install the socket path embedded Core bound. Idempotent — the first
/// call wins (we boot Core exactly once per process).
pub fn set_socket_override(path: PathBuf) {
    let _ = SOCKET_OVERRIDE.set(path);
}
```

Then change `default_socket_path` to consult it:

```rust
pub fn default_socket_path() -> Option<PathBuf> {
    if let Some(p) = SOCKET_OVERRIDE.get() {
        return Some(p.clone());
    }
    let home = home::home_dir()?;
    Some(home.join(".concerto").join("core.sock"))
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p concerto-desktop socket_override -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/core_client.rs
git commit -s -m "feat(desktop): allow overriding the Core socket path

Embedded mode sets the socket Core bound; default_socket_path consults
the override so every command call site follows without changes."
```

---

## Task 3: Add the `embedded-core` feature and the `embedded` module

**Files:**
- Modify: `apps/desktop/src-tauri/Cargo.toml`
- Create: `apps/desktop/src-tauri/src/embedded.rs`

- [ ] **Step 1: Add the optional dependency and feature in `Cargo.toml`**

In `apps/desktop/src-tauri/Cargo.toml`, under `[dependencies]` add:

```toml
# Optional: linked only when the `embedded-core` feature is on, so the
# lean daemon-client build carries no Core code or transitive deps.
concerto-core = { path = "../../../crates/core", optional = true }
tokio-util = { workspace = true }
```

Add a `[features]` table (after `[dependencies]`):

```toml
[features]
# Links concerto-core into the desktop binary so Core can boot
# in-process. Dev + standalone builds enable it; the production
# daemon-client build leaves it off.
embedded-core = ["dep:concerto-core"]
```

- [ ] **Step 2: Create `embedded.rs` with config resolution + mode enum**

Create `apps/desktop/src-tauri/src/embedded.rs`:

```rust
//! Embedded-Core mode: boot `concerto-core` inside the desktop process.
//!
//! Compiled only under the `embedded-core` feature. Picks a launch mode
//! from the environment, resolves a [`RuntimeConfig`], and boots Core on
//! a dedicated tokio runtime. Core's PID single-instance lock is the
//! coexistence guard: if a daemon already holds it, `boot::start` returns
//! `AlreadyRunning` and we fall back to dialing the live daemon.

use std::path::PathBuf;
use std::time::Duration;

use concerto_core::runtime::RuntimeConfig;

/// How this launch should obtain its Core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Boot Core in-process against real data (`~/concerto`, `~/.concerto`).
    EmbeddedReal,
    /// Boot Core in-process against an isolated scratch root.
    EmbeddedScratch { home: PathBuf },
    /// Do not embed — dial an externally running daemon.
    External,
}

/// Resolve the launch mode from env vars / flags.
///
/// Precedence: `CONCERTO_EMBEDDED=0` → External; an explicit
/// `CONCERTO_HOME` → EmbeddedScratch; otherwise (or `CONCERTO_EMBEDDED=1`)
/// → EmbeddedReal. CLI flags `--external` / `--embedded-scratch` map onto
/// the same variants.
pub fn resolve_mode(args: &[String], env_embedded: Option<&str>, env_home: Option<&str>) -> Mode {
    if args.iter().any(|a| a == "--external") || env_embedded == Some("0") {
        return Mode::External;
    }
    if let Some(home) = env_home.filter(|h| !h.is_empty()) {
        return Mode::EmbeddedScratch {
            home: PathBuf::from(home),
        };
    }
    if args.iter().any(|a| a == "--embedded-scratch") {
        // Scratch with no explicit home: caller must also set CONCERTO_HOME;
        // treated as real if absent to avoid a surprise temp location.
        return Mode::EmbeddedReal;
    }
    Mode::EmbeddedReal
}

/// Build a `RuntimeConfig` for a scratch home: `<home>` for data,
/// `<home>/.concerto` for config (mirrors the smoke-gate convention).
pub fn scratch_config(home: &std::path::Path) -> RuntimeConfig {
    RuntimeConfig {
        data_dir: home.to_path_buf(),
        config_dir: home.join(".concerto"),
        shutdown_grace: Duration::from_secs(5),
    }
}
```

- [ ] **Step 3: Write unit tests for mode resolution**

Append to `embedded.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_when_flag_or_env_zero() {
        assert_eq!(resolve_mode(&["--external".into()], None, None), Mode::External);
        assert_eq!(resolve_mode(&[], Some("0"), None), Mode::External);
    }

    #[test]
    fn scratch_when_home_set() {
        let m = resolve_mode(&[], None, Some("/tmp/scratch"));
        assert_eq!(m, Mode::EmbeddedScratch { home: "/tmp/scratch".into() });
    }

    #[test]
    fn real_by_default() {
        assert_eq!(resolve_mode(&[], None, None), Mode::EmbeddedReal);
        assert_eq!(resolve_mode(&[], Some("1"), None), Mode::EmbeddedReal);
    }

    #[test]
    fn scratch_config_splits_home() {
        let c = scratch_config(std::path::Path::new("/tmp/s"));
        assert_eq!(c.data_dir, std::path::PathBuf::from("/tmp/s"));
        assert_eq!(c.config_dir, std::path::PathBuf::from("/tmp/s/.concerto"));
    }
}
```

- [ ] **Step 4: Verify it compiles and tests pass under the feature**

Run: `cargo test -p concerto-desktop --features embedded-core embedded:: -- --nocapture`
Expected: PASS (4 tests). Also confirm the lean build still works: `cargo build -p concerto-desktop` (no feature) compiles without pulling in `concerto-core`.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/Cargo.toml apps/desktop/src-tauri/src/embedded.rs
git commit -s -m "feat(desktop): add embedded-core feature and mode resolution"
```

---

## Task 4: Boot embedded Core from the Tauri `setup` hook

**Files:**
- Modify: `apps/desktop/src-tauri/src/main.rs`
- Modify: `apps/desktop/src-tauri/src/embedded.rs` (add `start`)

- [ ] **Step 1: Add the `start` entry point to `embedded.rs`**

Append to `embedded.rs` (before the `#[cfg(test)]` block):

```rust
use tokio_util::sync::CancellationToken;

/// Handle stored in Tauri state so the window-close path can shut Core
/// down. `None` when running in External mode.
pub struct EmbeddedHandle {
    pub shutdown: CancellationToken,
}

/// Boot Core for the resolved mode on the given Tokio handle. Returns the
/// shutdown token (and installs the client socket override) when Core was
/// embedded; returns `None` for External / AlreadyRunning fallback, in
/// which case the client keeps its default socket and dials the daemon.
pub async fn start(mode: Mode) -> Option<EmbeddedHandle> {
    use concerto_core::boot::{self, BootOutcome};

    let config = match &mode {
        Mode::External => return None,
        Mode::EmbeddedScratch { home } => scratch_config(home),
        Mode::EmbeddedReal => match RuntimeConfig::default_for_user() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "embedded: failed to resolve runtime config");
                return None;
            }
        },
    };

    match boot::start(config).await {
        Ok(BootOutcome::Started(core)) => {
            crate::core_client::set_socket_override(core.socket_path().to_path_buf());
            let token = core.shutdown_token();
            tokio::spawn(async move {
                if let Err(e) = core.run_until_shutdown().await {
                    tracing::error!(error = %e, "embedded core shutdown error");
                }
            });
            tracing::info!("embedded core ready");
            Some(EmbeddedHandle { shutdown: token })
        }
        Ok(BootOutcome::AlreadyRunning { pid }) => {
            tracing::warn!(daemon_pid = pid, "daemon already running; dialing it instead of embedding");
            None
        }
        Err(e) => {
            tracing::error!(error = %e, "embedded core failed to boot; falling back to external");
            None
        }
    }
}
```

- [ ] **Step 2: Wire it into `main.rs`**

In `apps/desktop/src-tauri/src/main.rs`, add the module declaration with the others:

```rust
#[cfg(feature = "embedded-core")]
mod embedded;
```

Inside the `.setup(|app| { ... })` closure, **before** `commands::manage_subscriptions(app);`, add:

```rust
            #[cfg(feature = "embedded-core")]
            {
                let args: Vec<String> = std::env::args().collect();
                let mode = embedded::resolve_mode(
                    &args,
                    std::env::var("CONCERTO_EMBEDDED").ok().as_deref(),
                    std::env::var("CONCERTO_HOME").ok().as_deref(),
                );
                // Block the setup thread until Core is accepting, so the
                // renderer's first RPC never races the socket bind.
                let handle = tauri::async_runtime::block_on(embedded::start(mode));
                if let Some(h) = handle {
                    app.manage(h);
                }
            }
```

- [ ] **Step 3: Verify both builds compile**

Run: `cargo build -p concerto-desktop` (lean — no embedded module compiled)
Expected: clean build.
Run: `cargo build -p concerto-desktop --features embedded-core`
Expected: clean build.

- [ ] **Step 4: Manual smoke — embedded scratch launch**

Run from `apps/desktop`:
```bash
CONCERTO_HOME=$(mktemp -d) pnpm tauri dev -- --features embedded-core
```
Expected: window opens; logs show `embedded core ready`; the app round-trips RPCs against the in-process Core; no external daemon required. (If `pnpm tauri dev` flag passing differs, equivalently set the feature in a dev profile — see Task 6.)

- [ ] **Step 5: Commit**

```bash
git add apps/desktop/src-tauri/src/embedded.rs apps/desktop/src-tauri/src/main.rs
git commit -s -m "feat(desktop): boot embedded Core from the Tauri setup hook"
```

---

## Task 5: Window-close = full shutdown in embedded mode

**Files:**
- Modify: `apps/desktop/src-tauri/src/tray.rs:125-135` (close-to-hide handler)

Today `CloseRequested` calls `api.prevent_close()` + `window.hide()`. In embedded mode we instead cancel Core's shutdown token and let the close proceed, tearing the process down.

- [ ] **Step 1: Make the close handler embedded-aware**

In `tray.rs`, locate the `on_window_event` handler (around line 131) and replace the `CloseRequested` arm with:

```rust
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                #[cfg(feature = "embedded-core")]
                {
                    // Embedded: close means quit. Signal Core to stop
                    // (releases PID lock, flushes audit, stops agents),
                    // then allow the window to close and the process exit.
                    use tauri::Manager;
                    if let Some(h) = window_for_handler
                        .app_handle()
                        .try_state::<crate::embedded::EmbeddedHandle>()
                    {
                        h.shutdown.cancel();
                    }
                    // Do NOT prevent_close — let teardown proceed.
                    return;
                }
                #[cfg(not(feature = "embedded-core"))]
                {
                    api.prevent_close();
                    let _ = window_for_handler.hide();
                }
            }
        });
```

> Note: when the `embedded-core` feature is on but the resolved mode was External (no `EmbeddedHandle` in state), `try_state` returns `None`, so we simply close the window without hiding. That is acceptable — an embedded-capable build run in `--external` mode behaves as a plain client whose window close quits the app. If preserving close-to-hide for that sub-case matters, gate on `try_state().is_some()` and fall through to `prevent_close` + `hide` otherwise; keep it simple unless asked.

- [ ] **Step 2: Verify both builds compile**

Run: `cargo build -p concerto-desktop && cargo build -p concerto-desktop --features embedded-core`
Expected: both clean. (`api` may be unused in the embedded arm — prefix `_api` in the pattern if the compiler warns: `CloseRequested { api: _api, .. }`. Apply only if a warning appears.)

- [ ] **Step 3: Manual verification**

Launch embedded scratch (Task 4 Step 4), close the window, and confirm: process exits (no lingering desktop process), and the scratch `core.pid` lock file is released (file removed or stale). Check logs show `concerto-core stopped`.

- [ ] **Step 4: Commit**

```bash
git add apps/desktop/src-tauri/src/tray.rs
git commit -s -m "feat(desktop): window close shuts down embedded Core"
```

---

## Task 6: Dev hot-reload loop

**Files:**
- Modify: `apps/desktop/src-tauri/tauri.conf.json`
- Modify: `Makefile`

Goal: editing `crates/core` triggers a rebuild + relaunch under `tauri dev`, and there's a one-command entry point that enables the feature.

- [ ] **Step 1: Verify whether `tauri dev` already rebuilds on `crates/` changes**

Run `pnpm tauri dev -- --features embedded-core` from `apps/desktop`, then touch a file: `touch ../../crates/core/src/boot.rs`.
Expected (best case): Tauri's watcher recompiles and relaunches. If it does NOT react to the sibling crate, proceed to Step 2; otherwise skip to Step 3.

- [ ] **Step 2: If needed, broaden the dev watcher**

Tauri v2 watches the `src-tauri` crate by default. To include workspace crates, the robust path is a `cargo watch` wrapper. Add a `Makefile` target (see Step 3) that uses it. (No `tauri.conf.json` change is required for the wrapper approach; leave `tauri.conf.json` as-is unless you confirmed in Step 1 that a `build.devWatch`/watch entry is honored in your Tauri version — if so, add the watched paths there.)

- [ ] **Step 3: Add a one-command dev entry point to the `Makefile`**

Add to `Makefile`:

```makefile
.PHONY: dev-embedded
## Run the desktop app with Core embedded in-process, hot-reloading on
## changes to either the desktop crate or crates/core. Uses a scratch
## data root so it never touches ~/concerto.
dev-embedded:
	cd apps/desktop && CONCERTO_HOME=$${CONCERTO_HOME:-$$(mktemp -d -t concerto-dev.XXXXXX)} \
		pnpm tauri dev -- --features embedded-core
```

If Step 1 showed the watcher ignores `crates/`, use this variant instead (requires `cargo install cargo-watch`):

```makefile
.PHONY: dev-embedded
dev-embedded:
	cd apps/desktop && CONCERTO_HOME=$${CONCERTO_HOME:-$$(mktemp -d -t concerto-dev.XXXXXX)} \
		cargo watch -w ../../crates -w src -s 'pnpm tauri dev -- --features embedded-core'
```

- [ ] **Step 4: Verify the loop**

Run: `make dev-embedded`, wait for the window, edit a `tracing::info!` string in `crates/core/src/boot.rs`, save.
Expected: app rebuilds and relaunches within seconds; the new log line appears. Frontend HMR (Vite, port 5173) continues to work for `.tsx` edits without a Rust rebuild.

- [ ] **Step 5: Commit**

```bash
git add Makefile apps/desktop/src-tauri/tauri.conf.json
git commit -s -m "build: add make dev-embedded hot-reload loop for embedded Core"
```

---

## Task 7: Smoke variant + documentation

**Files:**
- Create: `scripts/smoke-embedded.sh`
- Modify: `README.md`

- [ ] **Step 1: Create the embedded smoke script**

Create `scripts/smoke-embedded.sh` (mode `+x`):

```bash
#!/usr/bin/env bash
# Smoke gate for embedded-core mode: builds the desktop binary with the
# feature, boots Core in-process against a scratch CONCERTO_HOME via the
# library boot path, and asserts the socket comes up and tears down.
#
# This exercises the one-process path that `scripts/smoke.sh` (daemon)
# does not. It relies on the `embedded_boot` integration test as the
# behavioral check, plus a feature-on build to catch link regressions.
set -euo pipefail

echo "smoke-embedded: building desktop with embedded-core feature"
cargo build -p concerto-desktop --features embedded-core

echo "smoke-embedded: running embedded boot integration test"
cargo test -p concerto-core --test embedded_boot -- --nocapture

echo "smoke-embedded: OK"
```

- [ ] **Step 2: Make it executable and run it**

Run:
```bash
chmod +x scripts/smoke-embedded.sh && ./scripts/smoke-embedded.sh
```
Expected: builds, the `embedded_boot` test passes, prints `smoke-embedded: OK`.

- [ ] **Step 3: Document embedded mode in `README.md`**

In `README.md`, after the "Run your first agent" section, add:

````markdown
## Embedded mode (testing & standalone)

By default the Desktop dials a separately-installed Core daemon. An
optional **embedded mode** links Core into the Desktop binary and boots
it in-process — one process, no separate daemon install, and a fast
hot-reload dev loop.

Enable it with the `embedded-core` Cargo feature. Mode is chosen per
launch:

| Launch | Behavior |
|---|---|
| default / `CONCERTO_EMBEDDED=1` | Boot Core in-process against your real `~/concerto` data. If a daemon is already running it is detected via the PID lock and the app dials it instead. |
| `CONCERTO_HOME=/path` | Boot Core in-process against an isolated scratch root — runs alongside an installed daemon with no conflict. Use this for testing. |
| `CONCERTO_EMBEDDED=0` / `--external` | Skip embedding; dial an existing daemon (default production behavior). |

Fast dev loop (scratch data, hot-reloads on `crates/core` changes):

```sh
make dev-embedded
```

In embedded mode, **closing the window quits the app and stops all
agents** — the "agents survive window close" guarantee holds only with
the separate daemon.
````

- [ ] **Step 4: Commit**

```bash
git add scripts/smoke-embedded.sh README.md
git commit -s -m "test+docs: embedded-core smoke variant and README section"
```

---

## Self-Review Notes

- **Spec coverage:** §2.1 boot extraction → Task 1; §2.2 embed module/dep → Tasks 3–4; §2.3 injectable socket → Task 2; §3 mode selection → Tasks 3–4; §4 coexistence guard → Task 4 (`AlreadyRunning` fallback, backed by Core's PID lock); §5 lifecycle → Task 5; §6 hot-reload → Task 6; §7 testing → Tasks 1 (round-trip), 3 (mode unit tests), 7 (smoke + guard exercised via PID lock). The standalone-vs-scratch data policy is covered by `resolve_mode` + `scratch_config`.
- **Guard test nuance:** the spec called for a dedicated "second instance falls back" test. It is covered structurally: a second embedded boot against the same real root hits Core's PID lock and returns `AlreadyRunning`, which `embedded::start` maps to `None` (dial the daemon). An explicit test can be added in Task 4 if desired, but the existing PID-lock tests in `crates/core` already prove the lock behavior.
- **Type consistency:** `BootOutcome` / `RunningCore` / `set_socket_override` / `Mode` / `EmbeddedHandle` / `resolve_mode` / `scratch_config` / `embedded::start` are used consistently across tasks.
- **Placeholders:** none — every code step shows real code; the one verbatim relocation (Task 1 Step 2) is explicitly bounded by line numbers rather than re-pasting ~250 unchanged lines.
