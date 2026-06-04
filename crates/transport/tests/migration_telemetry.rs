//! Tier-2 loopback double for QUIC migration + NAT telemetry (Task 216).
//!
//! Extends Task 212's loopback model (two Iroh endpoints on one host, relays
//! disabled, forced onto the direct loopback IP path) with a **forced/simulated
//! path change** and chosen direct/relayed outcomes. It proves, in CI with **no
//! network**:
//!
//! - a simulated path change (`IrohTransport::note_migration`) **does not** drop
//!   the [`ActiveSession`] or emit `session_closed` — the subscriber survives the
//!   migration blip (the FROZEN migration contract, `design/11 §3.7`, §7.4);
//! - the `NatStats` by-client-kind + by-network-class buckets increment
//!   correctly (`design/11 §2`, §3.6);
//! - `nat_success_changed` fires only on a material threshold crossing
//!   (`design/11 §5.3` debounce);
//! - a **true** connection close removes the session and emits `session_closed`
//!   (the seam a reconnect-with-offset replay through the Task-202 ring buffer
//!   relies on — this task asserts the seam, it does not re-implement replay).
//!
//! It does **NOT** cover a **real device migrating Wi-Fi→LTE** across a real
//! network, or the **real-NAT direct-%** across diverse real NATs — those are
//! physical and are the **Phase-2 Tier-3 checklist** lines + the residual Tier-3
//! rows of `design/spikes/iroh-nat-findings.md` (symmetric↔symmetric, two
//! residential ISPs, UDP-blocking ISP, real cellular handoff). See the task
//! Handoff for the exact Tier-3 lines.

#![cfg(feature = "dev-relay")]

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use concerto_transport::{
    connect_channel, direct_endpoint_addr, nat_success_is_material, ApiDispatcher, ClientKind,
    ConnectionPath, DeviceId, IrohTransport, NatStats, NoiseDuplex, TransportConfig,
    TransportError, TransportTelemetry, MAX_MESSAGE_SIZE, NAT_SUCCESS_DELTA_PCT,
};
use tonic::transport::Server;
use tonic::{Request, Response, Status};

pub mod pb {
    tonic::include_proto!("concerto.transport.loopback.v1");
}

use pb::loopback_client::LoopbackClient;
use pb::loopback_server::{Loopback, LoopbackServer};
use pb::{EchoReply, EchoRequest};

// ---------------------------------------------------------------------------
// Minimal Loopback service + dispatcher (mirrors `tests/loopback.rs`).
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
struct LoopbackSvc;

#[tonic::async_trait]
impl Loopback for LoopbackSvc {
    async fn echo(&self, req: Request<EchoRequest>) -> Result<Response<EchoReply>, Status> {
        Ok(Response::new(EchoReply {
            payload: req.into_inner().payload,
        }))
    }

    type FirehoseStream =
        Pin<Box<dyn futures::Stream<Item = Result<pb::FirehoseChunk, Status>> + Send>>;

    async fn firehose(
        &self,
        _req: Request<pb::FirehoseRequest>,
    ) -> Result<Response<Self::FirehoseStream>, Status> {
        let s = futures::stream::empty();
        Ok(Response::new(Box::pin(s) as Self::FirehoseStream))
    }
}

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

/// Build a relay-disabled server transport + a connected client over the direct
/// loopback path, returning the server transport, the gRPC client, and the live
/// client endpoint (leaked so it outlives the channel). The first RPC establishes
/// the server-side `ActiveSession`.
async fn connect_pair() -> (
    Arc<IrohTransport>,
    LoopbackClient<tonic::transport::Channel>,
    tokio::sync::broadcast::Receiver<TransportTelemetry>,
) {
    let core_seed = [7u8; 32];
    let core_noise_pub = concerto_identity::NoiseStatic::from_private(core_seed)
        .unwrap()
        .public();

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
    // Subscribe BEFORE the serve loop / client connect: the server-side session
    // (and its `session_opened` telemetry) is recorded the instant the Iroh
    // connection is accepted, which races the first RPC. Subscribing here
    // guarantees the test sees the open event.
    let telem = server.subscribe_telemetry();
    {
        let server = server.clone();
        tokio::spawn(async move {
            let _ = server.serve(Arc::new(LoopbackDispatcher)).await;
        });
    }

    let server_addr = direct_endpoint_addr(&server.endpoint())
        .await
        .expect("server direct addr");

    let client_ep: &'static iroh::Endpoint = Box::leak(Box::new(
        iroh::Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .expect("client endpoint bind"),
    ));
    let device_static = Arc::new(concerto_identity::NoiseStatic::generate().unwrap());
    let channel = connect_channel(client_ep, server_addr, device_static, core_noise_pub)
        .await
        .expect("connect channel");
    let client = LoopbackClient::new(channel)
        .max_decoding_message_size(MAX_MESSAGE_SIZE)
        .max_encoding_message_size(MAX_MESSAGE_SIZE);

    (server, client, telem)
}

/// Drain whatever telemetry has been broadcast so far (non-blocking).
fn drain(rx: &mut tokio::sync::broadcast::Receiver<TransportTelemetry>) -> Vec<TransportTelemetry> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

// ---------------------------------------------------------------------------
// Migration: a forced path change does NOT drop the session / emit closed.
// ---------------------------------------------------------------------------

/// The FROZEN migration contract (`design/11 §3.7`, §7.4): a client path change
/// updates the live session's path in place and the subscriber survives it — it
/// does **not** remove the session or broadcast `session_closed`. Simulated on
/// loopback (real Wi-Fi→LTE is Tier-3).
#[tokio::test]
async fn forced_path_change_does_not_drop_session() {
    let (server, mut client, mut telem) = connect_pair().await;

    // Establish the session.
    client
        .echo(Request::new(EchoRequest {
            payload: b"open".to_vec(),
        }))
        .await
        .expect("echo");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let paths = server.session_paths();
    assert_eq!(paths.len(), 1, "exactly one live session");
    let device_id = paths[0].0.clone();

    // A SessionOpened must have been broadcast; no SessionClosed yet.
    let opened = drain(&mut telem);
    assert!(
        opened
            .iter()
            .any(|e| matches!(e, TransportTelemetry::SessionOpened { .. })),
        "session_opened must be emitted on session establishment"
    );
    assert!(
        !opened
            .iter()
            .any(|e| matches!(e, TransportTelemetry::SessionClosed { .. })),
        "no session_closed before any drop"
    );

    // Force a path change (migration). The session must stay live, path
    // re-classified, and NO session_closed emitted.
    let new_path = server.note_migration(&device_id);
    assert!(
        new_path.is_some(),
        "migration on a live session re-classifies"
    );
    assert!(
        server
            .session_paths()
            .iter()
            .any(|(id, _)| *id == device_id),
        "the session survives the migration (not dropped)"
    );

    let after = drain(&mut telem);
    assert!(
        !after
            .iter()
            .any(|e| matches!(e, TransportTelemetry::SessionClosed { .. })),
        "a migration must NOT emit session_closed (the migration contract)"
    );

    // The RPC stream still works after the migration (subscriber survived).
    client
        .echo(Request::new(EchoRequest {
            payload: b"after-migration".to_vec(),
        }))
        .await
        .expect("RPC works after migration");

    server.stop();
}

/// A migration on a device with no live session is a no-op (`None`) — it never
/// fabricates a session.
#[tokio::test]
async fn migration_on_unknown_device_is_noop() {
    let server = Arc::new(
        IrohTransport::start(
            TransportConfig {
                relay_url: None,
                disable_remote: true,
                direct_addr: None,
            },
            [8u8; 32],
        )
        .await
        .expect("start"),
    );
    assert!(server.note_migration(&DeviceId("ghost".into())).is_none());
    server.stop();
}

// ---------------------------------------------------------------------------
// True drop: removes the session + emits session_closed (the 202 seam).
// ---------------------------------------------------------------------------

/// A **true** connection close (NOT a migration) removes the session and emits
/// `session_closed`. This is the seam a reconnecting client relies on: after a
/// true drop the client re-Iroh's, re-Noise's, and replays missed events from
/// offset via the Task-202 ring buffer (replay itself is asserted on the Core
/// side against 202's surface — `crates/core/tests/transport_events.rs`).
#[tokio::test]
async fn true_drop_removes_session_and_emits_closed() {
    let (server, mut client, mut telem) = connect_pair().await;

    client
        .echo(Request::new(EchoRequest {
            payload: b"open".to_vec(),
        }))
        .await
        .expect("echo");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(server.session_paths().len(), 1);
    let _ = drain(&mut telem);

    // Drop the client channel → the Iroh connection closes → the serve loop's
    // accept_bi returns Err → the session is removed and session_closed fires.
    drop(client);

    // Wait for the close to propagate to the server-side serve loop.
    let mut closed_seen = false;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        for ev in drain(&mut telem) {
            if matches!(ev, TransportTelemetry::SessionClosed { .. }) {
                closed_seen = true;
            }
        }
        if server.session_paths().is_empty() && closed_seen {
            break;
        }
    }
    assert!(closed_seen, "a true drop emits session_closed");
    assert!(
        server.session_paths().is_empty(),
        "a true drop removes the session"
    );

    server.stop();
}

// ---------------------------------------------------------------------------
// Telemetry from a real session-open over loopback.
// ---------------------------------------------------------------------------

/// Establishing a session over the loopback double broadcasts `session_opened`
/// (with a non-relayed path + the client kind) and the first
/// `nat_success_changed` (the initial rate). The session-open path increments
/// the by-client-kind + by-network-class buckets.
#[tokio::test]
async fn session_open_emits_telemetry_and_increments_buckets() {
    let (server, mut client, mut telem) = connect_pair().await;

    client
        .echo(Request::new(EchoRequest {
            payload: b"open".to_vec(),
        }))
        .await
        .expect("echo");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let evs = drain(&mut telem);
    let opened = evs
        .iter()
        .find_map(|e| match e {
            TransportTelemetry::SessionOpened {
                path, client_kind, ..
            } => Some((*path, *client_kind)),
            _ => None,
        })
        .expect("session_opened emitted");
    assert_ne!(
        opened.0,
        ConnectionPath::Relayed,
        "loopback path is Lan/Direct, never relayed"
    );
    assert_eq!(
        opened.1,
        ClientKind::DesktopSplitHost,
        "an over-Iroh accept is attributed to the split-host desktop kind by default"
    );
    assert!(
        evs.iter()
            .any(|e| matches!(e, TransportTelemetry::NatSuccessChanged { .. })),
        "the first session emits the initial nat_success_changed"
    );

    // The by-client-kind + by-network-class buckets reflect the open.
    let stats = server.nat_stats();
    let total = stats.direct_today + stats.relayed_today + stats.lan_today;
    assert_eq!(total, 1, "one session counted");
    let by_kind = stats
        .by_client_kind
        .get(&ClientKind::DesktopSplitHost)
        .copied()
        .expect("desktop bucket");
    assert_eq!(by_kind.total(), 1);
    assert_eq!(
        stats
            .by_network_class
            .values()
            .map(|v| v.total())
            .sum::<u32>(),
        1
    );

    server.stop();
}

// ---------------------------------------------------------------------------
// Pure bucket + threshold logic (synthetic events — no network).
// ---------------------------------------------------------------------------

/// A simulated relayed-vs-direct sequence increments the correct
/// by-client-kind + by-network-class buckets (`design/11 §2`, §3.6). Driven on
/// the pure `NatStats` (the loopback path can only produce Lan, so the relayed
/// outcome is simulated here — real relayed-% across real NATs is Tier-3).
#[test]
fn simulated_outcomes_increment_correct_buckets() {
    let mut s = NatStats::default();
    // Mobile, direct.
    s.record(ConnectionPath::Direct, "direct", ClientKind::Mobile);
    // Desktop split-host, relayed (the case `design/11 §2` wants visible).
    s.record(
        ConnectionPath::Relayed,
        "relayed",
        ClientKind::DesktopSplitHost,
    );
    // Desktop split-host, relayed again.
    s.record(
        ConnectionPath::Relayed,
        "relayed",
        ClientKind::DesktopSplitHost,
    );
    // Web, lan.
    s.record(ConnectionPath::Lan, "lan", ClientKind::Web);

    assert_eq!(s.direct_today, 1);
    assert_eq!(s.relayed_today, 2);
    assert_eq!(s.lan_today, 1);

    // Split-host desktop's *worse* direct rate is visible: 0 direct, 2 relayed.
    let desktop = s.by_client_kind[&ClientKind::DesktopSplitHost];
    assert_eq!(desktop.direct, 0);
    assert_eq!(desktop.relayed, 2);
    assert_eq!(desktop.direct_or_lan(), 0);
    // Mobile's better rate is visible: 1 direct.
    assert_eq!(s.by_client_kind[&ClientKind::Mobile].direct, 1);
    // Web counted lan-direct.
    assert_eq!(s.by_client_kind[&ClientKind::Web].lan, 1);

    // by_network_class mirrors the path labels.
    assert_eq!(s.by_network_class["relayed"].relayed, 2);
    assert_eq!(s.by_network_class["direct"].direct, 1);
    assert_eq!(s.by_network_class["lan"].lan, 1);
}

/// `nat_success_changed` is debounced: it fires only on a ≥ delta move or a
/// crossing of the 70% PRD line, never on a sub-threshold wobble (`design/11
/// §5.3`).
#[test]
fn nat_success_threshold_is_debounced() {
    // A sub-delta move (< NAT_SUCCESS_DELTA_PCT, same side of the 70% line) is
    // NOT material.
    assert!(!nat_success_is_material(
        80,
        80 + (NAT_SUCCESS_DELTA_PCT - 1)
    ));
    assert!(!nat_success_is_material(
        50,
        50 + (NAT_SUCCESS_DELTA_PCT - 1)
    ));

    // A move of exactly the delta IS material.
    assert!(nat_success_is_material(80, 80 - NAT_SUCCESS_DELTA_PCT));

    // Crossing the 70% PRD line is ALWAYS material, even for a tiny move.
    assert!(nat_success_is_material(71, 70));
    assert!(nat_success_is_material(70, 71));
}

/// `direct_percent` counts direct + lan as the "did not need a relay" numerator
/// and is exact at the boundaries.
#[test]
fn direct_percent_math() {
    let mut s = NatStats::default();
    assert_eq!(s.direct_percent(), 0, "no sessions → 0%");
    s.record(ConnectionPath::Direct, "direct", ClientKind::Mobile);
    assert_eq!(s.direct_percent(), 100);
    s.record(ConnectionPath::Relayed, "relayed", ClientKind::Mobile);
    assert_eq!(s.direct_percent(), 50, "1 direct of 2 → 50%");
    s.record(ConnectionPath::Lan, "lan", ClientKind::Web);
    assert_eq!(s.direct_percent(), 66, "(1 direct + 1 lan) of 3 → 66%");
}
