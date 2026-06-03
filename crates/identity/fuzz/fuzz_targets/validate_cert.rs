//! cargo-fuzz target on cert validation (Task 208, `design/12 §10`:
//! "Fuzz `validate_cert` with malformed input — cargo-fuzz").
//!
//! Feeds arbitrary attacker-controlled bytes (plus a fixed Core public key) to
//! the two public cert-validation entry points and asserts **panic-freedom +
//! totality**: every input must return `Ok` or `Err`, never panic, never
//! overflow, never UB. This is the trust boundary the auth middleware (Task
//! 210) sits behind, so it must be hardened against malformed wire input.
//!
//! - [`concerto_identity::verify_cert`] — signature + structural-validity check
//!   (Task 205); the raw on-wire form is `cert_bytes || signature`.
//! - [`concerto_identity::DeviceCertIssuer::validate`] via the V1.0
//!   [`concerto_identity::LocalCoreIssuer`] (Task 206) — adds expiry +
//!   revocation policy atop `verify_cert`.
//!
//! Both are total: a successful `Ok` is fine (the fuzzer can in principle forge
//! nothing without the private key, so `Ok` is astronomically unlikely, but is
//! a valid non-panicking outcome). The ONLY failure this target can surface is
//! a panic / abort, which libFuzzer catches.
//!
//! # CI compile gate vs local run
//!
//! CI's job is the **compile gate** only: `cargo +nightly fuzz build` (or
//! `cargo +nightly build` inside `crates/identity/fuzz`) must link the target.
//! Running the fuzzer to convergence is a manual local step:
//!
//! ```sh
//! cargo +nightly fuzz run validate_cert -- -max_total_time=60
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;

use concerto_identity::{
    new_revoked_set, DeviceCertIssuer, KeyPair, LocalCoreIssuer, PublicKey,
};

/// A fixed Core identity for the fuzz run (deterministic — the fuzzer varies
/// only the cert bytes, not the verifying key). All-`0x42`, matching the cert
/// known-answer vector's Core seed.
const CORE_SEED: [u8; 32] = [0x42; 32];

fuzz_target!(|data: &[u8]| {
    // Reconstruct the fixed Core identity once per input (cheap; keeps the
    // target free of global state). `from_seed` is infallible.
    let core_key = KeyPair::from_seed(&CORE_SEED);
    let core_pub: PublicKey = core_key.verifying_key();

    // (1) The low-level signature + structural check (Task 205). Must be total.
    let _ = concerto_identity::verify_cert(data, &core_pub);

    // (2) The full V1.0 validate policy (Task 206): verify_cert + core-match +
    // expiry + revocation. Must also be total on arbitrary bytes.
    let issuer = LocalCoreIssuer::new(
        KeyPair::from_seed(&CORE_SEED),
        core_pub,
        new_revoked_set(),
    );
    let _ = issuer.validate(data);
});
