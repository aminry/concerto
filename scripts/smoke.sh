#!/usr/bin/env bash
# Concerto smoke gate — the layer-2 verification backstop described in
# tasks/README.md §5.
#
# Responsibilities (grow over the build):
#   - Phase 1 (Task 15): Core boots, Desktop connects via UDS, GetCapabilities
#     round-trips, both shut down cleanly.
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
if [ -z "${CONCERTO_HOME:-}" ]; then
    CONCERTO_HOME=$(mktemp -d -t concerto-smoke.XXXXXX)
    export CONCERTO_HOME
    trap 'rm -rf "$CONCERTO_HOME"' EXIT
else
    export CONCERTO_HOME
fi

echo "Smoke gate: starting (CONCERTO_HOME=$CONCERTO_HOME)"

# Phase 1 checks — added in Task 15
# Phase 2 checks — added in Task 27
# Phase 3 checks — added in Tasks 42 + 44
# Phase 4 checks — added in Task 52

echo "Smoke gate: PASSED (no checks active yet — Phase 0)"
