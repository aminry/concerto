# shellcheck shell=bash
# Capability: transport-loopback (Task 212).
#
# Proves the production Iroh transport (design/11): TWO in-process Iroh
# endpoints on one host with relays DISABLED, forced onto the direct loopback
# path (the spike's Tier-2 model). It exercises, end to end with NO network:
#
#   - gRPC-over-Iroh via the hand-rolled tonic-0.12 ↔ Iroh-bidi-stream adapter
#     (the four spike-102 gotchas: poll_* shadowing, one-gRPC-conn-per-bidi,
#     acceptor priming, ≥64 MiB message limits),
#   - the Noise IK session layer (Task 208) INSIDE each API stream,
#   - the three-channel multiplexing (the channel-tag framing),
#   - `serve_iroh` shared-dispatch into the SAME Tonic handlers UDS serves, with
#     `ConnTransport(TransportKind::Iroh)` tagged via the Task-201 seam,
#   - and the assertion `transport_kind = IROH` over a real GetServerCapabilities.
#
# Unlike the other smoke checks this one does NOT use the shared smoke Core
# (which boots UDS-only): it drives the transport + `serve_iroh` directly via
# two hermetic `cargo test` invocations, so it needs NO keychain / no relay /
# no network. The transport's own loopback double (`concerto-transport`, behind
# the `dev-relay` feature) covers one RPC + one stream + the four gotchas + the
# disable_remote refusal + ConnectionPath classification; the Core end-to-end
# test (`concerto-core --test transport_iroh`) drives the REAL `serve_iroh` +
# the real Runtime handler + a real device cert and asserts `transport_kind =
# IROH`.
#
# What this does NOT cover (→ Phase-2 Tier-3 manual checklist / spike 101 field
# matrix): real-NAT hole-punch, a real WAN relay, real connection migration.
#
# Requires (from core-boot + the driver): SCRIPT_DIR. Self-contained; exports
# nothing.
check_transport_loopback() {
    echo "Smoke gate v3: transport-loopback — two Iroh endpoints, relays disabled..."
    REPO_ROOT="$SCRIPT_DIR/.."

    # 1) The transport crate's Tier-2 loopback double (gRPC-over-Iroh + adapter
    #    + Noise + channel mux + the four gotchas + disable_remote + path class).
    if ! ( cd "$REPO_ROOT" && cargo test -q -p concerto-transport --features dev-relay --test loopback ) ; then
        echo "FAIL transport-loopback"
        fail "concerto-transport loopback double failed (gRPC-over-Iroh + adapter + Noise)"
    fi

    # 2) The Core end-to-end IROH assertion over the real serve_iroh path:
    #    GetServerCapabilities.transport_kind == IROH.
    if ! ( cd "$REPO_ROOT" && cargo test -q -p concerto-core --test transport_iroh ) ; then
        echo "FAIL transport-loopback"
        fail "concerto-core serve_iroh end-to-end (transport_kind = IROH) failed"
    fi

    echo "PASS transport-loopback"
}
