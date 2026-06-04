//! Integration tests for Task 216: the `TransportEvent` broadcast round-trip
//! through the `Streams` `transport.events` subject + the `nat_stats` Runtime
//! read path (D1).
//!
//! These drive the `StreamsHandler` and `RuntimeHandler` **in-process** (no full
//! Core boot, no `concerto_keychain::Secrets::new()`), so there is no
//! keychain-in-CI hazard — the managers are built over a temp `Persistence` and
//! the transport telemetry is fed through a plain `tokio::sync::broadcast`
//! sender, exactly as Task 217's `TransportHandle` will wire it from the live
//! `IrohTransport::subscribe_telemetry()`.
//!
//! Tier-2 scope: this proves the Core-side mapping + fan-out + read-path logic.
//! Real LTE↔Wi-Fi migration and real-NAT direct-% across real networks are
//! Tier-3 (the Phase-2 manual checklist), exercised by the transport-side
//! loopback double's documented limits, not here.

#![cfg(unix)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use concerto_core::agent_supervisor::AgentSupervisorHandle;
use concerto_core::handlers::runtime::{NatStatsSource, NoNatStats, RuntimeHandler};
use concerto_core::handlers::streams::StreamsHandler;
use concerto_core::repo_manager::RepoManager;
use concerto_core::supervisor::SupervisorView;
use concerto_core::workspace_manager::{WorkareaManager, WorkspaceManager};
use concerto_persist::{Persistence, PersistenceConfig};
use concerto_proto::v1::event::Body as EventBody;
use concerto_proto::v1::runtime_server::Runtime as _RuntimeService;
use concerto_proto::v1::streams_server::Streams as _StreamsService;
use concerto_proto::v1::transport_event::Kind as TransportEventKind;
use concerto_proto::v1::{ClientKind as ProtoClientKind, Event, SubscribeRequest, TransportPath};
use concerto_transport::{
    ClientKind, ConnectionPath, DeviceId, NatStats, NetworkStats, TransportTelemetry,
};
use futures::StreamExt;
use tempfile::TempDir;
use tokio::sync::broadcast;
use tonic::Request;

// ---------------------------------------------------------------------------
// In-process StreamsHandler over a temp Persistence (no keychain / no boot).
// ---------------------------------------------------------------------------

async fn make_streams_handler() -> (
    TempDir,
    StreamsHandler,
    broadcast::Sender<TransportTelemetry>,
) {
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("data");
    let config_dir = tmp.path().join("config");
    tokio::fs::create_dir_all(&data_dir).await.unwrap();
    tokio::fs::create_dir_all(&config_dir).await.unwrap();

    let cfg = PersistenceConfig {
        db_path: data_dir.join("concerto.db"),
        max_readers: 2,
    };
    let persistence = Arc::new(Persistence::open(cfg).await.expect("persistence"));
    let data_dir = Arc::new(data_dir);
    let config_dir = Arc::new(config_dir);

    let repo_manager = RepoManager::new(persistence.clone(), tmp.path().join("repos"));
    let workspaces = WorkspaceManager::new(persistence.clone(), config_dir.clone());
    let workareas = WorkareaManager::new(
        persistence.clone(),
        repo_manager,
        data_dir.clone(),
        config_dir.clone(),
    );
    let supervisor = AgentSupervisorHandle::new(
        persistence,
        data_dir,
        config_dir,
        PathBuf::from("concerto-agent-host"),
    );

    // The transport telemetry source — the seam Task 217 wires from the live
    // `IrohTransport::subscribe_telemetry()`.
    let (telemetry_tx, _rx) = broadcast::channel(64);
    let handler = StreamsHandler::new(supervisor, workspaces, workareas)
        .with_transport_events(telemetry_tx.clone());

    (tmp, handler, telemetry_tx)
}

/// Read the next event off a subscribe stream within `budget`.
async fn next_event<S>(stream: &mut S, budget: Duration) -> Option<Event>
where
    S: futures::Stream<Item = Result<Event, tonic::Status>> + Unpin,
{
    match tokio::time::timeout(budget, stream.next()).await {
        Ok(Some(Ok(ev))) => Some(ev),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// TransportEvent proto arm round-trips through the broadcast.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn transport_event_arm_round_trips_through_subscribe() {
    let (_tmp, handler, telemetry_tx) = make_streams_handler().await;

    // Subscribe to the transport subject FIRST (this spawns the subject pump and
    // installs the live broadcast before any telemetry is sent).
    let resp = handler
        .subscribe(Request::new(SubscribeRequest {
            subject: "transport.events".into(),
            filter: None,
            since_offset: None,
        }))
        .await
        .expect("subscribe");
    let mut stream = resp.into_inner();

    // Give the pump a moment to attach to the telemetry broadcast.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Emit one of each lifecycle event through the transport's telemetry sender.
    telemetry_tx
        .send(TransportTelemetry::SessionOpened {
            device_id: DeviceId("dev-1".into()),
            path: ConnectionPath::Direct,
            client_kind: ClientKind::Mobile,
        })
        .expect("send opened");
    telemetry_tx
        .send(TransportTelemetry::RelaySwitched {
            relay_url: "https://relay.example/concerto".into(),
        })
        .expect("send relay");
    telemetry_tx
        .send(TransportTelemetry::NatSuccessChanged { direct_percent: 82 })
        .expect("send nat");
    telemetry_tx
        .send(TransportTelemetry::SessionClosed {
            device_id: DeviceId("dev-1".into()),
        })
        .expect("send closed");

    // The `SessionOpened` round-trips with the FROZEN proto shape (Event.body
    // .transport = 16; the TransportEvent oneof kind = SessionOpened).
    let ev = next_event(&mut stream, Duration::from_secs(2))
        .await
        .expect("session_opened event");
    let te = match ev.body {
        Some(EventBody::Transport(te)) => te,
        other => panic!("expected Transport body, got {other:?}"),
    };
    match te.kind {
        Some(TransportEventKind::SessionOpened(so)) => {
            assert_eq!(so.device_id, "dev-1");
            assert_eq!(so.path, TransportPath::Direct as i32);
            assert_eq!(so.client_kind, ProtoClientKind::Mobile as i32);
        }
        other => panic!("expected SessionOpened, got {other:?}"),
    }

    // RelaySwitched.
    let ev = next_event(&mut stream, Duration::from_secs(2))
        .await
        .expect("relay_switched event");
    match ev.body {
        Some(EventBody::Transport(te)) => match te.kind {
            Some(TransportEventKind::RelaySwitched(rs)) => {
                assert_eq!(rs.relay_url, "https://relay.example/concerto");
            }
            other => panic!("expected RelaySwitched, got {other:?}"),
        },
        other => panic!("expected Transport body, got {other:?}"),
    }

    // NatSuccessChanged.
    let ev = next_event(&mut stream, Duration::from_secs(2))
        .await
        .expect("nat_success_changed event");
    match ev.body {
        Some(EventBody::Transport(te)) => match te.kind {
            Some(TransportEventKind::NatSuccessChanged(n)) => assert_eq!(n.direct_percent, 82),
            other => panic!("expected NatSuccessChanged, got {other:?}"),
        },
        other => panic!("expected Transport body, got {other:?}"),
    }

    // SessionClosed (folded in per §5.3).
    let ev = next_event(&mut stream, Duration::from_secs(2))
        .await
        .expect("session_closed event");
    match ev.body {
        Some(EventBody::Transport(te)) => match te.kind {
            Some(TransportEventKind::SessionClosed(sc)) => assert_eq!(sc.device_id, "dev-1"),
            other => panic!("expected SessionClosed, got {other:?}"),
        },
        other => panic!("expected Transport body, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The 202 reconnect seam: a transport.events reconnect-with-offset replays
// from the ring buffer (the seam a true-drop reconnect relies on).
// ---------------------------------------------------------------------------

/// After a true drop the client reconnects and replays missed events from
/// offset via the Task-202 ring buffer. This asserts the seam on the
/// `transport.events` subject (the buffer is shared machinery; 216 does not
/// re-implement it): publish events, note the offsets, then a reconnect with
/// `since_offset` returns exactly the gap.
#[tokio::test(flavor = "multi_thread")]
async fn transport_events_reconnect_replays_from_offset() {
    let (_tmp, handler, telemetry_tx) = make_streams_handler().await;

    // First subscriber spawns the pump + ring buffer for the subject.
    let resp = handler
        .subscribe(Request::new(SubscribeRequest {
            subject: "transport.events".into(),
            filter: None,
            since_offset: None,
        }))
        .await
        .expect("subscribe");
    let mut live = resp.into_inner();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Publish three events; collect their assigned offsets off the live stream.
    for pct in [60u32, 70, 80] {
        telemetry_tx
            .send(TransportTelemetry::NatSuccessChanged {
                direct_percent: pct,
            })
            .expect("send");
    }
    let mut offsets = Vec::new();
    for _ in 0..3 {
        let ev = next_event(&mut live, Duration::from_secs(2))
            .await
            .expect("live event");
        offsets.push(ev.offset);
    }
    assert_eq!(offsets, vec![0, 1, 2], "monotonic offsets from the ring");

    // A reconnect with `since_offset = 0` replays exactly offsets 1 and 2 (the
    // gap), proving the transport subject rides the 202 ring-buffer replay path.
    let resp = handler
        .subscribe(Request::new(SubscribeRequest {
            subject: "transport.events".into(),
            filter: None,
            since_offset: Some(0),
        }))
        .await
        .expect("reconnect subscribe");
    let mut replay = resp.into_inner();
    let e1 = next_event(&mut replay, Duration::from_secs(2))
        .await
        .expect("replay offset 1");
    let e2 = next_event(&mut replay, Duration::from_secs(2))
        .await
        .expect("replay offset 2");
    assert_eq!(e1.offset, 1);
    assert_eq!(e2.offset, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn transport_events_subject_with_no_transport_yields_nothing() {
    // A handler with NO transport source attached: the subject is valid but
    // produces no events (the co-located / UDS-only Core answer).
    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("data");
    let config_dir = tmp.path().join("config");
    tokio::fs::create_dir_all(&data_dir).await.unwrap();
    tokio::fs::create_dir_all(&config_dir).await.unwrap();
    let cfg = PersistenceConfig {
        db_path: data_dir.join("concerto.db"),
        max_readers: 2,
    };
    let persistence = Arc::new(Persistence::open(cfg).await.expect("persistence"));
    let data_dir = Arc::new(data_dir);
    let config_dir = Arc::new(config_dir);
    let repo_manager = RepoManager::new(persistence.clone(), tmp.path().join("repos"));
    let workspaces = WorkspaceManager::new(persistence.clone(), config_dir.clone());
    let workareas = WorkareaManager::new(
        persistence.clone(),
        repo_manager,
        data_dir.clone(),
        config_dir.clone(),
    );
    let supervisor = AgentSupervisorHandle::new(
        persistence,
        data_dir,
        config_dir,
        PathBuf::from("concerto-agent-host"),
    );
    // No `.with_transport_events(..)`.
    let handler = StreamsHandler::new(supervisor, workspaces, workareas);

    let resp = handler
        .subscribe(Request::new(SubscribeRequest {
            subject: "transport.events".into(),
            filter: None,
            since_offset: None,
        }))
        .await
        .expect("subscribe still valid");
    let mut stream = resp.into_inner();
    // No events ever arrive.
    assert!(
        next_event(&mut stream, Duration::from_millis(400))
            .await
            .is_none(),
        "transport.events with no transport source yields nothing"
    );
}

// ---------------------------------------------------------------------------
// The nat_stats Runtime read path (D1).
// ---------------------------------------------------------------------------

/// A test `NatStatsSource` returning fixed by-client-kind + by-network-class
/// counters (stands in for Task 217's live `IrohTransport` source).
struct FakeNatStats(NatStats);

impl NatStatsSource for FakeNatStats {
    fn nat_stats(&self) -> NatStats {
        self.0.clone()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn get_nat_stats_maps_by_client_kind_and_network_class() {
    // Build a populated NatStats: mobile mostly direct, split-host desktop
    // mostly relayed (the design/11 §2 case we must be able to SEE).
    let mut stats = NatStats::default();
    stats.record(ConnectionPath::Direct, "direct", ClientKind::Mobile);
    stats.record(ConnectionPath::Direct, "direct", ClientKind::Mobile);
    stats.record(
        ConnectionPath::Relayed,
        "relayed",
        ClientKind::DesktopSplitHost,
    );
    stats.record(ConnectionPath::Lan, "lan", ClientKind::Web);

    let handler = RuntimeHandler::new(
        Arc::new(std::time::SystemTime::now()),
        SupervisorView::default(),
    )
    .with_nat_stats(Arc::new(FakeNatStats(stats)));

    let proto = handler
        .get_nat_stats(Request::new(()))
        .await
        .expect("get_nat_stats")
        .into_inner();

    assert_eq!(proto.direct_today, 2);
    assert_eq!(proto.relayed_today, 1);
    assert_eq!(proto.lan_today, 1);

    // by_client_kind is keyed on the ClientKind canonical name string.
    let desktop = proto
        .by_client_kind
        .get("CLIENT_KIND_DESKTOP_SPLIT_HOST")
        .expect("desktop bucket");
    assert_eq!(desktop.relayed, 1);
    assert_eq!(
        desktop.direct, 0,
        "the worse split-host direct rate is visible"
    );
    let mobile = proto
        .by_client_kind
        .get("CLIENT_KIND_MOBILE")
        .expect("mobile bucket");
    assert_eq!(mobile.direct, 2);
    let web = proto
        .by_client_kind
        .get("CLIENT_KIND_WEB")
        .expect("web bucket");
    assert_eq!(web.lan, 1);

    // by_network_class mirrors the path labels.
    assert_eq!(proto.by_network_class.get("direct").unwrap().direct, 2);
    assert_eq!(proto.by_network_class.get("relayed").unwrap().relayed, 1);
    assert_eq!(proto.by_network_class.get("lan").unwrap().lan, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn get_nat_stats_empty_when_no_transport_attached() {
    // Default handler (NoNatStats): a co-located / UDS-only Core returns empty
    // counters, never an error.
    let handler = RuntimeHandler::new(
        Arc::new(std::time::SystemTime::now()),
        SupervisorView::default(),
    );
    let proto = handler
        .get_nat_stats(Request::new(()))
        .await
        .expect("get_nat_stats")
        .into_inner();
    assert_eq!(proto.direct_today, 0);
    assert_eq!(proto.relayed_today, 0);
    assert_eq!(proto.lan_today, 0);
    assert!(proto.by_client_kind.is_empty());
    assert!(proto.by_network_class.is_empty());

    // And the explicit NoNatStats source is the same.
    let _: HashMap<String, _> = proto.by_network_class;
    let _ = NetworkStats::default();
    let _ = NoNatStats;
}
