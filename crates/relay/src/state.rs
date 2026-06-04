//! The in-memory routing-table lifecycle (`design/11 §3.2`, §4, Task 214).
//!
//! The FROZEN type declarations — [`RelayState`](crate::api::RelayState),
//! [`EndpointRoute`](crate::api::EndpointRoute),
//! [`BandwidthCounter`](crate::api::BandwidthCounter),
//! [`WssBridge`](crate::api::WssBridge) — live in [`crate::api`]; this module
//! holds their lifecycle methods.
//!
//! The relay embeds Iroh's `iroh-relay` server for the actual hole-punch assist
//! and relayed-QUIC forwarding (no new protocol — `design/11 §12 R-7`). This
//! parallel table is the relay's **observability + policy** layer: it tracks
//! `endpoint_id → current public IP+port` with a 90 s TTL refreshed by
//! keep-alive (`design/11 §3.2`), enforces the `MAX_ROUTES` cap and the
//! per-endpoint bandwidth cap, and feeds the Prometheus metrics. It is driven
//! through [`Relay`](crate::api::Relay)'s register/keepalive/forward API so the
//! routing counts + byte totals the relay reports are exact, while the byte
//! *content* never touches this layer (`design/11 §3.9` ciphertext-only).

use std::net::SocketAddr;
use std::time::Instant;

use crate::api::{BandwidthCounter, EndpointRoute, RelayState, ROUTE_TTL};

/// The outcome of a registration attempt against the `MAX_ROUTES` cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterOutcome {
    /// A new route was inserted.
    Inserted,
    /// An existing route was refreshed (keep-alive).
    Refreshed,
    /// Refused — the table is at `MAX_ROUTES` and this is a new endpoint
    /// (`design/11 §6.3`).
    Rejected,
}

/// The outcome of a forward-accounting attempt against the per-endpoint
/// bandwidth cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardOutcome {
    /// The bytes were accounted; forwarding may proceed.
    Forwarded,
    /// Refused — the endpoint hit `BANDWIDTH_CAP_PER_ENDPOINT`
    /// (`design/11 §6.3`, §3.9).
    Capped,
}

impl EndpointRoute {
    /// A fresh route for `public_addr`, registered now with a 90 s TTL.
    pub fn new(public_addr: SocketAddr, now: Instant) -> Self {
        Self {
            public_addr,
            last_seen: now,
            expires_at: now + ROUTE_TTL,
        }
    }

    /// Whether this route has expired at `now` (lazy TTL eviction on access,
    /// `design/11 §3.2`).
    pub fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires_at
    }
}

impl RelayState {
    /// A fresh, empty routing table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new endpoint route, or refresh an existing one's TTL on
    /// keep-alive (`design/11 §3.2`). Enforces the `MAX_ROUTES` cap: a *new*
    /// endpoint is rejected when the table is full, but an existing endpoint's
    /// keep-alive (or a public-addr update) always succeeds (it does not grow
    /// the table). Returns the [`RegisterOutcome`] so the caller can bump the
    /// reject metric.
    pub fn register(
        &mut self,
        endpoint_id: &str,
        public_addr: SocketAddr,
        max_routes: usize,
        now: Instant,
    ) -> RegisterOutcome {
        if let Some(route) = self.routes.get_mut(endpoint_id) {
            route.public_addr = public_addr;
            route.last_seen = now;
            route.expires_at = now + ROUTE_TTL;
            return RegisterOutcome::Refreshed;
        }
        // New endpoint — evict any expired entries first so the cap reflects
        // live routes, then enforce the cap.
        self.evict_expired(now);
        if self.routes.len() >= max_routes {
            return RegisterOutcome::Rejected;
        }
        self.routes.insert(
            endpoint_id.to_string(),
            EndpointRoute::new(public_addr, now),
        );
        self.bandwidth_counters
            .entry(endpoint_id.to_string())
            .or_default();
        RegisterOutcome::Inserted
    }

    /// Account `bytes` of forwarded ciphertext for an endpoint against its
    /// per-endpoint cap (`design/11 §6.3`, §3.9). Returns [`ForwardOutcome`]:
    /// `Capped` (and does **not** add the bytes) once the endpoint would exceed
    /// `cap`; `Forwarded` otherwise. `cap == None` → unlimited. Byte *content*
    /// never enters this layer — only the count (`design/11 §3.9`).
    pub fn account_forward(
        &mut self,
        endpoint_id: &str,
        bytes: u64,
        cap: Option<u64>,
    ) -> ForwardOutcome {
        let counter = self
            .bandwidth_counters
            .entry(endpoint_id.to_string())
            .or_default();
        if let Some(cap) = cap {
            if counter.bytes_forwarded.saturating_add(bytes) > cap {
                return ForwardOutcome::Capped;
            }
        }
        counter.bytes_forwarded = counter.bytes_forwarded.saturating_add(bytes);
        ForwardOutcome::Forwarded
    }

    /// Look up an endpoint's current route (if live).
    pub fn route(&self, endpoint_id: &str) -> Option<&EndpointRoute> {
        self.routes.get(endpoint_id)
    }

    /// The per-endpoint forwarded-byte counter (if any).
    pub fn bandwidth(&self, endpoint_id: &str) -> Option<&BandwidthCounter> {
        self.bandwidth_counters.get(endpoint_id)
    }

    /// The current number of live routes.
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Total ciphertext bytes forwarded across all endpoints (the
    /// `concerto_relay_bytes_forwarded_total` source of truth).
    pub fn total_bytes_forwarded(&self) -> u64 {
        self.bandwidth_counters
            .values()
            .map(|c| c.bytes_forwarded)
            .fold(0u64, |acc, n| acc.saturating_add(n))
    }

    /// Drop every expired route (`now >= expires_at`) and its bandwidth counter
    /// (`design/11 §3.2` TTL eviction). Returns the number evicted. Called by
    /// the periodic sweep and lazily before a new insertion.
    pub fn evict_expired(&mut self, now: Instant) -> usize {
        let expired: Vec<String> = self
            .routes
            .iter()
            .filter(|(_, r)| r.is_expired(now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &expired {
            self.routes.remove(id);
            self.bandwidth_counters.remove(id);
        }
        expired.len()
    }
}
