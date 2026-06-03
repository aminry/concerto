//! `concerto-transport` — the production Iroh QUIC transport for the Core
//! (Task 212, `design/11`).
//!
//! Fills the empty crate with: one long-lived Iroh endpoint (hole-punch + relay
//! fallback), the **hand-rolled tonic-0.12 ↔ Iroh-bidi-stream duplex adapter**
//! spike 102 proved (NOT `tonic-iroh-transport`, which forces tonic 0.14), the
//! **three logical channels** (API / push-hint / pairing) multiplexed over the
//! one endpoint by a channel-tag byte, and the in-memory
//! `TransportState`/`ActiveSession`/`ConnectionPath` model (`design/11 §4`).
//!
//! The Noise IK session layer (Task 208) runs **inside** each API stream
//! (responder on accept, initiator on connect) — the second AEAD atop Iroh's
//! TLS (`design/12 §3.4`). The same Tonic server that serves UDS today accepts
//! Iroh callers via the Core's `serve_iroh`, which drives this crate's
//! [`ApiDispatcher`] seam and tags `ConnTransport(TransportKind::Iroh)` (Task
//! 201) so the 210 auth path and 201 caps see `IROH` with no per-transport
//! handler branching.
//!
//! # Crate layout
//!
//! - [`api`] — the **FROZEN public surface** Task 217 wraps + 213/214/215/216/218
//!   build against. Declared here (keychain/identity convention) so
//!   `scripts/regen-interfaces.sh` indexes it; impls live in the topic modules.
//! - [`adapter`] — `IrohDuplex` / `NoiseDuplex` / `IrohConnector` (the four
//!   spike gotchas + the Noise wrap).
//! - [`channels`] — the channel-tag framing + the 64 MiB message ceiling.
//! - [`endpoint`] — endpoint lifecycle, the serve loop, relay query/switch,
//!   `listen_pairing`, `send_wakeup_hint`, `close_sessions_for_device`.
//! - [`state`] — `TransportState` / `ActiveSession` / `ConnectionPath` / NAT.
//! - [`error`] — the typed `TransportError`.
//!
//! # Cross-platform
//!
//! Iroh + the adapter are QUIC and portable; nothing here is `#[cfg(unix)]`. The
//! Windows CI lane (Task 113) builds this crate as-is.

pub mod adapter;
pub mod api;
pub mod channels;
pub mod endpoint;
pub mod error;
pub mod state;

// The frozen surface, flattened to the crate root for ergonomic `use
// concerto_transport::{..}` (217's façade + the clients import from here). The
// canonical declarations live in `api`; this re-exports them at the root.
pub use api::{
    classify_path, connect_channel, direct_endpoint_addr, ActiveSession, ApiDispatcher, ChannelTag,
    ConnectionPath, DeviceId, IrohConnector, IrohDuplex, IrohTransport, NatStats, NoiseDuplex,
    PairingListener, RelayInfo, TransportConfig, TransportState, WakeupHint, ALPN,
    MAX_MESSAGE_SIZE,
};
pub use error::{Result, TransportError};
