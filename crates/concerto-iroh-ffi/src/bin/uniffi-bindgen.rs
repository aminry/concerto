//! Task 509.5 — the `uniffi-bindgen` CLI for `concerto-iroh-ffi`.
//!
//! `concerto-iroh-ffi` uses uniffi's proc-macro mode (`setup_scaffolding!()`
//! plus `#[uniffi::export]`, no `.udl`), so the bindings are generated in
//! library mode: the generator reads the uniffi metadata embedded in the
//! already-built cdylib and emits the Swift / Kotlin sources. The uniffi book's
//! "Setup for crates using only proc-macros" recipe is exactly this: a tiny bin
//! that forwards to `uniffi::uniffi_bindgen_main()`.
//!
//! It is built ONLY under the off-by-default `cli` feature (see Cargo.toml's
//! `[[bin]]` `required-features = ["cli"]`), so the shipped cdylib / staticlib
//! and the mobile cross-compile never pull `clap` / `camino` / `uniffi_bindgen`.
//!
//! Invoked by `scripts/native/gen-bindings.sh`:
//!
//! ```text
//! cargo run -p concerto-iroh-ffi --features cli --bin uniffi-bindgen -- \
//!     generate --library <path/to/libconcerto_iroh_ffi.dylib> \
//!     --language swift  --out-dir <out>/swift
//! cargo run -p concerto-iroh-ffi --features cli --bin uniffi-bindgen -- \
//!     generate --library <path/to/libconcerto_iroh_ffi.dylib> \
//!     --language kotlin --out-dir <out>/kotlin
//! ```

fn main() {
    uniffi::uniffi_bindgen_main();
}
