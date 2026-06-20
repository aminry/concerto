# `concerto-iroh-ffi` packaging (Task 509.5)

Packaging + cross-compile tooling that turns the `concerto-iroh-ffi` uniffi
cdylib (Task 509) into the artifacts the mobile shell consumes:

- an **iOS** `ConcertoIroh.xcframework` (+ the generated Swift source), and
- an **Android** library project that assembles a `ConcertoIroh.aar` (+ the
  generated Kotlin source).

The crate uses uniffi **proc-macro mode** (`setup_scaffolding!()` +
`#[uniffi::export]`, no `.udl`), so bindings are generated in **library mode**:
the generator reads the uniffi metadata embedded in the built cdylib. The crate
is out of `default-members`, so none of this gates the default `cargo build` /
the Core/CLI CI lanes.

## Scripts

| Script | What it does | Tier |
|---|---|---|
| `gen-bindings.sh` | Builds the host cdylib, then runs the `uniffi-bindgen` helper bin (`--features cli`) to emit Swift + Kotlin. | **Tier-2** — host-only, no mobile SDK. This is the load-bearing proof the cdylib's uniffi metadata is valid. |
| `build-xcframework.sh` | Cross-compiles the iOS staticlib (device + sim arm64/x86_64), `lipo`s the sim slices, then `xcodebuild -create-xcframework`. | **Tier-3** — needs macOS + Xcode + rustup with the apple-ios targets. |
| `build-android-aar.sh` | `cargo-ndk` cross-compiles the `.so` for arm64-v8a / armeabi-v7a / x86_64, stages a Gradle library project with the Kotlin bindings. | **Tier-3** — needs the Android NDK + cargo-ndk + rustup with the android targets. |

Each Tier-3 script **exits non-zero with a clear message** when its toolchain is
absent (no Xcode, no rustup target, no NDK) rather than faking a build.

## Quick start (host bindgen — works anywhere with stable Rust)

```sh
./crates/concerto-iroh-ffi/packaging/gen-bindings.sh
# -> target/uniffi-bindings/swift/concerto_iroh_ffi.swift
#    target/uniffi-bindings/swift/concerto_iroh_ffiFFI.h
#    target/uniffi-bindings/swift/concerto_iroh_ffiFFI.modulemap
#    target/uniffi-bindings/kotlin/uniffi/concerto_iroh_ffi/concerto_iroh_ffi.kt
```

## iOS XCFramework (Tier-3 — Xcode + rustup)

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
./crates/concerto-iroh-ffi/packaging/build-xcframework.sh
# -> target/xcframework/ConcertoIroh.xcframework
#    target/xcframework/concerto_iroh_ffi.swift
```

## Android AAR (Tier-3 — Android NDK + cargo-ndk + rustup)

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk
export ANDROID_NDK_HOME=/path/to/android-ndk
./crates/concerto-iroh-ffi/packaging/build-android-aar.sh
# -> target/android-aar/aar-project/  (then: cd there && ./gradlew assembleRelease)
```

## CI

`.github/workflows/native.yml`:

- **`bindgen`** (ubuntu, always) — runs `gen-bindings.sh` and asserts the
  Swift + Kotlin files exist (uploaded as the `uniffi-bindings` artifact). The
  Tier-2 gate.
- **`link-check`** (matrix) — cross-compile **link-check** (`cargo build
  --target …`, not an on-device run) for `aarch64-apple-ios[-sim]` on macOS and
  `aarch64/x86_64-linux-android` (via cargo-ndk) on Linux. Uses the Xcode /
  Android NDK the GitHub runners already ship; no extra SDK download.

iOS/Android **SDK** toolchains (full `xcodebuild -create-xcframework`,
`gradlew assembleRelease`, simulator/device runs) are **not** assumed in default
CI — those are the phase-gate **Tier-3** steps the two `build-*.sh` scripts
cover on a host that has the toolchains.

## Honest local Tier-3 note

On a Homebrew-`rustc` host (no `rustup`), the apple-ios / linux-android std
libraries cannot be installed, so a local `cargo build --target aarch64-apple-ios`
fails with `error[E0463]: can't find crate for 'core'` (`= help: consider
downloading the target with 'rustup target add …'`). The bindgen Tier-2 proof
still runs locally; the cross-compile link-check is valid only where rustup can
add the targets (the CI lane uses `dtolnay/rust-toolchain` with `targets:`).
