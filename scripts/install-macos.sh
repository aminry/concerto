#!/usr/bin/env bash
# shellcheck shell=bash
#
# install-macos.sh — install concerto-core as a per-user macOS LaunchAgent.
#
# What this does:
#   1. Builds concerto-core in release mode.
#   2. Installs the binary to ~/Applications/concerto/concerto-core
#      (per-user, no sudo required).
#   3. Templates dist/macos/com.concerto.core.plist with the absolute
#      binary path and the user's $HOME, writing the rendered plist to
#      ~/Library/LaunchAgents/com.concerto.core.plist.
#   4. Reloads the agent via `launchctl bootout` (best-effort) then
#      `launchctl bootstrap` — the modern replacements for the deprecated
#      `launchctl load` / `unload`.
#
# Idempotency: re-running this script is safe. `bootout` is allowed to
# fail (e.g. when the agent was never loaded); `bootstrap` then loads the
# refreshed plist.
#
# Bash 3.2 compatible (default macOS /bin/bash).

set -euo pipefail

# ---------------------------------------------------------------------------
# Platform guard
# ---------------------------------------------------------------------------
if [ "$(uname -s)" != "Darwin" ]; then
    echo "install-macos.sh: this script is macOS-only (uname=$(uname -s))" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

PLIST_TEMPLATE="$REPO_ROOT/dist/macos/com.concerto.core.plist"
INSTALL_DIR="$HOME/Applications/concerto"
BIN_PATH="$INSTALL_DIR/concerto-core"
HOST_BIN_PATH="$INSTALL_DIR/concerto-agent-host"
LAUNCH_AGENT_DIR="$HOME/Library/LaunchAgents"
LAUNCH_AGENT_PATH="$LAUNCH_AGENT_DIR/com.concerto.core.plist"
LOG_DIR="$HOME/concerto/logs"

SERVICE_LABEL="com.concerto.core"
UID_NUM="$(id -u)"
SERVICE_TARGET="gui/$UID_NUM/$SERVICE_LABEL"
DOMAIN_TARGET="gui/$UID_NUM"

# ---------------------------------------------------------------------------
# Sanity checks
# ---------------------------------------------------------------------------
if [ ! -f "$PLIST_TEMPLATE" ]; then
    echo "install-macos.sh: missing plist template at $PLIST_TEMPLATE" >&2
    exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "install-macos.sh: cargo not found in PATH; install Rust toolchain first" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# 1. Build
# ---------------------------------------------------------------------------
echo "==> Building concerto-core + concerto-agent-host (release)"
(cd "$REPO_ROOT" && cargo build --release -p concerto-core -p concerto-agent-host)

BUILT_BIN="$REPO_ROOT/target/release/concerto-core"
BUILT_HOST_BIN="$REPO_ROOT/target/release/concerto-agent-host"
if [ ! -x "$BUILT_BIN" ]; then
    echo "install-macos.sh: build did not produce $BUILT_BIN" >&2
    exit 1
fi
if [ ! -x "$BUILT_HOST_BIN" ]; then
    # concerto-agent-host is a sibling binary the supervisor spawns per
    # session. Without it, every CreateSession fails with `spawn
    # agent-host: io: No such file or directory`.
    echo "install-macos.sh: build did not produce $BUILT_HOST_BIN" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# 2. Install binaries + log dir
# ---------------------------------------------------------------------------
echo "==> Installing binaries to $INSTALL_DIR"
mkdir -p "$INSTALL_DIR"
mkdir -p "$LOG_DIR"
cp -f "$BUILT_BIN" "$BIN_PATH"
chmod +x "$BIN_PATH"
cp -f "$BUILT_HOST_BIN" "$HOST_BIN_PATH"
chmod +x "$HOST_BIN_PATH"

# ---------------------------------------------------------------------------
# 3. Render plist
# ---------------------------------------------------------------------------
echo "==> Rendering LaunchAgent to $LAUNCH_AGENT_PATH"
mkdir -p "$LAUNCH_AGENT_DIR"

# sed -i differs between BSD/GNU; write to a tempfile and move atomically.
TMP_PLIST="$(mktemp -t com.concerto.core.plist.XXXXXX)"
trap 'rm -f "$TMP_PLIST"' EXIT

# Use a sed delimiter that won't appear in paths (|). Both substitutions
# are literal-string oriented; if a user has a "|" in $HOME they have
# bigger problems.
sed -e "s|__BIN_PATH__|$BIN_PATH|g" \
    -e "s|__HOME__|$HOME|g" \
    "$PLIST_TEMPLATE" > "$TMP_PLIST"

# Validate the rendered plist when plutil is available (it ships with
# macOS, so this should always run on the target platform).
if command -v plutil >/dev/null 2>&1; then
    plutil -lint "$TMP_PLIST" >/dev/null
fi

mv -f "$TMP_PLIST" "$LAUNCH_AGENT_PATH"
trap - EXIT

# ---------------------------------------------------------------------------
# 4. Reload via launchctl
# ---------------------------------------------------------------------------
echo "==> Reloading LaunchAgent ($SERVICE_TARGET)"

# bootout fails if the service isn't already loaded; that's fine on a
# clean machine. Suppress the error to keep the script idempotent.
launchctl bootout "$SERVICE_TARGET" 2>/dev/null || true

launchctl bootstrap "$DOMAIN_TARGET" "$LAUNCH_AGENT_PATH"

echo "==> Installed."
echo "    Binary:       $BIN_PATH"
echo "    Agent host:   $HOST_BIN_PATH"
echo "    LaunchAgent:  $LAUNCH_AGENT_PATH"
echo "    Logs:         $LOG_DIR/launchd-{out,err}.log"
echo ""
echo "Verify with:"
echo "    launchctl print $SERVICE_TARGET"
