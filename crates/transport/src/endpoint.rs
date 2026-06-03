//! The long-lived Iroh endpoint, its lifecycle, and the `serve` loop
//! (`design/11 §3.1`, §3.3, §6.1, Task 212).
//!
//! The FROZEN type declarations — [`IrohTransport`](crate::api::IrohTransport),
//! [`TransportConfig`](crate::api::TransportConfig),
//! [`RelayInfo`](crate::api::RelayInfo), [`WakeupHint`](crate::api::WakeupHint),
//! [`PairingListener`](crate::api::PairingListener),
//! [`ApiDispatcher`](crate::api::ApiDispatcher) — live in [`crate::api`]; this
//! module holds their method impls, the serve loop, and the free helpers.
//!
//! One Core has **one** long-lived [`iroh::Endpoint`]. Iroh generates and
//! persists its own endpoint key in its state dir — **separate** from the Core's
//! Ed25519 identity (`design/12 §3.1`). The endpoint registers with the
//! configured relay (hole-punch + relay fallback), honors a directly-supplied
//! Core address / relay (spike Note B), and — under `disable_remote` (Task 211)
//! — refuses relay registration and accepts only LAN connections (mDNS
//! unaffected).
//!
//! The serve loop accepts inbound connections and, for each bidi stream, reads
//! the channel-tag byte (`design/11 §3.3`), then routes: **API** → Noise IK
//! responder handshake → the caller-supplied [`ApiDispatcher`] (the Core's
//! shared Tonic service set); **Pairing** → the `listen_pairing` listener (Task
//! 207); **PushHint** → the dispatcher (wakeup-fetch rides the gRPC surface).
//!
//! The transport stays **proto-free** (no `concerto-core` / `concerto-proto`
//! dep): the Core supplies its service set through [`ApiDispatcher`], keeping
//! Task 217's façade thin and letting 213/214/215 reuse this loop.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::endpoint::{presets, Connection, RelayMode};
use iroh::{Endpoint, EndpointAddr, RelayMap, RelayUrl, Watcher};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::adapter;
use crate::api::{
    ApiDispatcher, ChannelTag, DeviceId, IrohConnector, IrohDuplex, IrohTransport, MdnsConfig,
    PairingListener, RelayInfo, TransportConfig, TransportState, WakeupHint,
};
use crate::error::{Result, TransportError};

/// The ALPN the Concerto Iroh transport speaks. **FROZEN** — every Concerto
/// endpoint (Core, Desktop, Mobile, relay-bridged) dials this exact protocol id.
pub const ALPN: &[u8] = b"concerto/transport/1";

/// Initial relay backoff for the relay-unreachable retry loop (`design/11 §8`).
const RELAY_BACKOFF_INITIAL: Duration = Duration::from_secs(1);
/// Cap on the exponential relay backoff.
const RELAY_BACKOFF_MAX: Duration = Duration::from_secs(60);

impl PairingListener {
    /// Construct a listener bound to `token_hash` over `rx` (only the owning
    /// transport calls this; the public surface is `accept` / `token_hash`).
    pub(crate) fn build(token_hash: [u8; 32], rx: mpsc::Receiver<IrohDuplex>) -> Self {
        Self { token_hash, rx }
    }
}

impl IrohTransport {
    /// Build the transport: derive the Core Noise static from its persisted
    /// private bytes, build the one long-lived Iroh endpoint (relay vs LAN-only
    /// per `disable_remote`), validate a directly-supplied address. Does **not**
    /// start the serve loop — call [`Self::serve`] with the Core's dispatcher.
    ///
    /// `core_noise_static_private` is the Core's persisted 32-byte X25519 Noise
    /// static private key — the transport's persistence path (the Core stores it
    /// the same way it stores its Ed25519 identity; see the api_server wiring).
    /// Passing the raw private bytes keeps the FROZEN `start` signature stable
    /// and lets the Core own the at-rest form.
    pub async fn start(
        config: TransportConfig,
        core_noise_static_private: [u8; 32],
    ) -> Result<Self> {
        let core_static = Arc::new(concerto_identity::NoiseStatic::from_private(
            core_noise_static_private,
        )?);

        let endpoint = build_endpoint(&config).await?;

        let relay = RelayInfo {
            url: if config.disable_remote {
                None
            } else {
                config.relay_url.clone()
            },
            remote_disabled: config.disable_remote,
        };

        let (wakeup_tx, wakeup_rx) = mpsc::unbounded_channel();

        let transport = Self {
            endpoint,
            state: Arc::new(Mutex::new(TransportState::new())),
            relay: Arc::new(Mutex::new(relay)),
            config,
            core_static,
            pairing_tx: Arc::new(Mutex::new(None)),
            wakeup_tx,
            wakeup_rx: Arc::new(Mutex::new(Some(wakeup_rx))),
            mdns: Arc::new(Mutex::new(None)),
            shutdown: CancellationToken::new(),
        };

        // Relay registration honours `disable_remote`: only spawn the
        // registration/backoff loop when remote is enabled.
        if !transport.config.disable_remote {
            transport.spawn_relay_registration();
        } else {
            tracing::info!(
                "transport: disable_remote=true — not registering with any relay; LAN connections only (mDNS unaffected)"
            );
        }

        Ok(transport)
    }

    /// The Core's X25519 Noise static **public** key — the value a pairing flow
    /// embeds in the QR (alongside `core_pubkey` + `iroh_endpoint_id`) so the
    /// device pre-loads it as the responder static for the IK handshake. The
    /// "carry the responder's public half" half of the 208 open question.
    pub fn core_noise_public(&self) -> [u8; 32] {
        self.core_static.public()
    }

    /// The Iroh endpoint id clients dial (the QR's `iroh_endpoint_id`).
    pub fn endpoint_id(&self) -> iroh::EndpointId {
        self.endpoint.id()
    }

    /// A clone of the underlying Iroh endpoint (Task 213 mDNS publishes its
    /// addrs; Task 216 reads migration signals).
    pub fn endpoint(&self) -> Endpoint {
        self.endpoint.clone()
    }

    /// Publish (or re-publish) this Core on the LAN via mDNS
    /// (`_concerto._tcp.local`, `design/11 §3.5`, Task 213). Call **after** the
    /// endpoint is up — the TXT record's `endpoint_id` is read from
    /// [`Self::endpoint_id`] when the caller is `None`, otherwise from the
    /// supplied [`MdnsConfig`].
    ///
    /// `config.opt_out` (the dedicated managed / per-network mDNS opt-out)
    /// suppresses publication; **`disable_remote` does NOT** — LAN-only mode
    /// still publishes mDNS (`design/11 §6.4`). Re-announces by replacing the
    /// prior responder (its drop sends the goodbye for the old record), so
    /// callers re-invoke this on `version` / `endpoint_id` / `caps` change.
    pub fn publish_mdns(&self, config: MdnsConfig) -> Result<()> {
        let responder = crate::api::MdnsResponder::publish(config)?;
        let mut slot = self.mdns.lock().expect("mdns lock");
        // Drop any prior responder first (its Drop sends the goodbye packet for
        // the stale record before the new one is registered).
        *slot = None;
        *slot = Some(responder);
        Ok(())
    }

    /// Deregister the mDNS service (mDNS goodbye) and stop advertising. After
    /// this the Core is no longer LAN-discoverable until [`Self::publish_mdns`]
    /// is called again. Idempotent.
    pub fn stop_mdns(&self) {
        if let Some(mut responder) = self.mdns.lock().expect("mdns lock").take() {
            responder.shutdown();
        }
    }

    /// Whether the Core is currently advertising over mDNS (false before
    /// [`Self::publish_mdns`], after [`Self::stop_mdns`], or when the opt-out
    /// suppressed publication).
    pub fn is_mdns_publishing(&self) -> bool {
        self.mdns
            .lock()
            .expect("mdns lock")
            .as_ref()
            .is_some_and(|r| r.is_publishing())
    }

    /// Start an mDNS **browser** for `_concerto._tcp.local` (`design/11 §3.5`,
    /// Task 213). The returned [`MdnsBrowser`] yields discovered Cores; the
    /// caller feeds each [`DiscoveredCore::endpoint_id`] to
    /// [`connect_channel`] to open Iroh directly on the LAN
    /// ([`ConnectionPath::Lan`]). Independent of the responder — a client that
    /// is not itself a Core browses without publishing.
    ///
    /// This is a free helper on the transport for ergonomics; it does not need
    /// `self`'s endpoint (browsing is pure mDNS). 218/219/511 may also call
    /// [`MdnsBrowser::start`] directly.
    pub fn browse_lan(&self) -> Result<crate::api::MdnsBrowser> {
        crate::api::MdnsBrowser::start(None)
    }

    /// The current relay association (`design/11 §5.1`, Task 217 `current_relay`).
    pub fn current_relay(&self) -> RelayInfo {
        self.relay.lock().expect("relay lock").clone()
    }

    /// Switch the configured relay URL (`design/11 §5.1`, Task 217
    /// `switch_relay`). Refused with [`TransportError::RemoteDisabled`] under
    /// `disable_remote` (`design/11 §6.4`).
    pub fn switch_relay(&self, url: String) -> Result<()> {
        if self.config.disable_remote {
            return Err(TransportError::RemoteDisabled(
                "switch_relay refused: disable_remote=true (LAN-only)".into(),
            ));
        }
        let mut relay = self.relay.lock().expect("relay lock");
        relay.url = Some(url);
        Ok(())
    }

    /// Open a pairing listener gated on `token_hash` (`design/11 §5.1`, Task 217
    /// `listen_pairing`; Task 207 drives the handshake inside). Replaces any
    /// prior listener. The serve loop routes [`ChannelTag::Pairing`] duplexes
    /// here. FROZEN signature.
    pub fn listen_pairing(&self, token_hash: [u8; 32]) -> PairingListener {
        let (tx, rx) = mpsc::channel(4);
        *self.pairing_tx.lock().expect("pairing lock") = Some((token_hash, tx));
        PairingListener::build(token_hash, rx)
    }

    /// Close any open pairing listener (`design/11 §5.1`, Task 217
    /// `close_pairing`). Idempotent.
    pub fn close_pairing(&self) {
        *self.pairing_tx.lock().expect("pairing lock") = None;
    }

    /// Enqueue a push-hint toward a device (`design/11 §5.1`, Task 217
    /// `send_wakeup_hint`; `design/14` delivers it). ID-only payload — no PII.
    pub fn send_wakeup_hint(&self, device_id: DeviceId, payload: Vec<u8>) -> Result<()> {
        self.wakeup_tx
            .send(WakeupHint { device_id, payload })
            .map_err(|_| TransportError::Channel("wakeup-hint channel closed".into()))
    }

    /// Take the push-hint receiver (Task 14's delivery loop drains it). `None`
    /// if already taken.
    pub fn take_wakeup_receiver(&self) -> Option<mpsc::UnboundedReceiver<WakeupHint>> {
        self.wakeup_rx.lock().expect("wakeup lock").take()
    }

    /// Close every open session for `device_id` (`design/12 §7.3`, the Task-209
    /// `SessionCloser` seam Task 217 satisfies). FROZEN signature.
    pub fn close_sessions_for_device(&self, device_id: &DeviceId) {
        self.state
            .lock()
            .expect("state lock")
            .close_sessions_for_device(device_id);
    }

    /// A snapshot of the live NAT counters (`design/11 §5.1`, Task 216/217
    /// `nat_stats`).
    pub fn nat_stats(&self) -> crate::api::NatStats {
        self.state.lock().expect("state lock").nat_stats.clone()
    }

    /// The connection paths of the currently-live sessions, keyed by device id
    /// (`design/11 §4` — 216 reads per-session `path`).
    pub fn session_paths(&self) -> Vec<(DeviceId, crate::api::ConnectionPath)> {
        self.state
            .lock()
            .expect("state lock")
            .sessions
            .values()
            .map(|s| (s.device_id.clone(), s.path))
            .collect()
    }

    /// Run the accept/serve loop until [`Self::stop`] (or the returned future is
    /// dropped). `dispatcher` is the Core's shared gRPC service set; the loop
    /// hands every API stream to it. Mirrors `run_uds`'s
    /// `serve_with_incoming_shutdown` shape — resolves on the shutdown token.
    pub async fn serve<D: ApiDispatcher>(&self, dispatcher: Arc<D>) -> Result<()> {
        let shutdown = self.shutdown.clone();
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                incoming = self.endpoint.accept() => {
                    let Some(incoming) = incoming else { break };
                    let dispatcher = dispatcher.clone();
                    let core_static = self.core_static.clone();
                    let state = self.state.clone();
                    let pairing_tx = self.pairing_tx.clone();
                    let sd = shutdown.clone();
                    tokio::spawn(async move {
                        match incoming.await {
                            Ok(conn) => {
                                if let Err(err) =
                                    serve_conn(conn, dispatcher, core_static, state, pairing_tx, sd)
                                        .await
                                {
                                    tracing::warn!(%err, "iroh connection server error");
                                }
                            }
                            Err(err) => tracing::warn!(?err, "iroh incoming failed"),
                        }
                    });
                }
            }
        }
        self.endpoint.close().await;
        Ok(())
    }

    /// Signal the serve loop to stop (`design/11 §5.1`, Task 217 `stop`). Also
    /// deregisters the mDNS service (goodbye packet) so a stopped Core stops
    /// being LAN-discoverable.
    pub fn stop(&self) {
        self.stop_mdns();
        self.shutdown.cancel();
    }

    /// Spawn the relay registration / backoff loop (`design/11 §8`): retry with
    /// exponential backoff while the LAN path stays usable. In iroh 0.98 the
    /// endpoint's relay actor handles the actual (re)connection; this loop owns
    /// the observability + backoff envelope and updates [`RelayInfo`]. No-op
    /// when no relay URL is configured (Iroh's default map is used).
    fn spawn_relay_registration(&self) {
        let Some(url) = self.relay.lock().expect("relay lock").url.clone() else {
            return;
        };
        let endpoint = self.endpoint.clone();
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let mut backoff = RELAY_BACKOFF_INITIAL;
            loop {
                if shutdown.is_cancelled() {
                    break;
                }
                let addr = endpoint.watch_addr().get();
                let has_relay = addr.relay_urls().next().is_some();
                if has_relay {
                    tracing::info!(relay = %url, "transport: registered with relay");
                    backoff = RELAY_BACKOFF_INITIAL;
                    tokio::select! {
                        _ = shutdown.cancelled() => break,
                        _ = tokio::time::sleep(RELAY_BACKOFF_MAX) => continue,
                    }
                } else {
                    tracing::debug!(
                        relay = %url,
                        backoff_ms = backoff.as_millis() as u64,
                        "transport: relay not yet reachable; retrying (LAN path usable)"
                    );
                    tokio::select! {
                        _ = shutdown.cancelled() => break,
                        _ = tokio::time::sleep(backoff) => {}
                    }
                    backoff = (backoff * 2).min(RELAY_BACKOFF_MAX);
                }
            }
        });
    }
}

/// Serve every bidi stream on one Iroh connection, demultiplexing on the
/// channel-tag byte (`design/11 §3.3`, §6.1).
#[allow(clippy::type_complexity)]
async fn serve_conn<D: ApiDispatcher>(
    conn: Connection,
    dispatcher: Arc<D>,
    core_static: Arc<concerto_identity::NoiseStatic>,
    state: Arc<Mutex<TransportState>>,
    pairing_tx: Arc<Mutex<Option<([u8; 32], mpsc::Sender<IrohDuplex>)>>>,
    shutdown: CancellationToken,
) -> Result<()> {
    // The transport keys its registry on the remote Iroh endpoint id so
    // `close_sessions_for_device` can sever it via the cert→endpoint mapping
    // Task 209/217 maintain (the cert's device_id is resolved inside the gRPC
    // auth layer, not at the transport boundary).
    let remote = conn.remote_id();
    let device_key = DeviceId(remote.to_string());
    {
        let mut st = state.lock().expect("state lock");
        st.insert_session(crate::api::ActiveSession::new(
            device_key.clone(),
            conn.clone(),
        ));
    }

    let result = loop {
        let (send, recv) = tokio::select! {
            _ = shutdown.cancelled() => break Ok(()),
            res = conn.accept_bi() => match res {
                Ok(pair) => pair,
                // Peer closed the connection — normal end of life.
                Err(_) => break Ok(()),
            },
        };
        let duplex = IrohDuplex::new(send, recv);

        let (tag, duplex) = match adapter::read_channel_tag(duplex).await {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(%err, "iroh stream: bad channel tag; dropping stream");
                continue;
            }
        };

        match tag {
            ChannelTag::Api | ChannelTag::PushHint => {
                // Both ride the same gRPC dispatcher; the tag is distinguished
                // so 217/14 can special-case lightweight push-hint handling
                // later without changing the demux contract.
                let dispatcher = dispatcher.clone();
                let core_static = core_static.clone();
                let label = if tag == ChannelTag::Api {
                    "api"
                } else {
                    "push-hint"
                };
                tokio::spawn(async move {
                    match adapter::handshake_responder(duplex, &core_static).await {
                        Ok(noise) => {
                            if let Err(err) = dispatcher.serve_connection(noise).await {
                                tracing::debug!(%err, channel = label, "iroh: serve_connection ended");
                            }
                        }
                        Err(err) => {
                            tracing::warn!(%err, channel = label, "iroh: noise responder handshake failed")
                        }
                    }
                });
            }
            ChannelTag::Pairing => {
                let listener = pairing_tx.lock().expect("pairing lock").clone();
                match listener {
                    Some((_token_hash, tx)) => {
                        // Token-hash gating is enforced by Task 207 inside the
                        // Noise XX over the token; the transport only routes the
                        // raw duplex to the open listener.
                        if tx.send(duplex).await.is_err() {
                            tracing::warn!("iroh pairing: listener closed; dropping stream");
                        }
                    }
                    None => {
                        tracing::warn!("iroh pairing: no open pairing listener; dropping stream");
                    }
                }
            }
        }
    };

    state
        .lock()
        .expect("state lock")
        .sessions
        .remove(&device_key);
    result
}

/// Build the one long-lived Iroh endpoint per `config` (`design/11 §3.1`).
/// Relay-disabled (LAN-only) under `disable_remote`; otherwise relay-enabled
/// with the configured URL (or Iroh's default map). A directly-supplied address
/// is validated here (spike Note B); it is consumed by the *client* dial path,
/// not the endpoint bind.
async fn build_endpoint(config: &TransportConfig) -> Result<Endpoint> {
    if let Some(direct) = &config.direct_addr {
        if !direct.is_empty() {
            direct.parse::<SocketAddr>().map_err(|e| {
                TransportError::Endpoint(format!("invalid direct_addr '{direct}': {e}"))
            })?;
        }
    }

    let mut builder = Endpoint::builder(presets::N0).alpns(vec![ALPN.to_vec()]);

    builder = if config.disable_remote {
        // LAN-only (`design/11 §6.4`): relays disabled → only direct/LAN IP
        // paths. mDNS publication (Task 213) is independent of this.
        builder.relay_mode(RelayMode::Disabled)
    } else {
        match &config.relay_url {
            Some(url) if !url.is_empty() => {
                let relay_url: RelayUrl = url.parse().map_err(|e| {
                    TransportError::Endpoint(format!("invalid relay_url '{url}': {e}"))
                })?;
                let map = RelayMap::from_iter([relay_url]);
                builder.relay_mode(RelayMode::Custom(map))
            }
            // No explicit URL → Iroh's default relay map (per `design/11 §3.1`
            // "default: our hosted; override via managed").
            _ => builder.relay_mode(RelayMode::Default),
        }
    };

    builder
        .bind()
        .await
        .map_err(|e| TransportError::Endpoint(format!("binding iroh endpoint: {e}")))
}

/// Build a Tonic [`Channel`](tonic::transport::Channel) over a fresh Iroh
/// connection to `server_addr`, with the channel-tag + Noise IK initiator
/// layering and the ≥64 MiB message limits (`design/11 §3.1.1`). The **client**
/// entry the Desktop/Mobile (Task 218/509) build their typed gRPC stubs on; the
/// loopback double uses it too.
pub async fn connect_channel(
    client: &Endpoint,
    server_addr: EndpointAddr,
    local_static: Arc<concerto_identity::NoiseStatic>,
    core_noise_pub: [u8; 32],
) -> Result<tonic::transport::Channel> {
    let conn = client
        .connect(server_addr, ALPN)
        .await
        .map_err(|e| TransportError::Connection(format!("iroh connect: {e}")))?;
    let connector = IrohConnector::new(conn, local_static, core_noise_pub);
    let channel = tonic::transport::Endpoint::from_static("http://iroh.invalid")
        .connect_with_connector(connector)
        .await
        .map_err(|e| {
            TransportError::Adapter(format!("tonic connect_with_connector over iroh: {e}"))
        })?;
    Ok(channel)
}

/// Resolve a dialable [`EndpointAddr`] for a server endpoint on the **direct**
/// (LAN/loopback) path — its id plus its learned IP addrs, no relay. Used by the
/// loopback double + by a client given a directly-supplied address. Waits
/// briefly for the endpoint to learn a socket address.
pub async fn direct_endpoint_addr(endpoint: &Endpoint) -> Result<EndpointAddr> {
    let id = endpoint.id();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let addr = endpoint.watch_addr().get();
        if addr.ip_addrs().next().is_some() {
            let mut out = EndpointAddr::new(id);
            for ip in addr.ip_addrs().copied() {
                out = out.with_ip_addr(ip);
            }
            return Ok(out);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(TransportError::Endpoint(
                "endpoint never learned a socket address".into(),
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
