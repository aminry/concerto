//! Relay error type (`concerto-relay`, Task 214).
//!
//! A small owned error enum (the transport-crate convention, not `anyhow`): the
//! binary wrapper and Task 215 can branch on the variant. Each carries a human
//! string; underlying errors are folded into the message so the type stays
//! `Send + Sync` and dependency-light.

use std::fmt;

/// The relay's result alias.
pub type Result<T> = std::result::Result<T, RelayError>;

/// Errors raised parsing config, spawning the embedded `iroh-relay` server, or
/// serving the Prometheus endpoint.
#[derive(Debug)]
pub enum RelayError {
    /// An env-var config value was malformed (`design/11 §6.3`). The message
    /// names the offending variable so a misconfigured deploy fails fast.
    Config(String),
    /// Spawning or running the embedded `iroh-relay` server failed
    /// (`design/11 §3.2`, R-7).
    Server(String),
    /// Binding or serving the Prometheus `/metrics` endpoint failed
    /// (`design/11 §6.3`).
    Metrics(String),
    /// A registration was refused because the routing table is at `MAX_ROUTES`
    /// (`design/11 §6.3`).
    RoutesFull(String),
    /// A forward was refused because the endpoint hit
    /// `BANDWIDTH_CAP_PER_ENDPOINT` (`design/11 §6.3`, §3.9).
    BandwidthCapped(String),
}

impl fmt::Display for RelayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RelayError::Config(m) => write!(f, "relay config: {m}"),
            RelayError::Server(m) => write!(f, "relay server: {m}"),
            RelayError::Metrics(m) => write!(f, "relay metrics: {m}"),
            RelayError::RoutesFull(m) => write!(f, "relay routes full: {m}"),
            RelayError::BandwidthCapped(m) => write!(f, "relay bandwidth capped: {m}"),
        }
    }
}

impl std::error::Error for RelayError {}

impl From<std::io::Error> for RelayError {
    fn from(e: std::io::Error) -> Self {
        RelayError::Server(e.to_string())
    }
}
