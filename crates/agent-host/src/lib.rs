//! `concerto-agent-host` library surface.
//!
//! The binary in `src/main.rs` is intentionally thin — argv parsing and a
//! Tokio runtime bootstrap. The reusable pieces (CBOR frame codec, ring
//! buffer, final-info writer, public types) live here so the integration
//! test in `tests/` can link against them directly without re-driving
//! the binary for unit-style assertions.
//!
//! **Portable surface, Unix-only runtime.** The types and helpers in this
//! library (`api`, `bridge`, `ring`, `exit`) are platform-independent and
//! compile + test on macOS, Linux, and Windows. The PTY/UDS supervisor
//! that drives them lives in `src/main.rs`'s `#[cfg(unix)] mod unix`; the
//! Windows ConPTY backend is pending (Task 702), so the binary is a stub
//! there. Keeping this library portable lets the Windows CI lane build and
//! lint the whole crate (the integration test cfg-gates to empty on
//! Windows) instead of excluding agent-host wholesale.

pub mod api;
pub mod bridge;
pub mod exit;
pub mod ring;
