#!/usr/bin/env bash
# Smoke gate for embedded-core mode: builds the desktop binary with the
# feature, then exercises the one-process boot path that scripts/smoke.sh
# (separate daemon) does not. Headless — no GUI launch.
#
# Relationship to the composable gate (Task 108): the daemon gate's
# capability checks under scripts/smoke.d/ all drive Core over its UDS
# socket via the smoke-client. Embedded mode boots Core in-process with no
# UDS surface, so those checks can't be sourced here as-is; the in-process
# boot is instead proven by the cargo integration tests below. When a V1.0
# task exposes an embedded loopback transport, this script can grow to
# source the shared scripts/smoke.d/ checks against it.
set -euo pipefail

echo "smoke-embedded: building desktop with embedded-core feature"
cargo build -p concerto-desktop --features embedded-core

echo "smoke-embedded: core library boot path (boot::start round-trip)"
cargo test -p concerto-core --test embedded_boot -- --nocapture

echo "smoke-embedded: desktop embedded::start scratch boot + teardown"
cargo test -p concerto-desktop --features embedded-core \
    embedded::tests::start_scratch_boots_and_shuts_down -- --nocapture

echo "smoke-embedded: OK"
