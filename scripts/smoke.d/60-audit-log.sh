# shellcheck shell=bash
# Capability: audit-log.
#
# Verifies the audit JSONL records the workspace_created event emitted when
# workspace-workarea created the workspace.
#
# Requires (from earlier checks):
#   CORE_DATA_DIR (and a workspace must have been created earlier).
check_audit_log() {
    echo "Smoke gate v3: verifying audit log contains workspace_created..."
    # Task 44 wires `WorkspaceCreated` into `WorkspaceManager::create_workspace`
    # so the `new-workspace` step is guaranteed to have emitted a row.
    # The JSONL lives under `<data_dir>/audit/audit-<YYYY-MM-DD>.jsonl`.
    AUDIT_FILE="$CORE_DATA_DIR/audit/audit-$(date -u +%F).jsonl"
    if [ ! -f "$AUDIT_FILE" ]; then
        echo "FAIL audit-log"
        fail "audit log file not present at $AUDIT_FILE"
    fi
    if ! grep -q '"kind":"workspace_created"' "$AUDIT_FILE"; then
        echo "smoke: audit log contents:" >&2
        sed 's/^/    /' "$AUDIT_FILE" >&2 || true
        echo "FAIL audit-log"
        fail "audit log missing workspace_created"
    fi

    echo "PASS audit-log"
}
