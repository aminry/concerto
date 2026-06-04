//! `concerto-relay` — the self-hosted relay library (`design/11 §3.2`, §6.2,
//! Task 214).
//!
//! Embeds Iroh's `iroh-relay 0.98.0` **as a library** (the `server` feature) —
//! it does **not** define a new relay protocol (`design/11 §12 R-7`). The
//! `concerto_relay` lib owns: the relay core ([`Relay`] — build + run the
//! embedded iroh-relay server from a config struct), the in-memory routing-table
//! lifecycle ([`RelayState`] / [`EndpointRoute`], 90 s TTL refreshed by
//! keep-alive, `MAX_ROUTES` cap), the per-endpoint bandwidth caps, and the
//! Prometheus metrics endpoint. The `concerto-relay` **binary** (the sibling
//! `main.rs`) is a thin wrapper: parse env config, start the relay + Prometheus
//! endpoint, handle signals/shutdown.
//!
//! The hole-punch address-exchange assist and the relayed-QUIC forwarding are
//! provided by `iroh-relay` itself — this crate wires its config and adds the
//! observability + caps the operators see; it does not reimplement the relay
//! protocol.
//!
//! # Ciphertext-only posture (`design/11 §3.9`)
//!
//! The relay forwards encrypted QUIC; the Noise IK layer (`design/12 §3.4`) is
//! inside the tunnel and the relay cannot decrypt it. Metrics and logs carry
//! only **metadata** — source IP, endpoint id, byte counts, timestamps, region —
//! never payload, certs, names, or tokens. The routing-table + bandwidth layers
//! observe byte *counts*, never byte *content*.
//!
//! # FROZEN surface
//!
//! The public lib entry point ([`Relay::start`] / [`Relay::shutdown`]), the
//! [`RelayConfig`] env-var surface, the Prometheus metric **names** (see
//! [`metrics`]), and the [`RelayState`] / [`EndpointRoute`] field layout are
//! **FROZEN** (`tasks/v1.0/214-relay-binary.md`) — declared in [`api`] and
//! [`metrics`] so Task 215 wraps this binary to add the WSS bridge on the
//! reserved `WSS_LISTEN_ADDR` without a re-lock.

pub mod api;
pub mod config;
pub mod error;
pub mod metrics;
pub mod relay;
pub mod state;

// The FROZEN public surface (re-exported from `api` per the regen-interfaces
// convention — `api.rs` is the canonical, generator-indexed declaration site).
pub use crate::api::{
    BandwidthCounter, EndpointRoute, Relay, RelayConfig, RelayState, WssBridge, DEFAULT_MAX_ROUTES,
    DEFAULT_PROMETHEUS_LISTEN_ADDR, DEFAULT_RELAY_LISTEN_ADDR, ROUTE_TTL,
};
pub use crate::error::{RelayError, Result};
