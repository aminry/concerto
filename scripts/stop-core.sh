#!/usr/bin/env bash
# shellcheck shell=bash
# Stop the standalone Concerto Core so embedded-real mode can take over.
#
#   1. launchctl bootout the LaunchAgent (best-effort; "not loaded" is fine).
#   2. If the PID lock still points at a live process (e.g. a bare,
#      directly-launched Core), SIGTERM it.
#
# macOS-only (launchd). Honors CONCERTO_CONFIG_DIR for the PID-file path.
set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
    echo "stop-core: macOS-only (launchd); nothing to do on $(uname -s)" >&2
    exit 0
fi

SERVICE_TARGET="gui/$(id -u)/com.concerto.core"
if launchctl bootout "$SERVICE_TARGET" 2>/dev/null; then
    echo "stop-core: stopped launchd service ($SERVICE_TARGET)"
else
    echo "stop-core: launchd service not loaded ($SERVICE_TARGET)"
fi

PID_FILE="${CONCERTO_CONFIG_DIR:-$HOME/.concerto}/core.pid"
if [ -f "$PID_FILE" ]; then
    # core.pid is JSON: {"pid":N,"version":"...","start_epoch_secs":N}.
    PID="$(sed -n 's/.*"pid"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$PID_FILE" | head -1)"
    if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
        kill "$PID"
        echo "stop-core: sent SIGTERM to bare Core process (pid $PID)"
    else
        echo "stop-core: PID lock present but no live process; nothing to kill"
    fi
fi

echo "stop-core: done"
