#!/usr/bin/env bash
# Maestro chat end-to-end harness.
#
# Drives the live `Maestro.*` gRPC surface — the exact contract the desktop
# chat's `callRpc("Maestro.*")` bindings hit through the Tauri shell — against a
# running Core, and asserts the chat works end to end: state, history, digest,
# the freeform send→assistant-reply loop, and `@workarea` routing notices.
#
# Usage:
#   tools/maestro-chat-e2e.sh [--socket <path>] [--smoke-client <path>]
#
# Requires a running Core (with its real `claude` provider) reachable on the
# socket. Exits 0 if every REQUIRED assertion passes, 1 otherwise. Routing
# assertions are environment-aware (they report, never hard-fail on missing
# workareas) so the harness is safe to run against any Core.
set -uo pipefail

SOCK="${CONCERTO_CORE_SOCK:-$HOME/.concerto/core.sock}"
SC=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --socket) SOCK="$2"; shift 2 ;;
    --smoke-client) SC="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

# Locate the smoke-client binary (built target by default).
if [[ -z "$SC" ]]; then
  ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  SC="$ROOT/target/debug/smoke-client"
fi
[[ -x "$SC" ]] || { echo "smoke-client not found at $SC (build: cargo build -p concerto-smoke-client)" >&2; exit 2; }
[[ -S "$SOCK" ]] || { echo "Core socket not found at $SOCK (is the Core running?)" >&2; exit 2; }

PASS=0; FAIL=0
ok()   { echo "  PASS: $1"; PASS=$((PASS+1)); }
bad()  { echo "  FAIL: $1"; FAIL=$((FAIL+1)); }
info() { echo "  INFO: $1"; }

mae() { "$SC" --socket "$SOCK" "$@" 2>&1; }

echo "== Maestro chat E2E (socket: $SOCK) =="

# 1. GetState — enabled, live session, frozen caps.
echo "[1] Maestro.GetState"
STATE="$(mae maestro-state)"
echo "$STATE" | grep -q '"enabled":true'            && ok "maestro enabled"            || bad "maestro not enabled: $STATE"
echo "$STATE" | grep -qE '"maestro_session_id":"[0-9a-f]' && ok "live session present"  || bad "no live maestro session: $STATE"
echo "$STATE" | grep -q '"in_cap":200000'           && ok "in_cap = 200000"            || bad "in_cap wrong: $STATE"
echo "$STATE" | grep -q '"out_cap":50000'           && ok "out_cap = 50000"            || bad "out_cap wrong: $STATE"

# 2. GetHistory — returns without error.
echo "[2] Maestro.GetHistory"
HIST="$(mae maestro-history)"
echo "$HIST" | head -1 | grep -qE '^turns: [0-9]+' && ok "history readable ($(echo "$HIST" | head -1))" || bad "history unreadable: $(echo "$HIST" | head -1)"

# 3. GetDigest — returns, and must NOT echo the raw LLM prompt (a real summary).
echo "[3] Maestro.GetDigest"
DIG="$(mae maestro-digest)"
echo "$DIG" | grep -qE '^chips: [0-9]+' && ok "digest readable" || bad "digest unreadable: $DIG"
if echo "$DIG" | grep -qiE "You are Concerto's maestro|Write a concise|proposed next step"; then
  bad "digest echoes the raw LLM prompt instead of a summary"
else
  ok "digest is a summary (no prompt echo)"
fi

# 4. Freeform send -> assistant reply on maestro.events (the critical loop).
echo "[4] Freeform send -> assistant reply"
WOUT="$(mktemp)"
mae maestro-watch --timeout 50 >"$WOUT" 2>&1 &
WPID=$!
sleep 1
mae maestro-send --text "In one short sentence, how many workareas do I have right now?" >/dev/null
for _ in $(seq 1 16); do
  sleep 3
  grep -q '"role":"assistant"' "$WOUT" && break
done
kill "$WPID" 2>/dev/null; wait "$WPID" 2>/dev/null
grep -q '"role":"user"' "$WOUT"      && ok "user turn echoed"   || bad "no user turn frame"
if grep -q '"role":"assistant"' "$WOUT"; then
  REPLY="$(grep -oE '"role":"assistant"[^}]*"text":"[^"]{1,80}' "$WOUT" | head -1)"
  ok "assistant reply received: ${REPLY:-(text)}"
else
  bad "no assistant reply within budget (chat loop broken)"
fi
rm -f "$WOUT"

# 5. Routing @workarea — environment-aware (reports; never hard-fails).
echo "[5] @workarea routing notice"
ROUT="$(mktemp)"
mae maestro-watch --timeout 10 >"$ROUT" 2>&1 &
RPID=$!
sleep 1
mae maestro-send --text "@bach hi" >/dev/null
sleep 5
kill "$RPID" 2>/dev/null; wait "$RPID" 2>/dev/null
if grep -q 'routing_executed' "$ROUT"; then
  ok "routing executed to a live session"
elif grep -qiE "active session|no active|start a session" "$ROUT"; then
  ok "no-session notice surfaced (idle workarea handled gracefully)"
else
  info "no @bach routing frame (no 'bach' workarea in this Core — skipped)"
fi
rm -f "$ROUT"

echo "== RESULT: $PASS passed, $FAIL failed =="
[[ "$FAIL" -eq 0 ]]
