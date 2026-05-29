#!/usr/bin/env bash
# shellcheck shell=bash
#
# uninstall-macos.sh — remove the concerto-core LaunchAgent and binary.
#
# Default behavior:
#   - launchctl bootout gui/<uid>/com.concerto.core (best-effort).
#   - Remove ~/Library/LaunchAgents/com.concerto.core.plist.
#   - Remove the installed binary at ~/Applications/concerto/concerto-core.
#
# With --purge:
#   - Additionally removes ~/concerto/ (data + logs) and ~/.concerto/
#     (config + keychain shadow). This is destructive; ask before running
#     it on a machine with real project state.
#
# Idempotent: safe to re-run; missing files / unloaded agents are not
# treated as errors.
#
# Bash 3.2 compatible.

set -euo pipefail

# ---------------------------------------------------------------------------
# Platform guard
# ---------------------------------------------------------------------------
if [ "$(uname -s)" != "Darwin" ]; then
    echo "uninstall-macos.sh: this script is macOS-only (uname=$(uname -s))" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Args
# ---------------------------------------------------------------------------
PURGE=0
for arg in "$@"; do
    case "$arg" in
        --purge)
            PURGE=1
            ;;
        -h|--help)
            echo "Usage: $0 [--purge]"
            echo ""
            echo "  --purge    Also remove ~/concerto/ and ~/.concerto/ (destructive)."
            exit 0
            ;;
        *)
            echo "uninstall-macos.sh: unknown argument: $arg" >&2
            exit 1
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
INSTALL_DIR="$HOME/Applications/concerto"
BIN_PATH="$INSTALL_DIR/concerto-core"
LAUNCH_AGENT_PATH="$HOME/Library/LaunchAgents/com.concerto.core.plist"

SERVICE_LABEL="com.concerto.core"
UID_NUM="$(id -u)"
SERVICE_TARGET="gui/$UID_NUM/$SERVICE_LABEL"

# ---------------------------------------------------------------------------
# 1. Unload service
# ---------------------------------------------------------------------------
echo "==> Unloading LaunchAgent ($SERVICE_TARGET)"
# bootout returns non-zero when the service isn't loaded; that's fine.
launchctl bootout "$SERVICE_TARGET" 2>/dev/null || true

# ---------------------------------------------------------------------------
# 2. Remove plist + binary
# ---------------------------------------------------------------------------
if [ -f "$LAUNCH_AGENT_PATH" ]; then
    echo "==> Removing $LAUNCH_AGENT_PATH"
    rm -f "$LAUNCH_AGENT_PATH"
fi

if [ -f "$BIN_PATH" ]; then
    echo "==> Removing $BIN_PATH"
    rm -f "$BIN_PATH"
fi

# Clean up the install dir if it ended up empty.
if [ -d "$INSTALL_DIR" ]; then
    rmdir "$INSTALL_DIR" 2>/dev/null || true
fi

# ---------------------------------------------------------------------------
# 3. Optional purge
# ---------------------------------------------------------------------------
if [ "$PURGE" -eq 1 ]; then
    echo "==> Purging Concerto data + config dirs"
    rm -rf "$HOME/concerto"
    rm -rf "$HOME/.concerto"
fi

echo "==> Uninstalled."
