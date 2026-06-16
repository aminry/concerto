#!/usr/bin/env bash
# shellcheck shell=bash
#
# gen-bindings.sh — generate the Swift + Kotlin uniffi bindings for the
# `concerto-iroh-ffi` native module (Task 509.5).
#
# `concerto-iroh-ffi` uses uniffi's PROC-MACRO mode (`setup_scaffolding!()` +
# `#[uniffi::export]`, no `.udl`), so bindings are produced in **library mode**:
# the generator reads the uniffi metadata embedded in the already-built cdylib
# and emits the foreign sources. This is the LOAD-BEARING Tier-2 proof that the
# cdylib's uniffi metadata is valid — if the metadata were malformed the
# generator would fail here, on the host, before any mobile toolchain is in play.
#
# What it does:
#   1. builds the host cdylib (`cargo build -p concerto-iroh-ffi`) unless
#      $DYLIB already points at one;
#   2. builds + runs the off-by-default `uniffi-bindgen` helper bin
#      (`--features cli`) against that cdylib, once per language;
#   3. writes Swift to <out>/swift and Kotlin to <out>/kotlin.
#
# Inputs (env, all optional):
#   OUT_DIR  — where to write bindings. Default: target/uniffi-bindings.
#   DYLIB    — path to a prebuilt cdylib to read metadata from. Default:
#              target/debug/libconcerto_iroh_ffi.dylib (Linux: .so).
#   PROFILE  — cargo profile for the auto-build path (debug|release).
#              Default: debug.
#
# Tier: Tier-2. Runs entirely on the host with the stable Rust toolchain — no
# iOS / Android SDK required. The XCFramework / .aar packaging (which DO need
# those SDKs) live in build-xcframework.sh / build-android-aar.sh and are
# Tier-3 where the toolchains are absent.
#
# Linux/macOS only. Bash 3.2 compatible.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# packaging/ -> crates/concerto-iroh-ffi/ -> crates/ -> repo root.
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

OUT_DIR="${OUT_DIR:-$REPO_ROOT/target/uniffi-bindings}"
PROFILE="${PROFILE:-debug}"

# The cdylib extension is platform-specific (.dylib on macOS, .so on Linux).
case "$(uname -s)" in
Darwin) LIB_EXT="dylib" ;;
*) LIB_EXT="so" ;;
esac
DEFAULT_DYLIB="$REPO_ROOT/target/$PROFILE/libconcerto_iroh_ffi.$LIB_EXT"
DYLIB="${DYLIB:-$DEFAULT_DYLIB}"

run_cargo() {
    # Always operate from the repo root so the workspace + features resolve.
    (cd "$REPO_ROOT" && cargo "$@")
}

# 1. Build the cdylib if the caller did not point us at one.
if [ ! -f "$DYLIB" ]; then
    echo "gen-bindings: building host cdylib ($PROFILE)..."
    if [ "$PROFILE" = "release" ]; then
        run_cargo build -p concerto-iroh-ffi --release
    else
        run_cargo build -p concerto-iroh-ffi
    fi
fi

if [ ! -f "$DYLIB" ]; then
    echo "gen-bindings: cdylib not found at $DYLIB" >&2
    exit 1
fi

echo "gen-bindings: reading uniffi metadata from $DYLIB"
mkdir -p "$OUT_DIR/swift" "$OUT_DIR/kotlin"

# 2 + 3. Generate, once per language. `--features cli` enables the helper bin
# (clap/camino/uniffi_bindgen) which the shipped cdylib never carries.
gen() {
    local language="$1"
    local out="$2"
    echo "gen-bindings: generating $language -> $out"
    run_cargo run -q -p concerto-iroh-ffi --features cli --bin uniffi-bindgen -- \
        generate --library "$DYLIB" --language "$language" --out-dir "$out" --no-format
}

gen swift "$OUT_DIR/swift"
gen kotlin "$OUT_DIR/kotlin"

echo
echo "gen-bindings: done. Generated files:"
find "$OUT_DIR" -type f | sort | sed 's/^/  /'

# Belt-and-suspenders: fail loudly if the expected sources are missing (a
# silent empty generation would defeat the Tier-2 proof).
SWIFT_SRC="$OUT_DIR/swift/concerto_iroh_ffi.swift"
KOTLIN_SRC="$OUT_DIR/kotlin/uniffi/concerto_iroh_ffi/concerto_iroh_ffi.kt"
missing=0
[ -f "$SWIFT_SRC" ] || { echo "gen-bindings: MISSING $SWIFT_SRC" >&2; missing=1; }
[ -f "$KOTLIN_SRC" ] || { echo "gen-bindings: MISSING $KOTLIN_SRC" >&2; missing=1; }
[ "$missing" -eq 0 ] || exit 1
