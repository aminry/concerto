# shellcheck shell=bash
# Capability: audit-rotation (extends:audit-rotation, Task 112).
#
# Asserts the JSONL default still writes under the generalized
# AuditLogSubscriber fan-out (Task 112): the always-on JsonlFileSubscriber
# is the durable floor, and the on-disk file keeps the daily
# `audit-<YYYY-MM-DD>.jsonl` rotated-file naming convention under
# `<data_dir>/audit/`.
#
# We can't cross a real UTC day boundary inside the smoke run, so the true
# rotation boundary (a second dated file appearing) is covered by the Rust
# test `crates/core/tests/audit_subscribers.rs::daily_rotation_opens_new_file_at_boundary`.
# Here we prove the rotated-NAMING convention holds for today's file and
# that the fan-out still routes a real event to the JSONL default.
#
# Requires (from earlier checks):
#   CORE_DATA_DIR — set by 00-core-boot; a workspace_created event must
#   have been emitted earlier (workspace-workarea + audit-log checks).
check_audit_rotation() {
    echo "Smoke gate v3: verifying audit JSONL default + rotated-file naming..."

    AUDIT_DIR="$CORE_DATA_DIR/audit"
    if [ ! -d "$AUDIT_DIR" ]; then
        echo "FAIL audit-rotation"
        fail "audit dir not present at $AUDIT_DIR"
    fi

    # The daily file for today must exist with the rotated-naming convention
    # `audit-<YYYY-MM-DD>.jsonl` (the layout Task 112 formalizes).
    TODAY="$(date -u +%F)"
    DAILY_FILE="$AUDIT_DIR/audit-$TODAY.jsonl"
    if [ ! -f "$DAILY_FILE" ]; then
        echo "smoke: audit dir contents:" >&2
        ls -la "$AUDIT_DIR" >&2 || true
        echo "FAIL audit-rotation"
        fail "daily JSONL not present at $DAILY_FILE (rotated-naming convention broken)"
    fi

    # Every file in the audit dir must match the rotated-naming convention
    # `audit-<YYYY-MM-DD>[.<seq>].jsonl` — nothing else may leak in.
    for f in "$AUDIT_DIR"/*; do
        [ -e "$f" ] || continue
        base="$(basename "$f")"
        case "$base" in
            audit-[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9].jsonl) : ;;
            audit-[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9].[0-9]*.jsonl) : ;;
            *)
                echo "FAIL audit-rotation"
                fail "unexpected file in audit dir (breaks rotated-naming): $base"
                ;;
        esac
    done

    # The JSONL default must still carry the workspace_created event that
    # the workspace-workarea check produced — proving the fan-out routes to
    # the durable floor under the new pipeline.
    if ! grep -q '"kind":"workspace_created"' "$DAILY_FILE"; then
        echo "smoke: daily audit contents:" >&2
        sed 's/^/    /' "$DAILY_FILE" >&2 || true
        echo "FAIL audit-rotation"
        fail "JSONL default missing workspace_created under the fan-out pipeline"
    fi

    echo "PASS audit-rotation"
}
