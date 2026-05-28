//! `concerto-agent-host` library surface.
//!
//! The binary in `src/main.rs` is intentionally thin — argv parsing and a
//! Tokio runtime bootstrap. The reusable pieces (CBOR frame codec, ring
//! buffer, final-info writer, public types) live here so the integration
//! test in `tests/` can link against them directly without re-driving
//! the binary for unit-style assertions.
//!
//! **Unix-only.** V0.1 ships the host on macOS (and Linux for CI); a
//! Windows ConPTY backend is V1.0 (see Task 21 Handoff Notes). The
//! `#[cfg(unix)]` gate at the binary level keeps the Windows CI matrix
//! green by failing closed with an informative error.

pub mod api;
pub mod bridge;
pub mod exit;
pub mod ring;
