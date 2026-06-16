# shellcheck shell=bash
# Capability: notifications (Task 507 — the sub-system 14 service half).
#
# Proves the live `Notifications` gRPC service (design/14 §5.1) round-trips
# over the shared Core's loopback UDS: seed a notification row directly into the
# running Core's SQLite DB, then call `Notifications.GetInbox` via the
# smoke-client and assert the seeded notification surfaces (id + body + kind).
# This is the same seed-then-GetInbox proof the live web inbox uses
# (scripts/web-live-demo.sh), reduced to a headless gRPC round-trip against the
# UDS Core the smoke gate already booted.
#
# Why seed via sqlite (not drive `notify_user`): the `notify_user` MCP tool runs
# INSIDE the spawned Maestro CLI session, which the headless smoke gate does not
# stand up. The notifications *service* is what Task 507 exposes on every
# transport, so we exercise THAT directly — a seeded row + a real GetInbox is a
# faithful end-to-end check of the wire path the phone/web clients use, and it
# is exactly what `crates/core/tests/maestro_notify_user.rs` complements at the
# `notify_user` → `NotificationHandle` layer.
#
# Requires (from core-boot + the driver): CONCERTO_HOME, SOCKET, SMOKE_CLIENT,
# CORE_DATA_DIR, CORE_LOG, CI_MODE. SCRIPT_DIR is set by the driver.
# Exports: nothing (self-contained; the seeded row lives in the scratch DB).
check_notifications() {
    # The Core's SQLite DB lives at <data_dir>/concerto.db (RuntimeConfig::db_path).
    NOTIF_DB="$CORE_DATA_DIR/concerto.db"
    if [ ! -f "$NOTIF_DB" ]; then
        echo "FAIL notifications"
        fail "Core SQLite DB not found at $NOTIF_DB"
    fi

    # `sqlite3` is the only external dependency; if it's missing we cannot seed
    # the row, so SKIP cleanly rather than fail (the gRPC service is still
    # covered by the in-crate integration tests).
    if ! command -v sqlite3 >/dev/null 2>&1; then
        echo "SKIP notifications (sqlite3 not available to seed the DB)"
        return 0
    fi

    # A unique id so a re-run (or a stray row) can't false-pass.
    NOTIF_ID="smoke-notif-$$-$(date +%s)"
    NOTIF_BODY="smoke notify_user body $$"
    NOW_MS=$(( $(date +%s) * 1000 ))

    # Seed one notification. `subject_kind='session'` + `subject_id` mirror the
    # live `notify_user` mapping (AgentCompletedWithMessage / Session subject),
    # but the FK columns (workspace_id/workarea_id/session_id) stay NULL so the
    # insert never trips a foreign-key constraint against the empty smoke DB.
    # `-cmd '.timeout 8000'` lets this writer coexist with the running Core (WAL)
    # without echoing a PRAGMA result row to stdout.
    echo "Smoke gate v3: seeding notification $NOTIF_ID into $NOTIF_DB ..."
    if ! sqlite3 -cmd '.timeout 8000' "$NOTIF_DB" "
INSERT INTO notifications (id,kind,subject_kind,subject_id,title,body,severity,created_at)
VALUES ('$NOTIF_ID','agent_completed_with_message','session','smoke-sess','Concerto','$NOTIF_BODY','medium',$NOW_MS);"; then
        echo "FAIL notifications"
        fail "failed to seed notification row via sqlite3"
    fi

    # Read it back over the live gRPC `Notifications.GetInbox`.
    echo "Smoke gate v3: calling Notifications.GetInbox ..."
    INBOX_OUT="$("${SMOKE_CLIENT[@]}" --socket "$SOCKET" get-inbox)"
    if ! printf '%s\n' "$INBOX_OUT" | grep -q "$NOTIF_ID"; then
        echo "smoke: GetInbox output:" >&2
        printf '%s\n' "$INBOX_OUT" | sed 's/^/    /' >&2
        echo "smoke: core log:" >&2
        sed 's/^/    /' "$CORE_LOG" >&2 || true
        echo "FAIL notifications"
        fail "seeded notification $NOTIF_ID did not surface via GetInbox"
    fi
    # Confirm the body + kind crossed the wire intact (not just the id).
    if ! printf '%s\n' "$INBOX_OUT" | grep -q "$NOTIF_BODY"; then
        echo "FAIL notifications"
        fail "GetInbox row for $NOTIF_ID is missing its body"
    fi
    if ! printf '%s\n' "$INBOX_OUT" | grep -q 'NOTIFICATION_KIND_AGENT_COMPLETED_WITH_MESSAGE'; then
        echo "FAIL notifications"
        fail "GetInbox row for $NOTIF_ID is missing the expected kind"
    fi

    # Real-Expo push delivery (a live wakeup to the Expo push gateway) needs
    # network + a registered device push token; it has no place in an unattended
    # runner. The seed→GetInbox leg above is the CI-safe core of the check;
    # `--ci-mode` skips only the (currently demo-only) real-push leg.
    if [ "${CI_MODE:-0}" -eq 1 ]; then
        echo "Smoke gate v3: ci-mode — skipping the real-Expo push delivery leg"
    else
        echo "Smoke gate v3: (real-Expo push delivery leg not exercised in the headless gate)"
    fi

    echo "PASS notifications"
}
