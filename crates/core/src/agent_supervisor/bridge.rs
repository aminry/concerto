//! Core-side wrapper over the host's locked CBOR frame codec (Task 22).
//!
//! The Agent Host crate (`concerto_agent_host`) already exposes the
//! frame types and codec functions used on the wire (`HostFrame`,
//! `read_frame`, `write_frame`); reimplementing them on the Core side
//! would risk silently diverging from the locked protocol. Instead, this
//! module re-exports those primitives and provides a small wrapper that
//! frames the Core's `Hello` payload.
//!
//! The codec accepts any `AsyncRead`/`AsyncWrite` half, so the same
//! function works for the in-process `tokio::io::duplex` test paths and
//! the production `tokio::net::UnixStream`.

pub use concerto_agent_host::api::{AgentKind as HostAgentKind, HostFrame};
pub use concerto_agent_host::bridge::{read_frame, write_frame, FrameError};

/// Build a `HostFrame::Hello` frame on the Core side. Wraps the
/// argument-only constructor so the call site reads the same on Core
/// and host. Pass `last_seq = 0` for first connect; for hot reconnect
/// (Task 36) pass the persisted `sessions.last_acked_seq` watermark so
/// the host replays only the unacked tail of its ring buffer.
pub fn build_hello(core_version: &str, cookie: [u8; 32], last_seq: u64) -> HostFrame {
    HostFrame::Hello {
        core_version: core_version.to_string(),
        expected_cookie: cookie,
        last_seq,
    }
}
