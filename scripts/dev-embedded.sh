#!/usr/bin/env bash
# shellcheck shell=bash
# Run the desktop app with Core embedded in-process, hot-reloading the
# frontend (Vite HMR), the src-tauri crate (Tauri's own watcher), AND
# crates/core (cargo watch, which restarts `tauri dev` on a Core change).
#
# Data root: real ~/concerto unless CONCERTO_HOME is set (the scratch
# variant sets it). Run `make stop-core` first if a standalone daemon is
# live, or embedded-real will detect the PID lock and dial it instead.
set -euo pipefail

# Preflight: cargo-watch must be installed.
if ! cargo watch --version >/dev/null 2>&1; then
    echo "dev-embedded: cargo-watch not found." >&2
    echo "  Install it with: cargo install cargo-watch" >&2
    exit 1
fi

# Warn (don't block) if a standalone daemon holds the lock in real-data mode.
if [ -z "${CONCERTO_HOME:-}" ]; then
    PID_FILE="${CONCERTO_CONFIG_DIR:-$HOME/.concerto}/core.pid"
    if [ -f "$PID_FILE" ]; then
        echo "dev-embedded: note — $PID_FILE exists; a standalone Core may be" >&2
        echo "  running. Run 'make stop-core' first so embedded mode boots" >&2
        echo "  in-process instead of dialing the daemon." >&2
    fi
fi

# Resolve the repo root from this script's own location so the paths below
# are absolute and don't depend on the caller's working directory.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$REPO_ROOT/apps/desktop"
# cargo watch watches crates/core AND crates/agent-host; Vite HMR +
# src-tauri rebuilds are handled by `tauri dev` itself. An edit to either
# watched crate restarts the whole dev session (rebuild + relaunch).
#
# Why build concerto-agent-host here: the embedded Core spawns the
# agent-host binary resolved by
# crates/core/src/agent_supervisor/spawn.rs::resolve_host_binary (Task 106:
# CONCERTO_AGENT_HOST_BIN override → co-located → target/<profile> sibling).
# In dev the desktop binary's directory is the workspace `target/debug/`,
# but `tauri dev` only compiles `concerto-desktop` — the agent-host binary
# is never produced unless we build it. We keep building it so it exists in
# target/debug, exactly as install-macos.sh does for the release install.
#
# CONCERTO_AGENT_HOST_BIN (belt-and-suspenders): we ALSO export the absolute
# path to the freshly built binary as the highest-precedence override, so
# resolution is correct by contract rather than by co-location accident. If
# resolution ever fails it now names this var and the paths it tried instead
# of surfacing a bare "Rpc" (the old "io: No such file or directory").
export CONCERTO_AGENT_HOST_BIN="$REPO_ROOT/target/debug/concerto-agent-host"
#
# Paths are absolute so they resolve regardless of the caller's CWD. `-C`
# sets the command's working directory to apps/desktop — cargo watch
# otherwise runs `-s` commands from the Cargo crate root (the repo root),
# where there's no package.json for pnpm. `-w` is absolute for the same
# canonicalization-independence.
exec cargo watch \
    -C "$REPO_ROOT/apps/desktop" \
    -w "$REPO_ROOT/crates/core" \
    -w "$REPO_ROOT/crates/agent-host" \
    -s 'cargo build -p concerto-agent-host && pnpm tauri dev -f embedded-core'
