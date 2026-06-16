#!/usr/bin/env bash
# shellcheck shell=bash
#
# build-xcframework.sh — package the `concerto-iroh-ffi` static library +
# its uniffi Swift bindings into a ConcertoIroh.xcframework for iOS (Task
# 509.5, the iOS leg).
#
# Pipeline (mirrors the standard uniffi + Rust-on-iOS recipe):
#   1. cross-compile the STATICLIB (`libconcerto_iroh_ffi.a`) for:
#        * aarch64-apple-ios          (device, arm64)
#        * aarch64-apple-ios-sim      (simulator, Apple-silicon)
#        * x86_64-apple-ios           (simulator, Intel)
#   2. lipo the two simulator slices into one fat `.a`;
#   3. run gen-bindings.sh to emit the Swift source + the C header +
#      modulemap (renamed to module.modulemap inside a Headers/ dir, which
#      `xcodebuild -create-xcframework` requires for each library);
#   4. `xcodebuild -create-xcframework` over the device `.a` + the fat
#      simulator `.a`, each paired with the Headers/ dir, into
#      <out>/ConcertoIroh.xcframework;
#   5. copy the generated Swift source next to the xcframework so the Expo
#      config plugin / CocoaPods podspec (owned by the mobile agent) can add
#      both to the app target.
#
# Inputs (env, all optional):
#   OUT_DIR   — output dir. Default: target/xcframework.
#   PROFILE   — cargo profile (debug|release). Default: release.
#
# Tier: Tier-3 where the iOS targets / Xcode are absent. The script REQUIRES
#   * rustup with the apple-ios targets added
#     (`rustup target add aarch64-apple-ios aarch64-apple-ios-sim
#       x86_64-apple-ios`), and
#   * a macOS host with Xcode (`xcodebuild`, `lipo`).
# It exits non-zero with a clear message when either is missing, rather than
# faking a build. On default CI (no Apple toolchain assumed) it is NOT run; the
# native.yml `xcframework` job is gated to macOS + opt-in.
#
# macOS only (Xcode-only tooling). Bash 3.2 compatible.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

OUT_DIR="${OUT_DIR:-$REPO_ROOT/target/xcframework}"
PROFILE="${PROFILE:-release}"
CRATE="concerto-iroh-ffi"
LIB="libconcerto_iroh_ffi.a"
FRAMEWORK="ConcertoIroh.xcframework"

# --- preconditions -----------------------------------------------------------

if [ "$(uname -s)" != "Darwin" ]; then
    echo "build-xcframework: macOS + Xcode required (uname=$(uname -s))" >&2
    echo "  This is an honest Tier-3 boundary: iOS packaging needs Xcode." >&2
    exit 1
fi

need() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "build-xcframework: '$1' not found on PATH — Tier-3 (Xcode/rustup absent)" >&2
        exit 1
    }
}
need cargo
need xcodebuild
need lipo

# rustup is how iOS std targets are installed; Homebrew rustc cannot add them.
if ! command -v rustup >/dev/null 2>&1; then
    echo "build-xcframework: rustup not found — the apple-ios std targets cannot" >&2
    echo "  be added without it. Install rustup, then:" >&2
    echo "    rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios" >&2
    echo "  (Honest Tier-3 boundary on a Homebrew-rustc host.)" >&2
    exit 1
fi

DEVICE_TARGET="aarch64-apple-ios"
SIM_ARM_TARGET="aarch64-apple-ios-sim"
SIM_X86_TARGET="x86_64-apple-ios"

for t in "$DEVICE_TARGET" "$SIM_ARM_TARGET" "$SIM_X86_TARGET"; do
    if ! rustup target list --installed | grep -qx "$t"; then
        echo "build-xcframework: missing rust target '$t'. Run:" >&2
        echo "    rustup target add $t" >&2
        exit 1
    fi
done

# --- build the static lib per target ----------------------------------------

build_target() {
    local target="$1"
    echo "build-xcframework: cargo build $target ($PROFILE)..."
    if [ "$PROFILE" = "release" ]; then
        (cd "$REPO_ROOT" && cargo build -p "$CRATE" --target "$target" --release)
    else
        (cd "$REPO_ROOT" && cargo build -p "$CRATE" --target "$target")
    fi
}

build_target "$DEVICE_TARGET"
build_target "$SIM_ARM_TARGET"
build_target "$SIM_X86_TARGET"

TARGET_DIR="$REPO_ROOT/target"
DEVICE_A="$TARGET_DIR/$DEVICE_TARGET/$PROFILE/$LIB"
SIM_ARM_A="$TARGET_DIR/$SIM_ARM_TARGET/$PROFILE/$LIB"
SIM_X86_A="$TARGET_DIR/$SIM_X86_TARGET/$PROFILE/$LIB"

# --- fat simulator slice -----------------------------------------------------

WORK="$OUT_DIR/work"
rm -rf "$WORK"
mkdir -p "$WORK/sim" "$WORK/headers"
SIM_FAT_A="$WORK/sim/$LIB"
echo "build-xcframework: lipo simulator slices -> $SIM_FAT_A"
lipo -create "$SIM_ARM_A" "$SIM_X86_A" -output "$SIM_FAT_A"

# --- headers (C header + module.modulemap) -----------------------------------

# gen-bindings emits concerto_iroh_ffiFFI.h + .modulemap + the Swift source.
echo "build-xcframework: generating uniffi bindings..."
BINDINGS_OUT="$OUT_DIR/bindings"
OUT_DIR="$BINDINGS_OUT" PROFILE="$PROFILE" "$SCRIPT_DIR/gen-bindings.sh"

SWIFT_BINDINGS="$BINDINGS_OUT/swift"
cp "$SWIFT_BINDINGS/concerto_iroh_ffiFFI.h" "$WORK/headers/"
# xcodebuild wants the modulemap named exactly module.modulemap in the headers
# dir it is handed.
cp "$SWIFT_BINDINGS/concerto_iroh_ffiFFI.modulemap" "$WORK/headers/module.modulemap"

# --- assemble the xcframework ------------------------------------------------

rm -rf "${OUT_DIR:?}/$FRAMEWORK"
echo "build-xcframework: xcodebuild -create-xcframework -> $OUT_DIR/$FRAMEWORK"
xcodebuild -create-xcframework \
    -library "$DEVICE_A" -headers "$WORK/headers" \
    -library "$SIM_FAT_A" -headers "$WORK/headers" \
    -output "$OUT_DIR/$FRAMEWORK"

# Ship the Swift source alongside the framework (the app target compiles it).
cp "$SWIFT_BINDINGS/concerto_iroh_ffi.swift" "$OUT_DIR/"

echo
echo "build-xcframework: done."
echo "  framework: $OUT_DIR/$FRAMEWORK"
echo "  swift:     $OUT_DIR/concerto_iroh_ffi.swift"
