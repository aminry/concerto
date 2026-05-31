#!/usr/bin/env bash
# Concerto smoke gate — the layer-2 verification backstop described in
# tasks/README.md §5 and tasks/v1.0/README.md §5.3.
#
# This file is the DRIVER. The actual capability checks live one-per-file
# under scripts/smoke.d/<NN>-<capability>.sh and are enabled, in order, by
# scripts/smoke.manifest. V1.0 tasks extend the gate additively: drop a new
# scripts/smoke.d/<NN>-<cap>.sh file (defining `check_<cap>`) and append its
# capability name to scripts/smoke.manifest — no edit to this driver.
#
# Capability coverage (V0.1, unchanged by this refactor):
#   - core-boot           Core boots, smoke-client connects via UDS,
#                         Runtime.GetServerCapabilities round-trips UDS.
#   - project-repo-clone  create a project + bare repo + clone.
#   - workspace-workarea  create a workspace + workarea; on-disk worktree
#                         layout (.context/ + repo/.git) verified inline.
#   - echo-session        spawn an echo session under the workarea.
#   - streams-subscribe   stream session.io via Streams.Subscribe; stop it.
#   - permission-flip     flip workarea permission mode to auto.
#   - audit-log           audit JSONL contains workspace_created.
#   - loop                /loop create + list round-trip.
#   - skills              skills discovery picks up a planted SKILL.md.
#   - mcp                 MCP listing picks up a planted mcp.json.
# Clean shutdown of Core + tmpdir is the driver's responsibility (cleanup
# trap), not a capability check.
#
# State-sharing contract (why checks are SOURCED, not sub-processed):
#   The checks share one Core boot and a sequential ID chain
#   (PROJECT_ID -> REPO_ID -> WS_ID -> WA_ID -> SID). The driver SOURCES each
#   smoke.d file into THIS shell process so those variables, the SMOKE_CLIENT
#   argv array, the cleanup trap, and CORE_PID all persist across checks.
#   Each check function documents (in its file header) the variables it
#   requires to be set before it runs, and which it exports for later checks.
#
# Contract:
#   - Exit 0 = pass, non-zero = fail. Output is human-readable.
#   - Each check echoes `PASS <capability>` / `FAIL <capability>`.
#   - CONCERTO_HOME points to a tempdir for the duration of the script.
#     Tasks must not rely on the literal path ~/concerto/.
#   - Linux/macOS only in V0.1; Windows port (scripts/smoke.ps1) is V1.0.
#
# Flags:
#   --ci-mode            Skip checks that are inappropriate for unattended
#                        CI runners. V0.1 is a no-op (everything is CI-safe
#                        today); documented so the workflow file can pass the
#                        flag now and future gh-CLI / network-touching checks
#                        can opt in via `ci_skip` (see below).
#   --only <capability>  Run a single capability check (plus the mandatory
#                        core-boot scaffolding it depends on), then exit.
#   --list               Print the enabled capabilities (manifest order) and
#                        exit 0 without booting Core.

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)

# shellcheck source-path=SCRIPTDIR
# shellcheck source=lib/common.sh
. "$SCRIPT_DIR/lib/common.sh"

SMOKE_D_DIR="$SCRIPT_DIR/smoke.d"
MANIFEST="$SCRIPT_DIR/smoke.manifest"

# ---------------------------------------------------------------------------
# Argument parsing.
# ---------------------------------------------------------------------------
CI_MODE=0
LIST_ONLY=0
ONLY_CAP=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --ci-mode)
            CI_MODE=1
            ;;
        --list)
            LIST_ONLY=1
            ;;
        --only)
            shift
            [ "$#" -ge 1 ] || fail "--only requires a <capability> argument"
            ONLY_CAP="$1"
            ;;
        --only=*)
            ONLY_CAP="${1#--only=}"
            ;;
        *)
            fail "unknown flag: $1"
            ;;
    esac
    shift
done
export CI_MODE

# ---------------------------------------------------------------------------
# Manifest loading. The manifest lists one capability per line, in run
# order. Blank lines and `#`-comments are ignored. Each capability `<cap>`
# maps to a file scripts/smoke.d/<NN>-<cap>.sh that defines `check_<cap>`.
# ---------------------------------------------------------------------------
[ -f "$MANIFEST" ] || fail "manifest not found: $MANIFEST"

# manifest_capabilities: populate the global MANIFEST_CAPS array (space-safe
# because capability names never contain whitespace) in manifest order.
MANIFEST_CAPS=()
while IFS= read -r line || [ -n "$line" ]; do
    # Strip leading/trailing whitespace and inline comments.
    line="${line%%#*}"
    # Trim surrounding whitespace (bash 3.2-safe).
    line="$(printf '%s' "$line" | tr -d '[:space:]')"
    [ -n "$line" ] || continue
    MANIFEST_CAPS+=("$line")
done < "$MANIFEST"

[ "${#MANIFEST_CAPS[@]}" -gt 0 ] || fail "manifest is empty: $MANIFEST"

# cap_file <capability> — resolve the smoke.d file for a capability by its
# <NN>-<capability>.sh suffix. Echoes the path; fails if missing/ambiguous.
cap_file() {
    cap="$1"
    matches=""
    count=0
    for f in "$SMOKE_D_DIR"/[0-9][0-9]-"$cap".sh; do
        [ -e "$f" ] || continue
        matches="$f"
        count=$(( count + 1 ))
    done
    [ "$count" -eq 1 ] || fail "expected exactly one smoke.d file for '$cap', found $count"
    printf '%s' "$matches"
}

# --list: print enabled capabilities and exit, without booting Core.
if [ "$LIST_ONLY" -eq 1 ]; then
    echo "Enabled smoke capabilities (manifest order):"
    for cap in "${MANIFEST_CAPS[@]}"; do
        printf '  %s\n' "$cap"
    done
    exit 0
fi

# Validate --only target up front (before the multi-minute boot).
if [ -n "$ONLY_CAP" ]; then
    found=0
    for cap in "${MANIFEST_CAPS[@]}"; do
        [ "$cap" = "$ONLY_CAP" ] && found=1
    done
    [ "$found" -eq 1 ] || fail "--only: '$ONLY_CAP' is not an enabled capability (see --list)"
fi

# ---------------------------------------------------------------------------
# Source every check file so its `check_<cap>` function is defined in THIS
# process. Sourcing (vs executing) is what lets checks share the Core boot,
# the cleanup trap, and the PROJECT_ID->...->SID variable chain.
# ---------------------------------------------------------------------------
for cap in "${MANIFEST_CAPS[@]}"; do
    f="$(cap_file "$cap")"
    # shellcheck source=/dev/null
    . "$f"
done

# ---------------------------------------------------------------------------
# Shared scaffolding state (consumed by the check functions).
# ---------------------------------------------------------------------------
START_TS=$(date +%s)

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

if [ -n "$ONLY_CAP" ]; then
    echo "Smoke gate v3: starting (CONCERTO_HOME=$CONCERTO_HOME, ci_mode=$CI_MODE, only=$ONLY_CAP)"
else
    echo "Smoke gate v3: starting (CONCERTO_HOME=$CONCERTO_HOME, ci_mode=$CI_MODE)"
fi

# ---------------------------------------------------------------------------
# Prerequisite handling for `--only`. The V0.1 checks form a strictly
# sequential state chain: core-boot builds + boots Core and sets SOCKET /
# SMOKE_CLIENT; then PROJECT_ID -> REPO_ID -> WS_ID -> WA_ID -> SID are built
# one capability at a time, each later check reading what an earlier one set.
# There is no per-check dependency declaration to resolve, so `--only <cap>`
# runs the manifest PREFIX up to and including <cap> — i.e. every check that
# must have run to satisfy <cap>'s shared-state preconditions. `--only
# core-boot` therefore runs just core-boot; `--only permission-flip` runs
# core-boot -> ... -> permission-flip. A full run is the whole manifest.
# ---------------------------------------------------------------------------
run_check() {
    cap="$1"
    fn="check_$(printf '%s' "$cap" | tr '-' '_')"
    command -v "$fn" >/dev/null 2>&1 || fail "check function '$fn' for '$cap' is not defined"
    "$fn"
}

for cap in "${MANIFEST_CAPS[@]}"; do
    run_check "$cap"
    # In --only mode, stop after the requested capability (its prerequisites
    # are exactly the manifest entries that preceded it).
    if [ -n "$ONLY_CAP" ] && [ "$cap" = "$ONLY_CAP" ]; then
        break
    fi
done

# ---------------------------------------------------------------------------
# Clean shutdown — SIGTERM the core, wait for it to exit, verify the
# pid file + socket were cleaned up. (Only meaningful once Core has booted,
# which core-boot guarantees for both full and --only runs.)
# ---------------------------------------------------------------------------
echo "Smoke gate v3: shutting down Core..."
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

END_TS=$(date +%s)
ELAPSED=$(( END_TS - START_TS ))
echo "Smoke gate v3: PASSED"
echo "V0.1 alpha — ${ELAPSED} seconds, all checks PASSED."
