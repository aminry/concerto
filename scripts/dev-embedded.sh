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
# cargo watch watches ONLY crates/core; Vite HMR + src-tauri rebuilds are
# handled by `tauri dev` itself. A crates/core edit restarts the whole
# dev session (full Core rebuild + relaunch). The watch path is absolute
# so it canonicalizes regardless of where the script was invoked from.
exec cargo watch -w "$REPO_ROOT/crates/core" -s 'pnpm tauri dev -f embedded-core'
