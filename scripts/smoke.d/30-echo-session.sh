# shellcheck shell=bash
# Capability: echo-session.
#
# Spawns an echo-kind agent session under the workarea.
#
# Requires (from earlier checks):
#   SMOKE_CLIENT, SOCKET, CORE_LOG, WA_ID.
# Exports (consumed by streams-subscribe):
#   SID  the spawned session id.
check_echo_session() {
    echo "Smoke gate v3: spawning echo session..."
    if ! SID=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" start-session --workarea-id "$WA_ID" --agent-kind echo 2>&1); then
        echo "smoke: start-session failed; output:" >&2
        echo "$SID" >&2
        echo "smoke: core log:" >&2
        sed 's/^/    /' "$CORE_LOG" >&2 || true
        echo "FAIL echo-session"
        fail "start-session"
    fi

    echo "PASS echo-session"
}
