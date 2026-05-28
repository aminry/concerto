#!/usr/bin/env bash
# Concerto smoke gate — the layer-2 verification backstop described in
# tasks/README.md §5.
#
# Responsibilities (grow over the build):
#   - Phase 1 (Task 15): Core boots, smoke-client connects via UDS,
#     Runtime.GetServerCapabilities round-trips, Core shuts down cleanly.
#   - Phase 2 (Task 27): create a workspace from a local git repo, spawn a
#     claude session, see output stream to Desktop, kill Core, restart Core,
#     reconnect to same session, output continues.
#   - Phase 3 (Tasks 42 + 44): permission modes, audit log presence, /loop.
#   - Phase 4 (Task 52): full V0.1 happy-path scenario.
#
# Contract:
#   - Exit 0 = pass, non-zero = fail. Output is human-readable.
#   - CONCERTO_HOME points to a tempdir for the duration of the script.
#     Tasks must not rely on the literal path ~/concerto/.
#   - Linux/macOS only in V0.1; Windows port (scripts/smoke.ps1) is V1.0.

set -euo pipefail

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib/common.sh
. "$(dirname "$0")/lib/common.sh"

# Only manage CONCERTO_HOME ourselves if the caller didn't pre-set it; that
# way an externally-provided directory isn't rm -rf'd on exit.
OWNS_HOME=0
if [ -z "${CONCERTO_HOME:-}" ]; then
    CONCERTO_HOME=$(mktemp -d -t concerto-smoke.XXXXXX)
    export CONCERTO_HOME
    OWNS_HOME=1
else
    export CONCERTO_HOME
fi

CORE_PID=""

# cleanup runs on every exit path (success, fail, signal). It must be
# idempotent because the trap fires once, but each step may have already
# happened in the happy path.
cleanup() {
    if [ -n "$CORE_PID" ]; then
        # Best-effort SIGTERM; if the core already exited cleanly the
        # kill is a no-op (returns non-zero, swallowed).
        kill -TERM "$CORE_PID" 2>/dev/null || true
        wait "$CORE_PID" 2>/dev/null || true
        CORE_PID=""
    fi
    if [ "$OWNS_HOME" -eq 1 ] && [ -n "${CONCERTO_HOME:-}" ]; then
        rm -rf "$CONCERTO_HOME"
    fi
}
trap cleanup EXIT INT TERM

echo "Smoke gate v1: starting (CONCERTO_HOME=$CONCERTO_HOME)"

# ---------------------------------------------------------------------------
# Phase 1 — Core boot + UDS + GetServerCapabilities + clean shutdown.
# ---------------------------------------------------------------------------

CORE_CONFIG_DIR="$CONCERTO_HOME/.concerto"
CORE_DATA_DIR="$CONCERTO_HOME/concerto"
mkdir -p "$CORE_CONFIG_DIR" "$CORE_DATA_DIR"

# Pre-build both binaries so `cargo run` doesn't slip a compile step into
# the wall clock. `--quiet` keeps the build noise out of smoke output;
# real errors still surface because cargo writes them to stderr.
echo "Smoke gate v1: building concerto-core and smoke-client..."
cargo build --quiet -p concerto-core -p concerto-smoke-client

echo "Smoke gate v1: starting concerto-core in background..."
CORE_LOG="$CONCERTO_HOME/core.log"
CONCERTO_CONFIG_DIR="$CORE_CONFIG_DIR" CONCERTO_DATA_DIR="$CORE_DATA_DIR" \
    cargo run --quiet --bin concerto-core > "$CORE_LOG" 2>&1 &
CORE_PID=$!

# Wait for the UDS socket to appear. Cap at 15s — longer than any
# reasonable cold start, short enough to fail CI fast when the core is
# wedged.
SOCKET="$CORE_CONFIG_DIR/core.sock"
if ! wait_for_file "$SOCKET" 15; then
    echo "smoke: core log:" >&2
    sed 's/^/    /' "$CORE_LOG" >&2 || true
    fail "core.sock not created within 15s"
fi
echo "Smoke gate v1: Core ready (socket: $SOCKET)"

# Call GetServerCapabilities and confirm the response advertises UDS.
echo "Smoke gate v1: calling Runtime.GetServerCapabilities..."
RESPONSE=$(cargo run --quiet -p concerto-smoke-client --bin smoke-client -- --socket "$SOCKET")
echo "Smoke gate v1: response: $RESPONSE"
if ! echo "$RESPONSE" | grep -q '"transport_kind": *"TRANSPORT_KIND_UDS"'; then
    echo "smoke: core log:" >&2
    sed 's/^/    /' "$CORE_LOG" >&2 || true
    fail "unexpected smoke-client output (missing TRANSPORT_KIND_UDS)"
fi

# Shut down cleanly: SIGTERM the core, wait for it to exit, verify the
# pid file was cleaned up.
echo "Smoke gate v1: shutting down Core..."
kill -TERM "$CORE_PID"
if ! wait "$CORE_PID"; then
    fail "core did not exit cleanly"
fi
# After successful join, clear CORE_PID so the EXIT trap doesn't re-kill.
CORE_PID=""

if [ -f "$CORE_CONFIG_DIR/core.pid" ]; then
    fail "core.pid not cleaned up at $CORE_CONFIG_DIR/core.pid"
fi
if [ -e "$SOCKET" ]; then
    fail "core.sock not cleaned up at $SOCKET"
fi

# Phase 2 checks — added in Task 27
# Phase 3 checks — added in Tasks 42 + 44
# Phase 4 checks — added in Task 52

echo "Smoke gate v1: PASSED"
