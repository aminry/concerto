//! Shared harness logic for the Iroh NAT-diversity spike (Task 101).
//!
//! Two binaries (`core`, `client`) link this. The `core` binary stands up an
//! Iroh endpoint and prints its `EndpointId` (the ticket the `client` dials);
//! the `client` binary dials that id, performs a tiny request/response, and
//! reports whether Iroh settled on a **direct** (hole-punched) path or a
//! **relayed** path — reading Iroh's OWN per-path signal rather than inferring
//! from latency. Both ends log the candidate paths Iroh exposes and the
//! round-trip connect time.
//!
//! Pinned to iroh 0.98.2 (see `Cargo.toml` and the findings doc). In 0.98.x
//! Iroh is multipath: a [`Connection`] can hold several paths at once
//! (`Connection::paths()`), each either an IP/UDP path (direct) or a relay
//! path. We classify a connection by its **selected** path — the one Iroh is
//! actually transmitting on — which is the realistic direct-vs-relay verdict.
//!
//! This is a throwaway measurement harness, not product code (see the task
//! file and `design/spikes/iroh-nat-findings.md`).

use std::time::Duration;

use anyhow::{Context, Result};
use iroh::endpoint::{presets, Connection, RelayMode};
use iroh::{Endpoint, RelayMap, RelayUrl, Watcher};

/// ALPN for the spike's tiny echo protocol. Throwaway value; not a product
/// identifier.
pub const ALPN: &[u8] = b"concerto/iroh-nat-spike/0";

/// The single application token the client sends and the core echoes. Keeping
/// the payload trivial means the measurement reflects path establishment, not
/// transfer time.
pub const PING: &[u8] = b"PING";
pub const PONG: &[u8] = b"PONG";

/// Direct-vs-relay verdict for a connection, derived from Iroh's selected
/// path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// Selected path is an IP/UDP path — hole-punch (or LAN) succeeded.
    Direct,
    /// Selected path is a relay path — fell back to relayed QUIC.
    Relayed,
    /// No path is selected yet / connection has no usable path.
    None,
}

impl PathKind {
    pub fn label(self) -> &'static str {
        match self {
            PathKind::Direct => "DIRECT",
            PathKind::Relayed => "RELAYED",
            PathKind::None => "NONE",
        }
    }

    /// Whether this counts as a successful hole-punch for the >70%-direct
    /// V1.0 bar.
    pub fn is_direct(self) -> bool {
        matches!(self, PathKind::Direct)
    }
}

/// Which relay configuration the harness should use.
///
/// `Default` uses n0's public relays (the `N0` preset) — the zero-config path
/// an operator gets out of the box, and what makes "relayed" *achievable* (so
/// a relay row is a real measurement, not just "failed"). `Custom(url)` points
/// at an operator's own throwaway `iroh-relay` (see the crate README) so the
/// relay path is one they control. `Disabled` forces direct-only (useful to
/// confirm a pair can hole-punch with no relay assist at all).
#[derive(Debug, Clone)]
pub enum RelayChoice {
    Default,
    Custom(String),
    Disabled,
}

impl RelayChoice {
    /// Parse the `--relay` CLI value. `default` / empty → n0 default,
    /// `disabled` / `off` / `none` → no relay, anything else → a custom URL.
    pub fn parse(raw: Option<&str>) -> Result<Self> {
        Ok(match raw.map(str::trim) {
            None | Some("") | Some("default") => RelayChoice::Default,
            Some("disabled") | Some("off") | Some("none") => RelayChoice::Disabled,
            Some(url) => RelayChoice::Custom(url.to_string()),
        })
    }

    fn relay_mode(&self) -> Result<Option<RelayMode>> {
        Ok(match self {
            // N0 preset already wires the default relays; no override needed.
            RelayChoice::Default => None,
            RelayChoice::Disabled => Some(RelayMode::Disabled),
            RelayChoice::Custom(url) => {
                // Validate the URL eagerly for a clear error, then hand the
                // raw string to RelayMap::try_from_iter (the 0.98 constructor).
                let _: RelayUrl = url
                    .parse()
                    .with_context(|| format!("invalid relay URL: {url}"))?;
                let map = RelayMap::try_from_iter([url.as_str()])
                    .with_context(|| format!("building relay map from {url}"))?;
                Some(RelayMode::Custom(map))
            }
        })
    }
}

/// Build an Iroh endpoint for this harness.
///
/// Starts from the `N0` preset — n0's default discovery + relays — so a bare
/// `EndpointId` (no pre-shared address) is dialable end to end, which is the
/// realistic remote case the spike measures. The relay choice can override the
/// preset's relay mode (custom relay, or disabled for direct-only).
pub async fn build_endpoint(relay: &RelayChoice) -> Result<Endpoint> {
    let mut builder = Endpoint::builder(presets::N0).alpns(vec![ALPN.to_vec()]);
    if let Some(mode) = relay.relay_mode()? {
        builder = builder.relay_mode(mode);
    }
    let endpoint = builder.bind().await.context("binding Iroh endpoint")?;
    Ok(endpoint)
}

/// Inspect a connection's current paths and return the direct-vs-relay verdict
/// from the **selected** path (the one Iroh is transmitting on). Falls back to
/// "any direct path present" if nothing is flagged selected yet.
pub fn classify_connection(conn: &Connection) -> PathKind {
    let mut paths = conn.paths();
    let list = paths.get();

    let mut saw_direct = false;
    let mut saw_relay = false;
    for path in list.iter() {
        if path.is_selected() && !path.is_closed() {
            return if path.is_ip() {
                PathKind::Direct
            } else {
                PathKind::Relayed
            };
        }
        if path.is_closed() {
            continue;
        }
        if path.is_ip() {
            saw_direct = true;
        }
        if path.is_relay() {
            saw_relay = true;
        }
    }

    if saw_direct {
        PathKind::Direct
    } else if saw_relay {
        PathKind::Relayed
    } else {
        PathKind::None
    }
}

/// Watch a connection's paths for up to `settle`, returning the best verdict
/// observed. Iroh upgrades a freshly-dialed connection from a relay path to a
/// direct path asynchronously once hole-punching completes, so we give it a
/// window to settle on the best path before recording the verdict.
pub async fn observe_settled_path(conn: &Connection, settle: Duration) -> PathKind {
    let mut current = classify_connection(conn);
    if current.is_direct() {
        log_paths(conn);
        return current;
    }

    let mut watcher = conn.paths();
    let deadline = tokio::time::Instant::now() + settle;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, watcher.updated()).await {
            Ok(Ok(_)) => {
                current = classify_connection(conn);
                if current.is_direct() {
                    break;
                }
            }
            // Watcher closed or window elapsed — keep the last value.
            Ok(Err(_)) | Err(_) => break,
        }
    }

    log_paths(conn);
    current
}

/// Log every path Iroh currently holds for this connection (direct candidates
/// + relay), so a relayed result can be diagnosed.
pub fn log_paths(conn: &Connection) {
    let mut paths = conn.paths();
    let list = paths.get();
    for path in list.iter() {
        tracing::info!(
            id = ?path.id(),
            remote_addr = ?path.remote_addr(),
            selected = path.is_selected(),
            closed = path.is_closed(),
            is_ip = path.is_ip(),
            is_relay = path.is_relay(),
            rtt = ?path.rtt(),
            "iroh connection path",
        );
    }
}

/// Install a tracing subscriber for the harness binaries.
pub fn init_tracing() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,iroh=info"));
    tracing_subscriber::registry()
        .with(fmt::layer().with_target(true))
        .with(filter)
        .init();
}
