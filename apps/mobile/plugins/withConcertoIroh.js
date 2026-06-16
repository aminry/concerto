// Expo config plugin for the ConcertoIroh native module (Task 509 cdylib / 511
// wiring). Registered in app.json's `expo.plugins`; runs at `expo prebuild` to
// link the hand-rolled uniffi cdylib (`crates/concerto-iroh-ffi`) into the
// generated iOS/Android projects so `requireNativeModule("ConcertoIroh")`
// resolves on a dev-client / production build.
//
// SCOPE (Tier-2 vs Tier-3): this file establishes the plugin SEAM and documents
// the link steps. The ACTUAL native build — compiling the Rust cdylib for each
// target triple, dropping the artifacts/headers into the autolinked module, and
// generating the uniffi JS bindings — is Tier-3 (a real toolchain + device).
// Until that lands, the plugin is intentionally a no-op pass-through so:
//   - the Expo config stays valid (prebuild/CI doesn't choke on a missing plugin),
//   - the app's transport selection (`hasNativeConcertoIroh()`) simply reports
//     "no native module" in Expo Go / tests and falls back to the mock.
//
// To finish the wiring (Tier-3, follow-up task):
//   1. Build the cdylib per target:
//        - iOS:     aarch64-apple-ios, aarch64-apple-ios-sim, x86_64-apple-ios
//                   → an XCFramework (libconcerto_iroh_ffi.a + module.modulemap).
//        - Android: arm64-v8a / armeabi-v7a / x86_64
//                   → jniLibs/<abi>/libconcerto_iroh_ffi.so.
//   2. Generate the uniffi JS/Swift/Kotlin bindings from the .udl/proc-macro
//      metadata and place the JS shim where `requireNativeModule("ConcertoIroh")`
//      finds it (an Expo Module wrapper around the uniffi scaffolding).
//   3. Expand this plugin to copy those artifacts via `withDangerousMod` /
//      `withXcodeProject` / `withGradleProperties` so prebuild is reproducible.
//
// @param {import('@expo/config-plugins').ExpoConfig} config
// @param {{ crate?: string }} [_props]
// @returns {import('@expo/config-plugins').ExpoConfig}
const withConcertoIroh = (config, _props = {}) => {
  // No-op until the Tier-3 native link lands. Returning the config unchanged
  // keeps `expo prebuild` / config resolution working today.
  return config;
};

module.exports = withConcertoIroh;
