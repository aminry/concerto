#!/usr/bin/env bash
# Concerto smoke gate — the layer-2 verification backstop described in
# tasks/README.md §5.
#
# Responsibilities (grow over the build):
#   - Phase 1 (Task 15): Core boots, smoke-client connects via UDS,
#     Runtime.GetServerCapabilities round-trips, Core shuts down cleanly.
#   - Phase 2 (Task 27): create a project + bare repo + clone + workspace +
#     workarea, spawn an echo session, verify session output streams via
#     Streams.Subscribe(session.io.<sid>), stop the session, shut Core
#     down cleanly. On-disk worktree layout (`.context/` + repo/.git) is
#     verified inline.
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

echo "Smoke gate v2: starting (CONCERTO_HOME=$CONCERTO_HOME)"

# ---------------------------------------------------------------------------
# Phase 1 — Core boot + UDS + GetServerCapabilities + clean shutdown.
# ---------------------------------------------------------------------------

CORE_CONFIG_DIR="$CONCERTO_HOME/.concerto"
CORE_DATA_DIR="$CONCERTO_HOME/concerto"
mkdir -p "$CORE_CONFIG_DIR" "$CORE_DATA_DIR"

# Pre-build all the binaries the smoke gate exercises so `cargo run`
# doesn't slip a compile step into the wall clock. `--quiet` keeps the
# build noise out of smoke output; real errors still surface because
# cargo writes them to stderr. `concerto-agent-host` is pre-built because
# the supervisor (Task 22) spawns it for `agent-kind=echo` sessions and
# resolves it through `current_exe().parent()`.
echo "Smoke gate v2: building concerto-core, concerto-agent-host, smoke-client..."
cargo build --quiet -p concerto-core -p concerto-agent-host -p concerto-smoke-client

echo "Smoke gate v2: starting concerto-core in background..."
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
echo "Smoke gate v2: Core ready (socket: $SOCKET)"

# Convenience: every subsequent smoke-client invocation passes
# `--socket "$SOCKET"`. The data-dir is also exported so the
# `add-project` subcommand resolves the same SQLite path the Core uses.
export CONCERTO_DATA_DIR="$CORE_DATA_DIR"
SMOKE_CLIENT=(cargo run --quiet -p concerto-smoke-client --bin smoke-client --)

# ---------------------------------------------------------------------------
# Phase 1 — Core boot + UDS + GetServerCapabilities.
# ---------------------------------------------------------------------------

# Call GetServerCapabilities and confirm the response advertises UDS.
echo "Smoke gate v2: calling Runtime.GetServerCapabilities..."
RESPONSE=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" caps)
echo "Smoke gate v2: response: $RESPONSE"
if ! echo "$RESPONSE" | grep -q '"transport_kind": *"TRANSPORT_KIND_UDS"'; then
    echo "smoke: core log:" >&2
    sed 's/^/    /' "$CORE_LOG" >&2 || true
    fail "unexpected smoke-client output (missing TRANSPORT_KIND_UDS)"
fi

# ---------------------------------------------------------------------------
# Phase 2 — project / repo / clone / workspace / workarea + echo session
#           round-trip via the smoke-client subcommands (Task 27).
# ---------------------------------------------------------------------------

echo "Smoke gate v2: creating bare test repo..."
BARE="$CONCERTO_HOME/bare-repo.git"
mkdir -p "$BARE"
git init --bare --quiet "$BARE"
git -C "$BARE" symbolic-ref HEAD refs/heads/main

# Push an initial commit via a temp clone so the bare repo has a real
# default branch the `git clone` shell-out in the Repo Manager can find.
TMP="$CONCERTO_HOME/seed"
git clone --quiet "$BARE" "$TMP"
echo "# smoke test" > "$TMP/README.md"
git -C "$TMP" add -A
git -C "$TMP" -c user.email=smoke@test -c user.name=Smoke commit -m "seed" --quiet
git -C "$TMP" push --quiet origin main

echo "Smoke gate v2: creating project / repo / workspace / workarea..."
PROJECT_ID=$("${SMOKE_CLIENT[@]}" add-project --name "smoke")
REPO_ID=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" add-repo --project-id "$PROJECT_ID" --url "file://$BARE")
"${SMOKE_CLIENT[@]}" --socket "$SOCKET" clone --repo-id "$REPO_ID" || fail "clone"
WS_ID=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" new-workspace --project-id "$PROJECT_ID" --name "wsp" --repo-id "$REPO_ID")
WA_ID=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" new-workarea --workspace-id "$WS_ID")

# Verify the workarea root on disk.
# `WT_ROOT` is `<data_dir>/workspaces/<workspace.slug>/<composer-name>/`
# per Task 20's locked layout. The composer name is server-allocated so
# we glob for it via `find` (shellcheck SC2012 forbids `ls | head`).
WT_ROOT=$(find "$CORE_DATA_DIR/workspaces/wsp" -maxdepth 1 -mindepth 1 -type d | head -n 1)
[ -n "$WT_ROOT" ] || fail "workarea root not found under $CORE_DATA_DIR/workspaces/wsp"
[ -d "$WT_ROOT/.context" ] || fail ".context/ missing in workarea root $WT_ROOT"
# Workarea contains one repo subdir whose `.git` is present (single-repo
# V0.1 layout per design/03 §4.2). `git worktree add` writes `.git` as a
# regular file (containing `gitdir: <abspath>`), not a directory — `-e`
# catches both forms.
REPO_GIT_FOUND=0
for repo_dir in "$WT_ROOT"/*/; do
    if [ -e "$repo_dir/.git" ]; then
        # Skip the `.context/` directory — it isn't a repo subdir.
        case "${repo_dir%/}" in
            *"/.context") continue ;;
        esac
        REPO_GIT_FOUND=1
    fi
done
[ "$REPO_GIT_FOUND" -eq 1 ] || fail "repo .git missing in workarea root $WT_ROOT"

echo "Smoke gate v2: spawning echo session and streaming output..."
if ! SID=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" start-session --workarea-id "$WA_ID" --agent-kind echo 2>&1); then
    echo "smoke: start-session failed; output:" >&2
    echo "$SID" >&2
    echo "smoke: core log:" >&2
    sed 's/^/    /' "$CORE_LOG" >&2 || true
    fail "start-session"
fi
SESSION_LOG="$CONCERTO_HOME/session-out.log"
if ! "${SMOKE_CLIENT[@]}" --socket "$SOCKET" stream-session-io \
    --session-id "$SID" --timeout 10 > "$SESSION_LOG" 2>"$CONCERTO_HOME/stream-err.log"; then
    echo "smoke: stream-session-io stderr:" >&2
    sed 's/^/    /' "$CONCERTO_HOME/stream-err.log" >&2 || true
    echo "smoke: core log:" >&2
    sed 's/^/    /' "$CORE_LOG" >&2 || true
    fail "stream-session-io"
fi
if ! grep -q . "$SESSION_LOG"; then
    echo "smoke: stream-session-io stderr:" >&2
    sed 's/^/    /' "$CONCERTO_HOME/stream-err.log" >&2 || true
    echo "smoke: core log (last 100 lines):" >&2
    tail -n 100 "$CORE_LOG" | sed 's/^/    /' >&2 || true
    fail "no session output captured at $SESSION_LOG"
fi

"${SMOKE_CLIENT[@]}" --socket "$SOCKET" stop-session --session-id "$SID" || true

# ---------------------------------------------------------------------------
# Clean shutdown — SIGTERM the core, wait for it to exit, verify the
# pid file + socket were cleaned up.
# ---------------------------------------------------------------------------

echo "Smoke gate v2: shutting down Core..."
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

# Phase 3 checks — added in Tasks 42 + 44
# Phase 4 checks — added in Task 52

echo "Smoke gate v2: PASSED"
