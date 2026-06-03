# shellcheck shell=bash
# Capability: mdns-discovery (Task 213).
#
# Proves mDNS LAN discovery (design/11 §3.5): TWO in-process mDNS daemons on one
# host — a RESPONDER publishing `_concerto._tcp.local` and a BROWSER browsing for
# it — discover each other over the LOOPBACK interface (which `mdns-sd` enables
# by default), recovering the EXACT four-key TXT schema
# (endpoint_id/core_pubkey/version/caps). It also asserts:
#
#   - IPv4 + IPv6 records where the interface delivers them (R-3),
#   - the dedicated opt-out SUPPRESSES publication, and
#   - `disable_remote = true` does NOT silence mDNS (design/11 §6.4) — driven
#     through the real `IrohTransport::publish_mdns` path.
#
# Like the transport-loopback check, this does NOT use the shared smoke Core
# (which boots UDS-only): it drives the mDNS responder + browser directly via one
# hermetic `cargo test` invocation, so it needs NO keychain / no relay / no
# real network. The loopback double's waits are all timeout-bounded and degrade
# gracefully if a CI sandbox blocks even loopback multicast (asserting the
# TXT encode/parse + responder/browser construction directly), so it never hangs
# and never silently passes on nothing.
#
# What this does NOT cover (→ Phase-2 Tier-3 manual checklist): real cross-device
# LAN discovery across two physical machines on real Wi-Fi (multicast on a real
# switch, mDNS-suppressing work networks, IPv6-only segments). That is "pair a
# real second machine over LAN via mDNS direct" on the Phase-2 manual checklist.
#
# Requires (from core-boot + the driver): SCRIPT_DIR. Self-contained; exports
# nothing.
check_mdns_discovery() {
    echo "Smoke gate v3: mdns-discovery — responder + browser on loopback..."
    REPO_ROOT="$SCRIPT_DIR/.."

    if ! ( cd "$REPO_ROOT" && cargo test -q -p concerto-transport --test mdns_loopback ) ; then
        echo "FAIL mdns-discovery"
        fail "concerto-transport mDNS loopback double failed (publish + browse + TXT schema + opt-out)"
    fi

    echo "PASS mdns-discovery"
}
