//! Tonic-over-Iroh latency & throughput spike (Task 102).
//!
//! Serves a trivial Tonic service (one unary echo + one server-streaming
//! byte-firehose) over three transports and benchmarks each.
//!
//! UDS — bare Unix-domain socket, the baseline the V1.0 envelope is measured
//! against. Iroh-direct — two Iroh endpoints on one host, relays disabled, so
//! the only viable QUIC path is the direct (loopback) IP path; the Tier-2
//! loopback double for the LAN-direct remote case. Iroh-relay — two Iroh
//! endpoints whose IP transports are cleared and that are pointed at a LOCAL
//! in-process `iroh-relay` dev instance, so every byte is forced through the
//! relay.
//!
//! For each transport we measure unary p50/p95 round-trip and streaming MB/s.
//! See the binary (`src/bin/bench.rs`) for the driver and `design/spikes/
//! tonic-iroh-findings.md` for the verdict.
//!
//! Throwaway measurement code, not the product transport (that is Task 212).

pub mod iroh_adapter;

/// Generated Tonic stubs for `proto/bench.proto` (tonic 0.12 / prost 0.13
/// codegen — the production generator).
pub mod pb {
    tonic::include_proto!("bench.v1");
}

use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::time::Duration;

use anyhow::{Context as _, Result};
use futures::Stream;
use iroh::endpoint::{presets, Connection, RelayMode};
use iroh::{Endpoint, EndpointAddr, RelayMap, RelayUrl, Watcher};
use iroh_relay::server::{
    AccessConfig, RelayConfig, Server as RelayServer, ServerConfig as RelayServerConfig,
};
use tokio::sync::oneshot;
use tokio_util_min::CancellationToken;
use tonic::{Request, Response, Status};

// Re-export the generated server type + the service impl + the cancellation
// token so the binary can drive a UDS server with the same shutdown primitive.
pub use pb::bench_server::BenchServer;
pub use tokio_util_min::CancellationToken as ShutdownToken;

use pb::bench_server::Bench;
use pb::{EchoReply, EchoRequest, FirehoseChunk, FirehoseRequest};

/// Construct a fresh shutdown token (used by the UDS path in the binary).
pub fn new_shutdown_token() -> ShutdownToken {
    ShutdownToken::new()
}

/// ALPN for the spike's gRPC-over-Iroh transport. Throwaway value.
pub const ALPN: &[u8] = b"concerto/tonic-iroh-spike/0";

/// Minimal cancellation token so the spike does not pull `tokio-util` just for
/// this. A `tokio::sync::watch` under the hood.
mod tokio_util_min {
    use tokio::sync::watch;

    /// A one-shot broadcast "shutdown now" signal, cloneable across tasks.
    #[derive(Clone)]
    pub struct CancellationToken {
        rx: watch::Receiver<bool>,
        tx: std::sync::Arc<watch::Sender<bool>>,
    }

    impl Default for CancellationToken {
        fn default() -> Self {
            Self::new()
        }
    }

    impl CancellationToken {
        pub fn new() -> Self {
            let (tx, rx) = watch::channel(false);
            Self {
                rx,
                tx: std::sync::Arc::new(tx),
            }
        }

        pub fn cancel(&self) {
            let _ = self.tx.send(true);
        }

        pub async fn cancelled(&self) {
            let mut rx = self.rx.clone();
            if *rx.borrow() {
                return;
            }
            // Wait until the value flips to true (or the sender is dropped).
            while rx.changed().await.is_ok() {
                if *rx.borrow() {
                    return;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The Bench gRPC service
// ---------------------------------------------------------------------------

/// The benchmark service implementation, shared by all three transports so the
/// only variable across runs is the transport itself.
#[derive(Default, Clone)]
pub struct BenchSvc;

type FirehoseStream = Pin<Box<dyn Stream<Item = Result<FirehoseChunk, Status>> + Send>>;

#[tonic::async_trait]
impl Bench for BenchSvc {
    async fn echo(&self, req: Request<EchoRequest>) -> Result<Response<EchoReply>, Status> {
        let payload = req.into_inner().payload;
        Ok(Response::new(EchoReply { payload }))
    }

    type FirehoseStream = FirehoseStream;

    async fn firehose(
        &self,
        req: Request<FirehoseRequest>,
    ) -> Result<Response<Self::FirehoseStream>, Status> {
        let FirehoseRequest {
            total_bytes,
            chunk_bytes,
        } = req.into_inner();
        let chunk_bytes = chunk_bytes.max(1) as usize;
        // Pre-build one chunk and reuse the buffer across the stream so the
        // measurement reflects transport throughput, not allocation.
        let chunk = bytes::Bytes::from(vec![0x5Au8; chunk_bytes]);

        let stream = async_stream::stream(total_bytes, chunk_bytes, chunk);
        Ok(Response::new(Box::pin(stream) as Self::FirehoseStream))
    }
}

/// Hand-rolled chunk stream (avoids pulling the `async-stream` crate).
mod async_stream {
    use super::{FirehoseChunk, Status};
    use futures::Stream;

    pub fn stream(
        total_bytes: u64,
        chunk_bytes: usize,
        chunk: bytes::Bytes,
    ) -> impl Stream<Item = Result<FirehoseChunk, Status>> + Send {
        let mut remaining = total_bytes;
        futures::stream::poll_fn(move |_cx| {
            if remaining == 0 {
                return std::task::Poll::Ready(None);
            }
            let take = remaining.min(chunk_bytes as u64) as usize;
            remaining -= take as u64;
            let data = if take == chunk_bytes {
                chunk.clone()
            } else {
                chunk.slice(0..take)
            };
            std::task::Poll::Ready(Some(Ok(FirehoseChunk { data })))
        })
    }
}

// ---------------------------------------------------------------------------
// Iroh endpoint construction
// ---------------------------------------------------------------------------

/// Build a pair of Iroh endpoints wired for the **direct** path: relays
/// disabled on both, so the only reachable QUIC path is a direct IP path
/// (loopback on a single host). Returns `(server, client)`.
pub async fn build_direct_pair() -> Result<(Endpoint, Endpoint)> {
    let server = Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .context("binding direct server endpoint")?;
    let client = Endpoint::builder(presets::N0)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .context("binding direct client endpoint")?;
    Ok((server, client))
}

/// Build a pair of Iroh endpoints wired for the **relayed** path: IP
/// transports cleared and both pointed at `relay_url`, so every byte is forced
/// through the relay. Returns `(server, client)`.
pub async fn build_relay_pair(relay_url: &RelayUrl) -> Result<(Endpoint, Endpoint)> {
    let map = RelayMap::from_iter([relay_url.clone()]);
    let server = Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .clear_ip_transports()
        .relay_mode(RelayMode::Custom(map.clone()))
        .bind()
        .await
        .context("binding relay server endpoint")?;
    let client = Endpoint::builder(presets::N0)
        .clear_ip_transports()
        .relay_mode(RelayMode::Custom(map))
        .bind()
        .await
        .context("binding relay client endpoint")?;
    Ok((server, client))
}

/// Resolve the server endpoint's dialable [`EndpointAddr`] for the direct path
/// (direct IP addrs only, no relay). Waits briefly for the endpoint to learn
/// its own socket addresses.
pub async fn direct_server_addr(server: &Endpoint) -> Result<EndpointAddr> {
    let id = server.id();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let addr = server.watch_addr().get();
        if addr.ip_addrs().next().is_some() {
            // Keep only direct IP addrs (relay is disabled anyway).
            let ips: Vec<SocketAddr> = addr.ip_addrs().copied().collect();
            let mut out = EndpointAddr::new(id);
            for ip in ips {
                out = out.with_ip_addr(ip);
            }
            return Ok(out);
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("direct server endpoint never learned a socket address");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Build the relayed-path [`EndpointAddr`] for the server: its id plus the
/// relay URL only (no direct IP addrs).
pub fn relay_server_addr(server: &Endpoint, relay_url: &RelayUrl) -> EndpointAddr {
    EndpointAddr::new(server.id()).with_relay_url(relay_url.clone())
}

// ---------------------------------------------------------------------------
// In-process iroh-relay dev server
// ---------------------------------------------------------------------------

/// A running in-process `iroh-relay` dev instance (plain HTTP, no TLS — the
/// hermetic equivalent of `iroh-relay --dev`). Holds the server so it stays
/// alive; `url()` is what both endpoints dial.
pub struct DevRelay {
    server: RelayServer,
    url: RelayUrl,
}

impl DevRelay {
    /// Stand up a plain-HTTP relay on an OS-assigned loopback port.
    pub async fn spawn() -> Result<Self> {
        let config = RelayServerConfig::<(), ()> {
            relay: Some(RelayConfig::<(), ()> {
                http_bind_addr: (Ipv4Addr::LOCALHOST, 0).into(),
                tls: None,
                limits: Default::default(),
                key_cache_capacity: Some(1024),
                access: AccessConfig::Everyone,
            }),
            quic: None,
            metrics_addr: None,
        };
        let server = RelayServer::spawn(config)
            .await
            .context("spawning in-process iroh-relay dev server")?;
        let http_addr = server
            .http_addr()
            .context("dev relay reported no http addr")?;
        let url: RelayUrl = format!("http://{http_addr}")
            .parse()
            .context("building relay url from dev relay http addr")?;
        Ok(Self { server, url })
    }

    pub fn url(&self) -> &RelayUrl {
        &self.url
    }

    /// Shut the relay down cleanly.
    pub async fn shutdown(self) -> Result<()> {
        self.server
            .shutdown()
            .await
            .context("shutting down dev relay")?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Iroh gRPC server: accept one connection, serve every bidi stream as a Tonic
// "connection".
// ---------------------------------------------------------------------------

/// Run a Tonic `Bench` server over an Iroh endpoint until `shutdown` fires.
///
/// Each inbound Iroh connection gets a task; each inbound bidi stream on that
/// connection is fed to a fresh `serve_with_incoming` as a single-element
/// incoming stream (one duplex = one Tonic connection). This is the realistic
/// "QUIC stream pool for gRPC" shape from `design/11 §3.3`.
pub async fn serve_iroh(endpoint: Endpoint, shutdown: CancellationToken) -> Result<()> {
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { break };
                let sd = shutdown.clone();
                tokio::spawn(async move {
                    match incoming.await {
                        Ok(conn) => {
                            if let Err(err) = serve_conn(conn, sd).await {
                                tracing::warn!(?err, "iroh conn server error");
                            }
                        }
                        Err(err) => tracing::warn!(?err, "iroh incoming failed"),
                    }
                });
            }
        }
    }
    endpoint.close().await;
    Ok(())
}

/// Serve every bidi stream on a single Iroh connection as its own Tonic
/// connection.
async fn serve_conn(conn: Connection, shutdown: CancellationToken) -> Result<()> {
    loop {
        let duplex = tokio::select! {
            _ = shutdown.cancelled() => break,
            res = iroh_adapter::accept_duplex(&conn) => match res {
                Ok(d) => d,
                // Connection closed by peer — normal end of life.
                Err(_) => break,
            },
        };

        tokio::spawn(async move {
            let incoming = futures::stream::once(async move {
                Ok::<_, std::io::Error>(duplex)
            });
            let svc = BenchServer::new(BenchSvc)
                // Lift the gRPC message-size ceiling so the firehose isn't
                // capped by the default 4 MiB decode limit on either side.
                .max_decoding_message_size(64 * 1024 * 1024)
                .max_encoding_message_size(64 * 1024 * 1024);
            if let Err(err) = tonic::transport::Server::builder()
                .add_service(svc)
                .serve_with_incoming(incoming)
                .await
            {
                tracing::debug!(?err, "tonic serve_with_incoming ended");
            }
        });
    }
    Ok(())
}

/// Spawn the Iroh `Bench` server in the background, returning a handle that
/// shuts it down when dropped/awaited and the endpoint id for the client to
/// dial. The caller supplies the already-built server endpoint.
pub fn spawn_iroh_server(endpoint: Endpoint) -> IrohServerHandle {
    let shutdown = CancellationToken::new();
    let sd = shutdown.clone();
    let (done_tx, done_rx) = oneshot::channel();
    tokio::spawn(async move {
        let _ = serve_iroh(endpoint, sd).await;
        let _ = done_tx.send(());
    });
    IrohServerHandle {
        shutdown,
        done: done_rx,
    }
}

/// Handle to a running Iroh `Bench` server.
pub struct IrohServerHandle {
    shutdown: CancellationToken,
    done: oneshot::Receiver<()>,
}

impl IrohServerHandle {
    pub async fn stop(self) {
        self.shutdown.cancel();
        let _ = self.done.await;
    }
}

// (No `SharedRelay` alias: the binary keeps the `DevRelay` alive directly.)

/// Connect a Tonic `Bench` client over an Iroh connection (one shared QUIC
/// connection; tonic opens a fresh bidi stream per channel).
pub async fn connect_iroh_client(
    client: &Endpoint,
    server_addr: EndpointAddr,
) -> Result<pb::bench_client::BenchClient<tonic::transport::Channel>> {
    let conn = client
        .connect(server_addr, ALPN)
        .await
        .context("iroh connect to bench server")?;
    let connector = iroh_adapter::IrohConnector::new(conn);
    // The URI is ignored by our connector but tonic requires a valid one.
    let channel = tonic::transport::Endpoint::from_static("http://iroh.invalid")
        .connect_with_connector(connector)
        .await
        .context("tonic connect_with_connector over iroh")?;
    let client = pb::bench_client::BenchClient::new(channel)
        .max_decoding_message_size(64 * 1024 * 1024)
        .max_encoding_message_size(64 * 1024 * 1024);
    Ok(client)
}
