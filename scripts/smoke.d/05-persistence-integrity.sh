# shellcheck shell=bash
# Capability: persistence-integrity (Task 110).
#
# Asserts that Core booted cleanly on a fresh DB and that the persistence
# open path ran its on-boot integrity guards without finding corruption or a
# schema downgrade. `00-core-boot` has already built + booted Core under a
# scratch HOME by the time this runs; the persistence open path logs a single
# deterministic success line once `PRAGMA quick_check` passes and the schema
# is not a downgrade. We grep the Core log for that line.
#
# Requires (from 00-core-boot, which runs first per the manifest):
#   CORE_LOG   path to the Core stdout+stderr log.
check_persistence_integrity() {
    echo "Smoke gate v3: verifying persistence integrity guard logged success..."

    if [ ! -f "$CORE_LOG" ]; then
        echo "FAIL persistence-integrity"
        fail "Core log not present at $CORE_LOG"
    fi

    # The persistence open path (crates/persist/src/api.rs) emits this line on
    # the success path after the on-open quick_check passes and the downgrade
    # guard clears. Its presence proves the guards ran and the fresh DB is
    # clean; a corrupt or downgraded DB would have aborted boot before
    # core.sock appeared (00-core-boot would already have failed).
    if ! grep -q 'persistence integrity ok' "$CORE_LOG"; then
        echo "smoke: core log:" >&2
        sed 's/^/    /' "$CORE_LOG" >&2 || true
        echo "FAIL persistence-integrity"
        fail "persistence integrity success line not found in Core log"
    fi

    echo "PASS persistence-integrity"
}
