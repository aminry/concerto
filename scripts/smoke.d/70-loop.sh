# shellcheck shell=bash
# Capability: loop.
#
# Creates a /loop schedule and confirms it shows up in the loop listing.
#
# Requires (from earlier checks):
#   SMOKE_CLIENT, SOCKET, WA_ID.
check_loop() {
    echo "Smoke gate v3: creating /loop schedule..."
    LOOP_ID=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" create-loop \
        --workarea "$WA_ID" --interval 30 --prompt "tick")
    if [ -z "$LOOP_ID" ]; then
        echo "FAIL loop"
        fail "create-loop returned empty id"
    fi
    LIST_OUT=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" list-loops --workarea "$WA_ID")
    if ! echo "$LIST_OUT" | grep -qx "$LOOP_ID"; then
        echo "FAIL loop"
        fail "list-loops missing $LOOP_ID; got: $LIST_OUT"
    fi

    echo "PASS loop"
}
