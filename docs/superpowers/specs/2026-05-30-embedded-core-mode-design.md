# Embedded-Core mode for the Desktop app

**Status:** Design approved — ready for implementation planning
**Date:** 2026-05-30
**Branch:** `kill-port-5173-error`

## Problem

Concerto ships as two separately-installed processes: the `concerto-core`
daemon (a launchd LaunchAgent) and the `concerto-desktop` Tauri app, which
dials Core over a Unix domain socket at `~/.concerto/core.sock`. This is
correct for production (agents survive window close, state is durable), but
it imposes friction in two situations:

1. **Testing / iteration.** To exercise a Core change you must rebuild the
   daemon, reinstall/restart the LaunchAgent, then launch the Desktop. There
   is no fast edit→run loop for Core code.
2. **Standalone installation.** A new user must install and manage two
   things. There is no single self-contained app.

We want an **optional** mode that collapses Core and the Desktop into one
process, gives a fast hot-reload dev loop for Core code, and enables a
single-artifact standalone install — without disturbing the existing
two-process production architecture.

## Decisions (locked during brainstorming)

| # | Decision | Choice |
|---|----------|--------|
| 1 | Meaning of "hot reload" | **Dev rebuild-restart loop** (`tauri dev` recompiles + relaunches on Rust changes). Not live runtime code-swap. |
| 2 | How the mode is selected | **Cargo feature (availability) + runtime flag (per-launch in-process vs external).** |
| 3 | Data dir / socket & daemon coexistence | **Real data + guard for standalone; isolated scratch dir for testing.** Both via env-var overrides Core already supports. |
| 4 | Window-close lifecycle | **Window close = full shutdown** of embedded Core (and its agents). The "agents survive window close" promise holds only in external mode. |

## Architecture

Embedded mode reuses the **exact same gRPC-over-UDS transport**. It starts
Core inside the desktop process on a dedicated tokio runtime, lets Core bind
its UDS as usual, and points the existing client at that socket. No new
transport, no in-memory channel.

```
External mode (today):   Desktop ──UDS──► [launchd] concerto-core
Embedded mode (new):     Desktop process { tokio rt → Core actors + gRPC on UDS } ◄──UDS── same Desktop's client
```

The renderer, `commands.rs`, and the bulk of `core_client.rs` are unchanged.
The only client-side change is *which socket path* is dialed.

## Components & changes

### 1. `crates/core` — extract boot into the library (the one nontrivial refactor)

The entire boot orchestration (spawning ~10 supervised actors + the gRPC
server) currently lives in `crates/core/src/main.rs::run()` — in the binary,
not the library. The library (`crates/core/src/lib.rs`) already exposes every
actor module publicly, so this is a mechanical extraction:

- New module `concerto_core::boot` exposing:
  - `async fn start(config: RuntimeConfig) -> Result<RunningCore>` — the body
    of today's `run()` from `Runtime::start` through "concerto-core ready",
    returning a handle. Returns early/distinctly on the `AlreadyRunning`
    single-instance outcome so callers can react (see guard).
  - `struct RunningCore` holding the `CancellationToken` / shutdown handle and
    the bound socket path, with `async fn shutdown(self)` wrapping
    `Runtime::stop`.
- `main.rs` shrinks to: init logging → build tokio runtime → `boot::start` →
  `Runtime::wait_for_shutdown` → `running.shutdown()`.

**Behavior change for the daemon: none.** Same code path, relocated. Existing
Core integration tests (`grpc_runtime.rs`, etc.) must pass unchanged — that is
the proof of equivalence.

### 2. `apps/desktop/src-tauri` — optional dependency + embed module

- `Cargo.toml`:
  - `concerto-core = { path = "../../../crates/core", optional = true }`
  - `[features] embedded-core = ["dep:concerto-core"]`
- New `src/embedded.rs`, compiled only under `#[cfg(feature = "embedded-core")]`:
  - Resolves `RuntimeConfig` honoring `CONCERTO_DATA_DIR` / `CONCERTO_CONFIG_DIR`
    / `CONCERTO_HOME`.
  - Runs the coexistence guard (see below).
  - Boots Core on a dedicated multi-thread tokio runtime owned for the process
    lifetime, and returns the resolved socket path + a shutdown handle.
- `main.rs`: in the Tauri `setup` hook, if embedded mode is selected at
  runtime, call `embedded::start()` **before** the renderer issues any RPC,
  and stash the socket path + shutdown handle in Tauri-managed state.

### 3. `core_client.rs` — make the socket path injectable

Currently `get_or_connect` hardcodes `default_socket_path()`
(`~/.concerto/core.sock`). Change it to read the path from a process-wide
`OnceLock<PathBuf>` set once at startup, defaulting to the current value.
Embedded mode sets it to Core's bound socket. Surgical change; external mode
behavior identical.

## Mode selection

- **Cargo feature `embedded-core`** gates whether Core is linked at all. The
  lean daemon-client build omits it → no Core code or transitive deps, smallest
  binary. Dev and standalone builds enable it.
- **Runtime flag** (only meaningful when the feature is compiled in) chooses
  per-launch:
  - default / `--embedded` / `CONCERTO_EMBEDDED=1` → boot Core in-process
    against real data, with the guard.
  - `--embedded-scratch` (or an explicit `CONCERTO_HOME=/tmp/...`) → in-process
    with an isolated data root (testing).
  - `--external` / `CONCERTO_EMBEDDED=0` → skip embedding, dial an existing
    daemon (today's behavior).
- The standalone installer ships the feature-on build defaulting to embedded.
  Dev builds flip between modes without recompiling.

## Coexistence guard

Before booting Core in-process against **real** data, check Core's existing
`PidFile` lock and probe the socket:

- **Daemon already live** → do not embed (would hit the single-instance guard
  / socket-bind conflict). Fall back to dialing the existing daemon and surface
  a clear notice to the renderer.
- **No daemon** → embed normally; this process *becomes* the daemon for the
  session.
- **Scratch mode** → isolated `CONCERTO_HOME` with its own DB + socket; no
  guard needed, runs alongside an installed daemon with zero conflict. This is
  the testing path.

This relies entirely on the env-var overrides `RuntimeConfig` already supports
plus the existing PID guard — no new locking machinery.

## Lifecycle (window close = full shutdown)

Closing the window tears down embedded Core: trigger the `CancellationToken`,
await `Runtime::stop` (releases the PID lock, flushes the audit log, stops
agents), then exit the process. Wired through Tauri's
`WindowEvent::CloseRequested` / exit path, overriding the existing
close-to-hide tray behavior (Task 48) **only when embedded**. External mode
keeps close-to-hide. In embedded mode, agents do not survive window close —
this is the documented, expected tradeoff for a self-contained app.

## Hot-reload dev loop

Because Core is linked into the desktop binary under the feature, `pnpm tauri
dev` rebuilds and relaunches the whole app when any compiled Rust changes —
Cargo recompiles changed path-dependencies including `crates/core`. Verify
Tauri v2's dev watcher covers the workspace `crates/` directory; if it only
watches `src-tauri/`, the fallback is a `cargo watch -w crates -w apps`
wrapper or a `devWatch` entry in `tauri.conf.json`. Frontend HMR (Vite, port
5173) is unaffected. Result: edit Core code → ~seconds → app back up on the
freshly-built embedded Core.

## Testing

- **Extraction safety:** existing Core integration tests pass unchanged,
  proving the relocated boot is behavior-identical.
- **Embedded round-trip (new, feature-gated):** desktop-crate integration test
  calls `embedded::start()` against a scratch `CONCERTO_HOME`, dials the bound
  socket, round-trips `Runtime.GetServerCapabilities`, shuts down cleanly, and
  asserts the PID lock is released.
- **Guard test:** boot one embedded Core, attempt a second against the same
  real data root, assert it detects the live instance and falls back rather
  than corrupting state or crashing.
- **Smoke gate:** add an embedded-scratch variant so CI exercises the
  one-process path end to end.

## Non-goals (YAGNI)

- Live runtime code-swap without restart (decision 1 → A).
- Close-to-tray persistence for embedded mode (decision 4 → A); can be layered
  on later if standalone users want daemon-like survival.
- Windows/Linux specifics — V0.1 is macOS-only; embedded mode follows the same
  `#[cfg(unix)]` boundaries Core already uses.
- Any change to the production two-process architecture or its install/upgrade
  flow.

## Risks

- **Boot extraction regressions.** Mitigated by keeping `run()` a thin caller
  of `boot::start` and relying on the existing integration suite.
- **Tauri dev watcher scope.** May not watch sibling workspace crates by
  default; fallback documented above.
- **Binary size / build time of the feature-on build.** Accepted — the lean
  daemon-client build (feature off) is unaffected.
