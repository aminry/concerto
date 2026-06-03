# shellcheck shell=bash
# Capability: files-transfer (Task 203).
#
# Exercises the `Files` gRPC service over the live UDS Core via
# `smoke-client files-transfer-probe`:
#   1. upload a small multi-chunk file into the workarea's `.context/`
#      root (chunked ≤ 256 KiB, incremental BLAKE2b-256, finalize-verified),
#   2. download it back and assert byte-identical + matching BLAKE2b-256,
#   3. stat it (exists / size / not-a-dir),
#   4. assert an out-of-scope `../escape.txt` upload is REJECTED (the
#      path_policy allow/deny floor is enforced before any byte hits disk).
#
# The probe targets `.context/` (repository_id unset), which is ALWAYS part
# of the workarea allow-list, so it does not depend on a repo checkout.
#
# The real split-host transfer over Iroh is Task 220 (Tier 3); this proves
# chunking/checksum/scoping co-located in CI.
#
# Requires (from workspace-workarea + earlier):
#   SMOKE_CLIENT, SOCKET, CONCERTO_HOME, CORE_LOG, WA_ID.
check_files_transfer() {
    echo "Smoke gate v3: probing Files upload/download/stat + out-of-scope reject..."
    PROBE_LOG="$CONCERTO_HOME/files-transfer-probe.log"
    if ! "${SMOKE_CLIENT[@]}" --socket "$SOCKET" files-transfer-probe \
        --workarea-id "$WA_ID" \
        > "$PROBE_LOG" 2>"$CONCERTO_HOME/files-transfer-probe-err.log"; then
        echo "smoke: files-transfer-probe stderr:" >&2
        sed 's/^/    /' "$CONCERTO_HOME/files-transfer-probe-err.log" >&2 || true
        echo "smoke: core log (last 100 lines):" >&2
        tail -n 100 "$CORE_LOG" | sed 's/^/    /' >&2 || true
        echo "FAIL files-transfer"
        fail "files-transfer-probe"
    fi
    if ! grep -q "files-transfer-probe: OK" "$PROBE_LOG"; then
        echo "smoke: files-transfer-probe did not report OK:" >&2
        sed 's/^/    /' "$PROBE_LOG" >&2 || true
        echo "FAIL files-transfer"
        fail "files-transfer-probe did not confirm OK"
    fi

    echo "PASS files-transfer"
}
