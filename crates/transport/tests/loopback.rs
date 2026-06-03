//! Tier-2 loopback double for the Iroh transport (Task 212).
//!
//! **Two Iroh endpoints on one host, relays disabled, forced onto the direct
//! loopback IP path** (the spike's Tier-2 model). Proves the full
//! **gRPC-over-Iroh + hand-rolled adapter + Noise IK + channel multiplexing +
//! `ConnectionPath` classification** path end to end in CI with **no network**.
//!
//! It does **NOT** cover real-NAT hole-punch, a real WAN relay, or real
//! connection migration — those are Tier-3 (the Phase-2 manual checklist / the
//! spike 101 field matrix). See the task Handoff for the Tier-3 lines.
//!
//! The double serves the trivial `Loopback` service (one unary echo, one
//! server-streaming firehose — generated from `proto/loopback.proto` by
//! `build.rs`) over the production tonic-0.12 codegen, exactly as the Core's
//! real services ride the adapter.

#![cfg(feature = "dev-relay")]

use std::pin::Pin;
use std::sync::Arc;

use concerto_transport::{
    connect_channel, direct_endpoint_addr, ApiDispatcher, ConnectionPath, IrohTransport,
    NoiseDuplex, TransportConfig, TransportError, MAX_MESSAGE_SIZE,
};
use futures::Stream;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

pub mod pb {
    tonic::include_proto!("concerto.transport.loopback.v1");
}

use pb::loopback_client::LoopbackClient;
use pb::loopback_server::{Loopback, LoopbackServer};
use pb::{EchoReply, EchoRequest, FirehoseChunk, FirehoseRequest};

// ---------------------------------------------------------------------------
// The trivial Loopback service
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct LoopbackSvc;

type FirehoseStream = Pin<Box<dyn Stream<Item = Result<FirehoseChunk, Status>> + Send>>;

#[tonic::async_trait]
impl Loopback for LoopbackSvc {
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
        let chunk_bytes = (chunk_bytes.max(1)) as usize;
        let chunk = vec![0x5Au8; chunk_bytes];
        let mut remaining = total_bytes;
        let stream = futures::stream::poll_fn(move |_cx| {
            if remaining == 0 {
                return std::task::Poll::Ready(None);
            }
            let take = remaining.min(chunk_bytes as u64) as usize;
            remaining -= take as u64;
            let data = chunk[..take].to_vec();
            std::task::Poll::Ready(Some(Ok(FirehoseChunk { data })))
        });
        Ok(Response::new(Box::pin(stream) as Self::FirehoseStream))
    }
}

/// The `ApiDispatcher` the transport's serve loop hands each Noise-wrapped API
/// stream to. Mirrors the Core's `serve_iroh`: build one tonic `Server` over the
/// single-element incoming stream (one gRPC connection == one Iroh bidi stream),
/// with the 64 MiB limits lifted.
struct LoopbackDispatcher;

impl ApiDispatcher for LoopbackDispatcher {
    fn serve_connection(
        &self,
        io: NoiseDuplex,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), TransportError>> + Send>> {
        Box::pin(async move {
            let svc = LoopbackServer::new(LoopbackSvc)
                .max_decoding_message_size(MAX_MESSAGE_SIZE)
                .max_encoding_message_size(MAX_MESSAGE_SIZE);
            let incoming = futures::stream::once(async move { Ok::<_, std::io::Error>(io) });
            Server::builder()
                .add_service(svc)
                .serve_with_incoming(incoming)
                .await
                .map_err(|e| TransportError::Adapter(format!("serve_with_incoming: {e}")))
        })
    }
}

// ---------------------------------------------------------------------------
// Harness: two endpoints, relays disabled, direct loopback path.
// ---------------------------------------------------------------------------

/// Build a server transport + a client endpoint, both relay-disabled, plus the
/// matched Noise statics, and return a connected tonic `LoopbackClient` + the
/// server handle.
///
/// The Core (responder) Noise static is derived deterministically from a known
/// seed via `from_private`, so the test can both (a) hand that seed to
/// `IrohTransport::start` (the persistence path) and (b) pre-load the matching
/// public half on the client. The device (initiator) static is fresh per run.
async fn connect_pair() -> (
    Arc<IrohTransport>,
    LoopbackClient<tonic::transport::Channel>,
) {
    let core_seed = [3u8; 32];
    let core_noise_pub = concerto_identity::NoiseStatic::from_private(core_seed)
        .unwrap()
        .public();

    // Server transport: LAN-only forces relays off → the only path is direct
    // loopback. Started with `core_seed` so its internal Core static matches
    // `core_noise_pub`.
    let server = Arc::new(
        IrohTransport::start(
            TransportConfig {
                relay_url: None,
                disable_remote: true,
                direct_addr: None,
            },
            core_seed,
        )
        .await
        .expect("server transport start"),
    );

    {
        let server = server.clone();
        tokio::spawn(async move {
            let _ = server.serve(Arc::new(LoopbackDispatcher)).await;
        });
    }

    // The server's dialable direct address (loopback IP, no relay).
    let server_addr = direct_endpoint_addr(&server.endpoint())
        .await
        .expect("server direct addr");

    // Client endpoint, relay disabled. Leaked so it outlives the channel for the
    // test's duration (the channel holds the Iroh `Connection`, which needs the
    // endpoint alive).
    let client_ep: &'static iroh::Endpoint = Box::leak(Box::new(
        iroh::Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .expect("client endpoint bind"),
    ));

    // The device (initiator) Noise static — fresh per run.
    let device_static = Arc::new(concerto_identity::NoiseStatic::generate().unwrap());

    let channel = connect_channel(client_ep, server_addr, device_static, core_noise_pub)
        .await
        .expect("connect channel");
    let client = LoopbackClient::new(channel)
        .max_decoding_message_size(MAX_MESSAGE_SIZE)
        .max_encoding_message_size(MAX_MESSAGE_SIZE);

    (server, client)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A full unary gRPC round-trip over Iroh + the adapter + Noise — proves the
/// architectural claim that the Tonic stack runs unmodified over Iroh.
#[tokio::test]
async fn unary_round_trip_over_iroh_and_noise() {
    let (server, mut client) = connect_pair().await;

    let reply = client
        .echo(Request::new(EchoRequest {
            payload: b"hello over iroh".to_vec(),
        }))
        .await
        .expect("echo")
        .into_inner();
    assert_eq!(reply.payload, b"hello over iroh");

    server.stop();
}

/// A server-streaming subject over the adapter + Noise — the `session.io` shape.
#[tokio::test]
async fn server_streaming_over_iroh_and_noise() {
    let (server, mut client) = connect_pair().await;

    let total: u64 = 4 * 1024 * 1024; // 4 MiB across 64 KiB chunks
    let mut stream = client
        .firehose(Request::new(FirehoseRequest {
            total_bytes: total,
            chunk_bytes: 64 * 1024,
        }))
        .await
        .expect("open firehose")
        .into_inner();

    let mut received: u64 = 0;
    while let Some(chunk) = stream.message().await.expect("firehose recv") {
        received += chunk.data.len() as u64;
    }
    assert_eq!(received, total, "streamed byte count");

    server.stop();
}

/// Gotcha #4: a single message larger than Tonic's default 4 MiB ceiling round
/// trips, because both ends lifted the limit to 64 MiB.
#[tokio::test]
async fn large_message_over_4mib_round_trips() {
    let (server, mut client) = connect_pair().await;

    let big = vec![0xABu8; 8 * 1024 * 1024]; // 8 MiB > the 4 MiB default ceiling
    let reply = client
        .echo(Request::new(EchoRequest {
            payload: big.clone(),
        }))
        .await
        .expect("large echo")
        .into_inner();
    assert_eq!(reply.payload.len(), big.len());
    assert_eq!(reply.payload, big);

    server.stop();
}

/// Gotcha #3: the very first RPC on a fresh connection does not stall — the
/// channel-tag priming write wakes `accept_bi` promptly. A tight timeout asserts
/// no first-RPC-stall.
#[tokio::test]
async fn first_rpc_does_not_stall() {
    let (server, mut client) = connect_pair().await;

    let fut = client.echo(Request::new(EchoRequest {
        payload: b"prime".to_vec(),
    }));
    let reply = tokio::time::timeout(std::time::Duration::from_secs(5), fut)
        .await
        .expect("first RPC must not stall (acceptor priming)")
        .expect("echo")
        .into_inner();
    assert_eq!(reply.payload, b"prime");

    server.stop();
}

/// `disable_remote = true` (Task 211) refuses relay registration / switch — the
/// LAN-only Core never contacts a relay.
#[tokio::test]
async fn disable_remote_refuses_relay() {
    let transport = IrohTransport::start(
        TransportConfig {
            relay_url: Some("https://relay.example/concerto".into()),
            disable_remote: true,
            direct_addr: None,
        },
        [9u8; 32],
    )
    .await
    .expect("transport start");

    // Under disable_remote the relay association is None (no registration).
    let relay = transport.current_relay();
    assert!(relay.remote_disabled);
    assert_eq!(relay.url, None, "LAN-only Core registers with no relay");

    // switch_relay is refused.
    let err = transport.switch_relay("https://other.example".into());
    assert!(matches!(err, Err(TransportError::RemoteDisabled(_))));

    transport.stop();
}

/// A relay-enabled Core *does* keep its configured relay URL (the complementary
/// case — proving the gate is conditioned on `disable_remote`, not always-off).
#[tokio::test]
async fn relay_enabled_keeps_url_and_allows_switch() {
    let transport = IrohTransport::start(
        TransportConfig {
            relay_url: Some("https://relay.example/concerto".into()),
            disable_remote: false,
            direct_addr: None,
        },
        [11u8; 32],
    )
    .await
    .expect("transport start");

    assert_eq!(
        transport.current_relay().url.as_deref(),
        Some("https://relay.example/concerto")
    );
    transport
        .switch_relay("https://relay2.example/concerto".into())
        .expect("switch allowed when remote enabled");
    assert_eq!(
        transport.current_relay().url.as_deref(),
        Some("https://relay2.example/concerto")
    );

    transport.stop();
}

/// The session is classified as a LAN/Direct (non-relayed) path on the loopback
/// double — relays are disabled, so a relayed classification would be a bug.
#[tokio::test]
async fn connection_path_is_local_on_loopback() {
    let (server, mut client) = connect_pair().await;

    // Drive one RPC so a session is established + a path selected.
    let _ = client
        .echo(Request::new(EchoRequest {
            payload: b"path".to_vec(),
        }))
        .await
        .expect("echo");

    // Give the path watcher a moment to settle on the loopback IP path.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let paths = server.session_paths();
    assert!(!paths.is_empty(), "a session should be registered");
    for (_id, path) in &paths {
        assert_ne!(
            *path,
            ConnectionPath::Relayed,
            "relays are disabled on the loopback double; path must be Lan or Direct, got {path:?}"
        );
    }

    server.stop();
}

/// A directly-supplied bad address is a clear config error at `start` (spike
/// Note B's directly-supplied-address path, validated up front).
#[tokio::test]
async fn invalid_direct_addr_is_a_config_error() {
    let err = IrohTransport::start(
        TransportConfig {
            relay_url: None,
            disable_remote: true,
            direct_addr: Some("not-an-addr".into()),
        },
        [1u8; 32],
    )
    .await;
    assert!(matches!(err, Err(TransportError::Endpoint(_))));
}
