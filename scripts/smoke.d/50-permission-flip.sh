# shellcheck shell=bash
# Capability: permission-flip.
#
# Flips the workarea permission mode to `auto` and asserts the echoed mode.
#
# Requires (from earlier checks):
#   SMOKE_CLIENT, SOCKET, WA_ID.
check_permission_flip() {
    echo "Smoke gate v3: flipping workarea permission mode to auto..."
    PERM_RESP=$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" set-perm-mode \
        --workarea "$WA_ID" --mode auto)
    if [ "$PERM_RESP" != "PERMISSION_MODE_AUTO" ]; then
        echo "FAIL permission-flip"
        fail "set-perm-mode: expected PERMISSION_MODE_AUTO, got '$PERM_RESP'"
    fi

    echo "PASS permission-flip"
}
