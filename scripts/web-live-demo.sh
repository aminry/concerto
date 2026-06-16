#!/usr/bin/env bash
# Live web-inbox demo (Task 523): boot a Core with the connect-web bridge, seed
# a few notifications directly into its DB, then run the live Playwright test
# (CONCERTO_LIVE=1) which loads apps/web (vite proxies gRPC-Web → the bridge)
# and screenshots the real inbox. Reusable harness for the live-browser proof.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
CORE_BIN="$ROOT/target/debug/concerto-core"
BRIDGE_ADDR="127.0.0.1:8787"
TMP="$(mktemp -d)"

CORE_PID=""
cleanup() { [ -n "$CORE_PID" ] && kill "$CORE_PID" 2>/dev/null || true; }
trap cleanup EXIT

echo "==> booting Core + connect-web bridge on $BRIDGE_ADDR"
CONCERTO_CONFIG_DIR="$TMP/config" CONCERTO_DATA_DIR="$TMP/data" \
  CONCERTO_KEYCHAIN_SERVICE="concerto-webdemo-$(date +%s)" \
  CONCERTO_CONNECT_BRIDGE=1 CONCERTO_CONNECT_BRIDGE_ADDR="$BRIDGE_ADDR" \
  "$CORE_BIN" >"$TMP/core.log" 2>&1 &
CORE_PID=$!

echo "==> waiting for the bridge port"
up=""
for _ in $(seq 1 40); do
  if curl -s -m 2 -o /dev/null "http://$BRIDGE_ADDR/" 2>/dev/null; then up=1; break; fi
  kill -0 "$CORE_PID" 2>/dev/null || { echo "core exited early"; tail -20 "$TMP/core.log"; exit 1; }
  sleep 1
done
[ -n "$up" ] || { echo "bridge never came up"; tail -20 "$TMP/core.log"; exit 1; }
echo "    bridge up"

DB="$TMP/data/concerto.db"
NOW=$(( $(date +%s) * 1000 ))
echo "==> seeding notifications into $DB"
sqlite3 "$DB" "PRAGMA busy_timeout=8000;
INSERT INTO notifications (id,kind,subject_kind,subject_id,title,body,severity,created_at) VALUES
 ('demo-crash','agent_crashed','workspace','ws-demo','Agent crashed in bach','panic: index out of bounds at tool_loop.rs:214','high',$NOW),
 ('demo-approval','tool_approval_needed','session','sess-1','Approve: rm -rf build/','bach wants to run a destructive command','high',$((NOW-60000))),
 ('demo-pr','pr_state_changed','pull_request','pr-9','PR #9 is ready to merge','All 3 checks passed on concerto/bee','low',$((NOW-3600000)));"

echo "==> running the live Playwright screenshot"
CONCERTO_LIVE=1 CONCERTO_BRIDGE_URL="http://$BRIDGE_ADDR" pnpm -C apps/web exec playwright test live-inbox
echo "==> done; core.log tail:"; tail -4 "$TMP/core.log"
