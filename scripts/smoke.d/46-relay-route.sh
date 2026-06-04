# shellcheck shell=bash
# Capability: relay-route (Task 214).
#
# Proves the self-hosted relay (design/11 §3.2, §6.2): the `concerto-relay`
# library stood up IN-PROCESS (the embedded `iroh-relay` dev server on an
# OS-assigned loopback port — the spike §6 construction), with two Iroh
# endpoints registering with it and routing a relayed gRPC session through it.
# IP transports are CLEARED on both endpoints, so the ONLY viable QUIC path is
# the relay. It exercises, end to end with NO network beyond loopback:
#
#   - the embedded relay library (`Relay::start`) — iroh-relay embedded as a
#     library, no new protocol (R-7),
#   - a real relayed gRPC unary round-trip + server-streaming firehose over the
#     Task-212 adapter + Noise IK,
#   - the routing-table lifecycle: register → route in `RelayState` with a
#     refreshing 90s TTL → eviction,
#   - the `MAX_ROUTES` + `BANDWIDTH_CAP_PER_ENDPOINT` caps (enforced + counted),
#   - the Prometheus endpoint scraped over real HTTP: `concerto_relay_routes`
#     >= 1 and `concerto_relay_bytes_forwarded_total` > 0 after the transfer,
#     hole-punch success/attempt labelled by region,
#   - the ciphertext-only posture (design/11 §3.9): the relay's observable
#     surface carries only metadata, never the plaintext payload.
#
# Like transport-loopback / mdns-discovery, this does NOT use the shared smoke
# Core (which boots UDS-only): it drives the relay + endpoints directly via one
# hermetic `cargo test` invocation, so it needs NO keychain / no external relay
# binary / no real network. Every wait inside the test is timeout-bounded so a
# headless CI runner can never hang.
#
# What this does NOT cover (→ Phase-2 Tier-3 manual checklist / the spike's
# PENDING real-WAN-relayed row): a relay on REAL infrastructure routing a REAL
# remote client over a real WAN (real relay-server distance, real RTT, real
# bandwidth limits, anycast routing). That is "deploy the relay on real infra and
# route a remote client through it" on the Phase-2 manual checklist.
#
# Requires (from core-boot + the driver): SCRIPT_DIR. Self-contained; exports
# nothing.
check_relay_route() {
    echo "Smoke gate v3: relay-route — in-process relay, two endpoints, IP transports cleared..."
    REPO_ROOT="$SCRIPT_DIR/.."

    if ! ( cd "$REPO_ROOT" && cargo test -q -p concerto-relay --test relay_route ) ; then
        echo "FAIL relay-route"
        fail "concerto-relay relay-route double failed (embedded relay + relayed gRPC + routing-table TTL + caps + Prometheus + ciphertext-only)"
    fi

    echo "PASS relay-route"
}
