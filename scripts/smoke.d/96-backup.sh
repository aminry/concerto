# shellcheck shell=bash
# Capability: backup (Task 111).
#
# Runs the shipped `concerto backup` against the Core booted by 00-core-boot,
# then asserts the snapshot exists and opens. `backup` is file-level (a
# `VACUUM INTO` snapshot of the local DB); it does NOT dial the Core, but it
# resolves the DB via $CONCERTO_DATA_DIR, which 00-core-boot exports to point
# at the scratch home's data dir — so the DB the Core just created is the one
# we snapshot.
#
# Runs after `cli` (manifest order): the Core has been booted + written its DB
# by then. This is read-only w.r.t. the live DB (the source is opened
# read-only), so it is safe to run while the Core is still up.
#
# Requires (from 00-core-boot, which runs first per the manifest):
#   CORE_DATA_DIR      <CONCERTO_HOME>/concerto — the data dir holding the
#                      live concerto.db (exported by 00-core-boot).
#   CONCERTO_DATA_DIR  exported by 00-core-boot == CORE_DATA_DIR; `concerto
#                      backup` reads this to resolve the source DB.
check_backup() {
    echo "Smoke gate v3: building + running the concerto CLI (backup)..."
    # Pre-build so `cargo run`'s compile step stays out of the assertion.
    cargo build --quiet -p concerto-cli --bin concerto

    BACKUP_OUT="$CONCERTO_HOME/backup-out"
    rm -rf "$BACKUP_OUT"

    # `concerto backup` resolves the source DB from $CONCERTO_DATA_DIR (exported
    # by 00-core-boot). Capture stdout so a non-zero exit trips `set -e`.
    BACKUP_LOG=$(cargo run --quiet -p concerto-cli --bin concerto -- backup --out "$BACKUP_OUT")
    echo "$BACKUP_LOG"

    SNAPSHOT="$BACKUP_OUT/concerto.db"
    if [ ! -f "$SNAPSHOT" ]; then
        echo "FAIL backup"
        fail "backup snapshot not created at $SNAPSHOT"
    fi

    # The manifest must be present too (frozen <out>/ layout).
    if [ ! -f "$BACKUP_OUT/manifest.json" ]; then
        echo "FAIL backup"
        fail "backup manifest.json not created at $BACKUP_OUT/manifest.json"
    fi

    # Confirm the snapshot opens + integrity-checks. Prefer the system
    # `sqlite3` if present (fast, no rebuild); otherwise re-run `concerto
    # backup` against the SNAPSHOT's own dir as a re-open smoke (it opens the
    # DB read-only via sqlx, which fails loudly on a corrupt file).
    if command -v sqlite3 >/dev/null 2>&1; then
        QC=$(sqlite3 "$SNAPSHOT" 'PRAGMA quick_check;' 2>&1 || true)
        if [ "$QC" != "ok" ]; then
            echo "FAIL backup"
            fail "snapshot failed PRAGMA quick_check: $QC"
        fi
    else
        # No sqlite3 on PATH: re-back-up the snapshot itself. `concerto backup`
        # opens the source DB read-only via sqlx; a corrupt snapshot would
        # fail the open and exit non-zero (tripping `set -e`).
        REVERIFY_OUT="$CONCERTO_HOME/backup-reverify"
        rm -rf "$REVERIFY_OUT"
        CONCERTO_DB_PATH="$SNAPSHOT" \
            cargo run --quiet -p concerto-cli --bin concerto -- \
            backup --out "$REVERIFY_OUT" >/dev/null
        if [ ! -f "$REVERIFY_OUT/concerto.db" ]; then
            echo "FAIL backup"
            fail "re-opening the snapshot via concerto backup did not produce a DB"
        fi
    fi

    echo "PASS backup"
}
