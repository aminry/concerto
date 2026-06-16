#!/usr/bin/env bash
# shellcheck shell=bash
#
# build-android-aar.sh — package the `concerto-iroh-ffi` shared library +
# its uniffi Kotlin bindings into an Android library project that Gradle can
# assemble into a ConcertoIroh.aar (Task 509.5, the Android leg).
#
# Pipeline (standard uniffi + cargo-ndk recipe):
#   1. cargo-ndk cross-compiles the CDYLIB (`libconcerto_iroh_ffi.so`) for the
#      three ABIs the React Native Android floor targets:
#        * arm64-v8a      (aarch64-linux-android)   — modern devices
#        * armeabi-v7a    (armv7-linux-androideabi) — older 32-bit devices
#        * x86_64         (x86_64-linux-android)    — emulator
#      cargo-ndk drops each `.so` into the per-ABI jniLibs/ layout for us.
#   2. gen-bindings.sh emits the Kotlin source into the AAR project's
#      src/main/kotlin tree.
#   3. emits a minimal Gradle library project (build.gradle.kts +
#      AndroidManifest.xml) under <out>/aar-project so a host with the Android
#      SDK can `./gradlew assembleRelease` to produce the .aar. (We DO NOT run
#      Gradle here — the SDK is not assumed; this stages everything it needs.)
#
# Inputs (env):
#   ANDROID_NDK_HOME / NDK_HOME / ANDROID_HOME — at least one must point at an
#                     installed NDK for cargo-ndk to find the toolchain.
#   OUT_DIR         — output dir. Default: target/android-aar.
#   PROFILE         — cargo profile (debug|release). Default: release.
#   API_LEVEL       — minSdk / NDK platform level. Default: 24.
#
# Tier: Tier-3 where the Android NDK / cargo-ndk are absent. The script
# REQUIRES rustup with the android targets added + cargo-ndk + an NDK. It exits
# non-zero with a clear message when any is missing, rather than faking a build.
# On default CI (no Android SDK assumed) it is NOT run; the native.yml lane only
# link-checks the cdylib for the android target (cheap, needs only the NDK
# linker), and the full .aar is the phase-gate Tier-3 step.
#
# Linux/macOS. Bash 3.2 compatible.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

OUT_DIR="${OUT_DIR:-$REPO_ROOT/target/android-aar}"
PROFILE="${PROFILE:-release}"
API_LEVEL="${API_LEVEL:-24}"
CRATE="concerto-iroh-ffi"
SO="libconcerto_iroh_ffi.so"

# --- preconditions -----------------------------------------------------------

need() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "build-android-aar: '$1' not found on PATH — Tier-3 (NDK toolchain absent)" >&2
        exit 1
    }
}
need cargo

if ! command -v rustup >/dev/null 2>&1; then
    echo "build-android-aar: rustup not found — the android std targets cannot be" >&2
    echo "  added without it. Install rustup, then:" >&2
    echo "    rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android" >&2
    echo "  (Honest Tier-3 boundary on a Homebrew-rustc host.)" >&2
    exit 1
fi

need cargo-ndk

NDK="${ANDROID_NDK_HOME:-${NDK_HOME:-}}"
if [ -z "$NDK" ]; then
    echo "build-android-aar: set ANDROID_NDK_HOME (or NDK_HOME) to an installed NDK." >&2
    echo "  (Honest Tier-3 boundary: no Android NDK present.)" >&2
    exit 1
fi

# cargo-ndk maps these android ABIs to the rust targets internally; we assert
# the rust std targets are present so the failure is actionable.
for t in aarch64-linux-android armv7-linux-androideabi x86_64-linux-android; do
    if ! rustup target list --installed | grep -qx "$t"; then
        echo "build-android-aar: missing rust target '$t'. Run:" >&2
        echo "    rustup target add $t" >&2
        exit 1
    fi
done

# --- stage the AAR project layout -------------------------------------------

PROJECT="$OUT_DIR/aar-project"
JNI_LIBS="$PROJECT/src/main/jniLibs"
KOTLIN_SRC="$PROJECT/src/main/kotlin"
rm -rf "$PROJECT"
mkdir -p "$JNI_LIBS" "$KOTLIN_SRC"

# --- cross-compile the .so per ABI via cargo-ndk ----------------------------

echo "build-android-aar: cargo-ndk build (ABIs: arm64-v8a armeabi-v7a x86_64)..."
NDK_PROFILE_FLAG=""
[ "$PROFILE" = "release" ] && NDK_PROFILE_FLAG="--release"

(
    cd "$REPO_ROOT"
    ANDROID_NDK_HOME="$NDK" cargo ndk \
        -t arm64-v8a -t armeabi-v7a -t x86_64 \
        --platform "$API_LEVEL" \
        -o "$JNI_LIBS" \
        build -p "$CRATE" $NDK_PROFILE_FLAG
)

# Sanity: cargo-ndk drops one .so per ABI under jniLibs/<abi>/.
for abi in arm64-v8a armeabi-v7a x86_64; do
    if [ ! -f "$JNI_LIBS/$abi/$SO" ]; then
        echo "build-android-aar: expected $JNI_LIBS/$abi/$SO not produced" >&2
        exit 1
    fi
done

# --- Kotlin bindings ---------------------------------------------------------

echo "build-android-aar: generating uniffi Kotlin bindings..."
BINDINGS_OUT="$OUT_DIR/bindings"
OUT_DIR="$BINDINGS_OUT" PROFILE="$PROFILE" "$SCRIPT_DIR/gen-bindings.sh"
# uniffi emits kotlin under uniffi/concerto_iroh_ffi/; copy that package tree
# into the project's source root.
cp -R "$BINDINGS_OUT/kotlin/." "$KOTLIN_SRC/"

# --- minimal Gradle library project -----------------------------------------

cat >"$PROJECT/build.gradle.kts" <<'GRADLE'
// Generated by crates/concerto-iroh-ffi/packaging/build-android-aar.sh (Task 509.5).
// Minimal Android library that bundles the cross-compiled libconcerto_iroh_ffi.so
// (jniLibs/) + the uniffi Kotlin bindings. `./gradlew assembleRelease` produces
// build/outputs/aar/ConcertoIroh-release.aar.
plugins {
    id("com.android.library")
    kotlin("android")
}
android {
    namespace = "com.concerto.iroh"
    compileSdk = 34
    defaultConfig { minSdk = 24 }
}
dependencies {
    // uniffi 0.28 Kotlin bindings call into net.java.dev.jna for the C ABI.
    implementation("net.java.dev.jna:jna:5.14.0@aar")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.8.0")
}
GRADLE

cat >"$PROJECT/src/main/AndroidManifest.xml" <<'MANIFEST'
<?xml version="1.0" encoding="utf-8"?>
<manifest xmlns:android="http://schemas.android.com/apk/res/android" />
MANIFEST

echo
echo "build-android-aar: staged Android library project."
echo "  jniLibs: $JNI_LIBS/{arm64-v8a,armeabi-v7a,x86_64}/$SO"
echo "  kotlin:  $KOTLIN_SRC/uniffi/concerto_iroh_ffi/"
echo "  next:    (with the Android SDK) cd '$PROJECT' && ./gradlew assembleRelease"
