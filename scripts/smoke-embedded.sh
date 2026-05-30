#!/usr/bin/env bash
# Smoke gate for embedded-core mode: builds the desktop binary with the
# feature, then exercises the one-process boot path that scripts/smoke.sh
# (separate daemon) does not. Headless — no GUI launch.
set -euo pipefail

echo "smoke-embedded: building desktop with embedded-core feature"
cargo build -p concerto-desktop --features embedded-core

echo "smoke-embedded: core library boot path (boot::start round-trip)"
cargo test -p concerto-core --test embedded_boot -- --nocapture

echo "smoke-embedded: desktop embedded::start scratch boot + teardown"
cargo test -p concerto-desktop --features embedded-core \
    embedded::tests::start_scratch_boots_and_shuts_down -- --nocapture

echo "smoke-embedded: OK"
