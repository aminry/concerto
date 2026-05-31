# shellcheck shell=bash
# Capability: streams-subscribe.
#
# Streams the session's output via Streams.Subscribe(session.io.<sid>),
# verifies output was captured, then stops the session.
#
# Requires (from echo-session + earlier):
#   SMOKE_CLIENT, SOCKET, CONCERTO_HOME, CORE_LOG, SID.
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

    echo "PASS streams-subscribe"
}
