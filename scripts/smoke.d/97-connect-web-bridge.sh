# shellcheck shell=bash
# Capability: connect-web-bridge (Task 204).
#
# Proves the Connect-Web bridge (design/11 §3.4 Path A): a loopback
# `tonic-web` server serving the same Tonic services over gRPC-Web, tagging
# every connection WSS_BRIDGE via the Task-201 ConnTransport seam.
#
# Unlike the other checks, this one does NOT reuse the shared smoke Core
# (which boots UDS-only): it starts its OWN dedicated Core with the bridge
# enabled (`CONCERTO_CONNECT_BRIDGE=1`) on a loopback port, under a SEPARATE
# config dir so it never contends for the shared Core's single-instance
# lock. It then drives a headless gRPC-Web request with `curl` — a 5-byte
# framed empty `GetServerCapabilities` — and asserts the response carries
# `transport_kind = TRANSPORT_KIND_WSS_BRIDGE`.
#
# The gRPC-Web response is a binary protobuf frame, so we assert on the wire
# bytes rather than JSON: in `ServerCapabilities`, `transport_kind` is field
# 5 (varint), and `TRANSPORT_KIND_WSS_BRIDGE = 3`, which encodes as the byte
# pair `28 03` (tag (5<<3)|0 = 0x28, value 0x03). We also confirm the
# `concerto.v1` schema string and the `grpc-status:0` success trailer to
# prove it's a real, successful caps response.
#
# Requires (from core-boot + the driver): CONCERTO_HOME, FAKE_HOME, and a
# pre-built `concerto-core` binary (core-boot's `cargo build` guarantees
# `target/debug/concerto-core`). SCRIPT_DIR is set by the driver.
# Exports: nothing (self-contained; tears down its own Core).
check_connect_web_bridge() {
    echo "Smoke gate v3: starting a dedicated Core with the Connect-Web bridge enabled..."
    BRIDGE_HOME="$CONCERTO_HOME/bridge"
    BRIDGE_CONFIG_DIR="$BRIDGE_HOME/.concerto"
    BRIDGE_DATA_DIR="$BRIDGE_HOME/concerto"
    mkdir -p "$BRIDGE_CONFIG_DIR" "$BRIDGE_DATA_DIR"

    # Reserve a concrete loopback port (the bridge would OS-assign on `0`,
    # which a subprocess can't report back). Prefer python3; fall back to a
    # high pseudo-random port (wait_for_port catches a rare collision).
    BRIDGE_PORT=""
    if command -v python3 >/dev/null 2>&1; then
        BRIDGE_PORT="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')"
    fi
    if [ -z "$BRIDGE_PORT" ]; then
        BRIDGE_PORT=$(( 20000 + (RANDOM % 20000) ))
    fi
    BRIDGE_ADDR="127.0.0.1:$BRIDGE_PORT"
    BRIDGE_LOG="$CONCERTO_HOME/bridge-core.log"

    # The debug binary core-boot built. Repo root is SCRIPT_DIR/.. .
    CORE_BIN="$SCRIPT_DIR/../target/debug/concerto-core"
    if [ ! -x "$CORE_BIN" ]; then
        echo "FAIL connect-web-bridge"
        fail "concerto-core binary not found at $CORE_BIN (expected core-boot to have built it)"
    fi

    HOME="$FAKE_HOME" \
        CONCERTO_CONFIG_DIR="$BRIDGE_CONFIG_DIR" \
        CONCERTO_DATA_DIR="$BRIDGE_DATA_DIR" \
        CONCERTO_CONNECT_BRIDGE=1 \
        CONCERTO_CONNECT_BRIDGE_ADDR="$BRIDGE_ADDR" \
        "$CORE_BIN" > "$BRIDGE_LOG" 2>&1 &
    BRIDGE_PID=$!

    # Reap the dedicated Core on every exit path of this check.
    # shellcheck disable=SC2064
    trap "kill -TERM $BRIDGE_PID 2>/dev/null || true; wait $BRIDGE_PID 2>/dev/null || true" RETURN

    # Wait for the bridge TCP port to accept (cap ~15s).
    if ! wait_for_port "$BRIDGE_PORT" 15; then
        echo "smoke: bridge core log:" >&2
        sed 's/^/    /' "$BRIDGE_LOG" >&2 || true
        echo "FAIL connect-web-bridge"
        fail "Connect-Web bridge did not accept on $BRIDGE_ADDR within 15s"
    fi
    echo "Smoke gate v3: bridge ready on $BRIDGE_ADDR"

    # Headless gRPC-Web GetServerCapabilities. Empty request → a 5-byte
    # frame: flag 0x00 + 4-byte length 0x00000000.
    REQ_BIN="$CONCERTO_HOME/bridge-caps-req.bin"
    RESP_BIN="$CONCERTO_HOME/bridge-caps-resp.bin"
    printf '\x00\x00\x00\x00\x00' > "$REQ_BIN"
    if ! curl -s --max-time 10 --output "$RESP_BIN" \
        -H 'content-type: application/grpc-web+proto' \
        -H 'x-grpc-web: 1' \
        --data-binary @"$REQ_BIN" \
        "http://$BRIDGE_ADDR/concerto.v1.Runtime/GetServerCapabilities"; then
        echo "smoke: bridge core log:" >&2
        sed 's/^/    /' "$BRIDGE_LOG" >&2 || true
        echo "FAIL connect-web-bridge"
        fail "curl gRPC-Web GetServerCapabilities failed"
    fi

    # Hex-dump the response and assert the wire bytes. `28 03` is
    # transport_kind(field 5)=WSS_BRIDGE(3). `concerto.v1` confirms a real
    # caps message; `grpc-status:0` confirms success.
    RESP_HEX="$(od -An -tx1 "$RESP_BIN" | tr -d ' \n')"
    if ! printf '%s' "$RESP_HEX" | grep -q '2803'; then
        echo "smoke: gRPC-Web caps response (hex): $RESP_HEX" >&2
        echo "smoke: bridge core log:" >&2
        sed 's/^/    /' "$BRIDGE_LOG" >&2 || true
        echo "FAIL connect-web-bridge"
        fail "caps response missing transport_kind=WSS_BRIDGE (bytes 28 03)"
    fi
    if ! grep -qa 'concerto.v1' "$RESP_BIN"; then
        echo "FAIL connect-web-bridge"
        fail "caps response missing concerto.v1 schema marker"
    fi
    if ! grep -qa 'grpc-status:0' "$RESP_BIN"; then
        echo "FAIL connect-web-bridge"
        fail "caps response missing grpc-status:0 success trailer"
    fi

    echo "PASS connect-web-bridge"
}
