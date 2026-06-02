# shellcheck shell=bash
# Capability: streams-subscribe.
#
# Streams the session's output via Streams.Subscribe(session.io.<sid>),
# verifies output was captured, then stops the session.
#
# Task 202 extension: after the session-IO assertion, probe the
# Streams.Subscribe RECONNECT path over the same live UDS Core —
# `since_offset` replay (a reconnecting client gets exactly the events it
# missed) and the AckOffset→prune→GapDetected path (an out-of-range
# `since_offset` after pruning yields a single GapDetected frame). The
# probe is self-contained in `smoke-client streams-replay-probe`
# (subscribes to workspace.events, creates two workspaces, reconnects).
#
# Requires (from echo-session + earlier):
#   SMOKE_CLIENT, SOCKET, CONCERTO_HOME, CORE_LOG, SID, PROJECT_ID, REPO_ID.
check_streams_subscribe() {
    echo "Smoke gate v3: streaming session output..."
    SESSION_LOG="$CONCERTO_HOME/session-out.log"
    if ! "${SMOKE_CLIENT[@]}" --socket "$SOCKET" stream-session-io \
        --session-id "$SID" --timeout 10 > "$SESSION_LOG" 2>"$CONCERTO_HOME/stream-err.log"; then
        echo "smoke: stream-session-io stderr:" >&2
        sed 's/^/    /' "$CONCERTO_HOME/stream-err.log" >&2 || true
        echo "smoke: core log:" >&2
        sed 's/^/    /' "$CORE_LOG" >&2 || true
        echo "FAIL streams-subscribe"
        fail "stream-session-io"
    fi
    if ! grep -q . "$SESSION_LOG"; then
        echo "smoke: stream-session-io stderr:" >&2
        sed 's/^/    /' "$CONCERTO_HOME/stream-err.log" >&2 || true
        echo "smoke: core log (last 100 lines):" >&2
        tail -n 100 "$CORE_LOG" | sed 's/^/    /' >&2 || true
        echo "FAIL streams-subscribe"
        fail "no session output captured at $SESSION_LOG"
    fi

    "${SMOKE_CLIENT[@]}" --socket "$SOCKET" stop-session --session-id "$SID" || true

    # Task 202: reconnect-with-offset (since_offset replay + GapDetected
    # after AckOffset prune) over the live Core. Deterministic: drives
    # workspace.events, which emits exactly one event per CreateWorkspace.
    echo "Smoke gate v3: probing Streams.Subscribe reconnect (since_offset + GapDetected)..."
    PROBE_LOG="$CONCERTO_HOME/streams-replay-probe.log"
    if ! "${SMOKE_CLIENT[@]}" --socket "$SOCKET" streams-replay-probe \
        --project-id "$PROJECT_ID" --repo-id "$REPO_ID" \
        > "$PROBE_LOG" 2>"$CONCERTO_HOME/streams-replay-probe-err.log"; then
        echo "smoke: streams-replay-probe stderr:" >&2
        sed 's/^/    /' "$CONCERTO_HOME/streams-replay-probe-err.log" >&2 || true
        echo "smoke: core log (last 100 lines):" >&2
        tail -n 100 "$CORE_LOG" | sed 's/^/    /' >&2 || true
        echo "FAIL streams-subscribe"
        fail "streams-replay-probe"
    fi
    if ! grep -q "streams-replay-probe: OK" "$PROBE_LOG"; then
        echo "smoke: streams-replay-probe did not report OK:" >&2
        sed 's/^/    /' "$PROBE_LOG" >&2 || true
        echo "FAIL streams-subscribe"
        fail "streams-replay-probe did not confirm OK"
    fi

    echo "PASS streams-subscribe"
}
