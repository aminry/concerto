# shellcheck shell=bash
# Capability: split-host-loopback (Task 220) — the Tier-2 capstone of the
# Phase-2 transport spine.
#
# Brings up TWO Iroh endpoints on one host with relays DISABLED (the spike's
# direct-loopback model — no NAT, no relay, no network) and drives the full
# remote-client path over the Iroh transport + Noise IK, in one driver process
# (`tools/split-host-loopback`):
#
#   1. boots an Iroh-enabled Core in-process (CONCERTO_ENABLE_IROH=1, Task
#      217.5), keychain-isolated under a unique CONCERTO_KEYCHAIN_SERVICE;
#   2. pairs a synthetic device over the real 0x03 pairing channel (Noise XX
#      over the one-shot token → SignedDeviceCert);
#   3. over the authenticated Iroh channel: an IROH-tagged unary
#      (GetServerCapabilities == IROH), a Streams.Subscribe(workspace.events)
#      stream, and a Files.Upload/Download round-trip (+ BLAKE2b-256) into the
#      workarea's .context/;
#   4. tears the Core (and its Iroh endpoint) down cleanly — no leaked process.
#
# macOS-ONLY at runtime, SKIP-CLEAN elsewhere. The booted Iroh path is
# keychain-backed (the Core's Ed25519 cert issuer + its Noise static); the
# `keyring` backend only works on macOS in V1.0 (Task 217.5 Handoff). On a
# keychain-less lane (Linux/Windows CI) RunningCore::iroh() degrades to None, the
# cert mint/validate issuers diverge, and the round-trip can't run. So on
# non-macOS this check SKIPs cleanly (PASS with a "skipped on $(uname)" note) and
# the ubuntu smoke lane stays green. The driver bin still BUILDS on every lane.
#
# What this does NOT cover (→ Phase-2 Tier-3 manual checklist, design/11 §10):
# real cross-machine split-host, real NAT diversity / direct-connection %, relay
# fallback, Wi-Fi↔LTE migration, throughput-vs-UDS budgets.
#
# Self-contained: it boots its OWN Iroh Core (separate from the shared UDS smoke
# Core) and builds its OWN project→repo→workspace→workarea chain over Iroh from a
# caller-seeded local bare repo (file://, no network). It does NOT touch the
# shared chain's WA_ID/SID and does NOT edit 00-core-boot.sh.
#
# Requires (from core-boot + the driver): CONCERTO_HOME, SCRIPT_DIR.
# Self-contained; exports nothing.
check_split_host_loopback() {
    # --- Skip cleanly on non-macOS (keychain-backed Iroh is macOS-only) -----
    uname_s="$(uname -s)"
    if [ "$uname_s" != "Darwin" ]; then
        echo "SKIP split-host-loopback (Iroh boot is keychain-backed → macOS-only until the Linux/Windows keychain backends land; skipped on $uname_s)"
        echo "PASS split-host-loopback"
        return 0
    fi

    echo "Smoke gate v3: split-host-loopback — two Iroh endpoints, relays disabled, pair + IROH unary + stream + Files..."
    REPO_ROOT="$SCRIPT_DIR/.."

    # Pre-build the driver so compile time stays out of the assertion wall
    # clock (like 00-core-boot.sh / 95-cli.sh).
    echo "Smoke gate v3: building split-host-loopback driver..."
    if ! ( cd "$REPO_ROOT" && cargo build --quiet -p concerto-split-host-loopback ); then
        echo "FAIL split-host-loopback"
        fail "split-host-loopback driver build"
    fi

    # Isolated scratch dirs for the driver's own Iroh Core (NOT the shared
    # smoke Core's dirs — this is a second, additive Core).
    SHL_ROOT="$CONCERTO_HOME/split-host-loopback"
    SHL_DATA="$SHL_ROOT/data"
    SHL_CONFIG="$SHL_ROOT/config"
    SHL_BARE="$SHL_ROOT/bare-repo.git"
    mkdir -p "$SHL_DATA" "$SHL_CONFIG"

    # Seed a local bare repo (file://, no network) the driver clones over Iroh —
    # same shape as 10-project-repo-clone.sh. Keeping the git shell-outs here
    # (not in the Rust bin) is the established split.
    echo "Smoke gate v3: seeding split-host-loopback bare repo..."
    git init --bare --quiet "$SHL_BARE"
    git -C "$SHL_BARE" symbolic-ref HEAD refs/heads/main
    SHL_SEED="$SHL_ROOT/seed"
    git clone --quiet "$SHL_BARE" "$SHL_SEED"
    echo "# split-host-loopback smoke" > "$SHL_SEED/README.md"
    git -C "$SHL_SEED" add -A
    git -C "$SHL_SEED" -c user.email=smoke@test -c user.name=Smoke commit -m "seed" --quiet
    git -C "$SHL_SEED" push --quiet origin main

    # Keychain isolation (KEYCHAIN-IN-CI hazard): a unique service so the
    # driver's Core never prompts / hangs. Iroh toggle ON for this Core only.
    SHL_KEYCHAIN_SERVICE="concerto-smoke-$$-split-host-loopback"
    SHL_LOG="$SHL_ROOT/driver.log"

    # The driver bounds every step internally (≤20s/step + ≤10s shutdown). We
    # add an outer wall-clock cap, implemented portably (macOS has no `timeout`)
    # by polling the driver pid against a deadline and killing on overrun, so the
    # smoke gate never hangs at exit.
    echo "Smoke gate v3: running split-host-loopback driver (cap 180s)..."
    SHL_DRIVER_BIN="$REPO_ROOT/target/debug/split-host-loopback"
    CONCERTO_ENABLE_IROH=1 \
        CONCERTO_KEYCHAIN_SERVICE="$SHL_KEYCHAIN_SERVICE" \
        CONCERTO_CONFIG_DIR="$SHL_CONFIG" \
        CONCERTO_DATA_DIR="$SHL_DATA" \
        "$SHL_DRIVER_BIN" \
        --data-dir "$SHL_DATA" \
        --config-dir "$SHL_CONFIG" \
        --bare-repo "$SHL_BARE" \
        > "$SHL_LOG" 2>&1 &
    SHL_PID=$!

    SHL_DEADLINE=$(( $(date +%s) + 180 ))
    while pid_alive "$SHL_PID"; do
        if [ "$(date +%s)" -ge "$SHL_DEADLINE" ]; then
            kill -TERM "$SHL_PID" 2>/dev/null || true
            wait "$SHL_PID" 2>/dev/null || true
            echo "smoke: split-host-loopback driver log:" >&2
            sed 's/^/    /' "$SHL_LOG" >&2 || true
            echo "FAIL split-host-loopback"
            fail "split-host-loopback driver exceeded the 180s wall-clock cap"
        fi
        sleep 0.5
    done
    SHL_RC=0
    wait "$SHL_PID" || SHL_RC=$?

    if [ "$SHL_RC" -ne 0 ]; then
        echo "smoke: split-host-loopback driver log:" >&2
        sed 's/^/    /' "$SHL_LOG" >&2 || true
        echo "FAIL split-host-loopback"
        fail "split-host-loopback driver exited $SHL_RC"
    fi

    # The driver prints `split-host-loopback: OK` on the full round-trip, or
    # `split-host-loopback: iroh-unavailable` if RunningCore::iroh() degraded to
    # None (which should not happen on macOS with a real keychain — treat it as
    # a failure here since we already gated on Darwin).
    if grep -q "split-host-loopback: iroh-unavailable" "$SHL_LOG"; then
        echo "smoke: split-host-loopback driver log:" >&2
        sed 's/^/    /' "$SHL_LOG" >&2 || true
        echo "FAIL split-host-loopback"
        fail "Iroh listener unavailable on macOS (keychain access failed) — expected a live round-trip"
    fi
    if ! grep -q "split-host-loopback: OK" "$SHL_LOG"; then
        echo "smoke: split-host-loopback driver did not report OK:" >&2
        sed 's/^/    /' "$SHL_LOG" >&2 || true
        echo "FAIL split-host-loopback"
        fail "split-host-loopback driver did not confirm OK"
    fi

    echo "PASS split-host-loopback"
}
