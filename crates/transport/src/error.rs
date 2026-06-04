//! Transport error type (`concerto-transport`).
//!
//! A small, owned error enum (not `anyhow`) per the Task-212 hardening note —
//! the spike used `anyhow`; production wants typed errors so callers (the Core
//! api_server, Task 217's `TransportHandle`) can branch. Every variant carries
//! a human string; the underlying `io::Error` / `IdentityError` are folded into
//! the message so the type stays `Clone`-free-but-`Send + Sync` and proto-free.

use std::fmt;

/// The transport's result alias.
pub type Result<T> = std::result::Result<T, TransportError>;

/// Errors raised by the Iroh transport, its adapter, and the channel/Noise
/// layering.
#[derive(Debug)]
pub enum TransportError {
    /// Building or binding the Iroh endpoint failed (bind, key load, relay
    /// config).
    Endpoint(String),
    /// Dialing / accepting an Iroh connection or bidi stream failed.
    Connection(String),
    /// The hand-rolled tonic adapter (duplex / connector / serve loop) failed.
    Adapter(String),
    /// The channel-tag handshake at the head of a stream was malformed or named
    /// an unknown channel.
    Channel(String),
    /// The Noise IK handshake or a session encrypt/decrypt failed — the caller
    /// drops the connection (`design/12 §6.3`).
    Noise(String),
    /// A remote (non-LAN) operation was refused because `disable_remote = true`
    /// (Task 211, `design/11 §6.4`).
    RemoteDisabled(String),
    /// The mDNS responder/browser (`design/11 §3.5`, Task 213) failed — daemon
    /// creation, service registration, or browse setup.
    Mdns(String),
    /// I/O on the underlying byte channel failed.
    Io(String),
    /// A [`TransportHandle`](crate::api::TransportHandle) lifecycle method was
    /// called against the wrong state — `start` on an already-running handle, or
    /// a delegating method (`current_relay`, `switch_relay`, `nat_stats`,
    /// `listen_pairing`, `send_wakeup_hint`, …) before `start` brought the
    /// endpoint up / after `stop` tore it down (Task 217, `design/11 §5.1`).
    Lifecycle(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::Endpoint(m) => write!(f, "iroh endpoint: {m}"),
            TransportError::Connection(m) => write!(f, "iroh connection: {m}"),
            TransportError::Adapter(m) => write!(f, "tonic-iroh adapter: {m}"),
            TransportError::Channel(m) => write!(f, "channel tag: {m}"),
            TransportError::Noise(m) => write!(f, "noise ik: {m}"),
            TransportError::RemoteDisabled(m) => write!(f, "remote disabled: {m}"),
            TransportError::Mdns(m) => write!(f, "mdns: {m}"),
            TransportError::Io(m) => write!(f, "transport io: {m}"),
            TransportError::Lifecycle(m) => write!(f, "transport handle lifecycle: {m}"),
        }
    }
}

impl std::error::Error for TransportError {}

impl From<std::io::Error> for TransportError {
    fn from(e: std::io::Error) -> Self {
        TransportError::Io(e.to_string())
    }
}

impl From<concerto_identity::IdentityError> for TransportError {
    fn from(e: concerto_identity::IdentityError) -> Self {
        TransportError::Noise(e.to_string())
    }
}

impl From<TransportError> for std::io::Error {
    fn from(e: TransportError) -> Self {
        std::io::Error::other(e.to_string())
    }
}
