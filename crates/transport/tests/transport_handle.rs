//! Tier-1 contract tests for the FROZEN [`TransportHandle`] façade
//! (`design/11 §5.1`, Task 217).
//!
//! Each of the nine `design/11 §5.1` methods is exercised against the **in-process
//! endpoint** — the handle is a thin wrapper over Task 212's [`IrohTransport`],
//! unit-testable with NO real NAT and NO second machine:
//!
//! - `start` → `stop` lifecycle is idempotent + clean; a double-`start` errors;
//!   delegating methods error cleanly before `start` / after `stop`;
//! - `listen_pairing` → `close_pairing` opens then releases the pairing channel;
//! - `switch_relay` updates what `current_relay` returns (and is refused under
//!   `disable_remote`);
//! - `nat_stats` returns the live counters;
//! - `close_sessions_for_device` removes the targeted device's session (asserted
//!   against a real in-process loopback session, observed via telemetry);
//! - `send_wakeup_hint` routes to the device's push-hint channel (and is a clean
//!   enqueue for any id — the transport routes opaque ID-only bytes).
//!
//! The session-bearing assertions use the Tier-2 **loopback double** (two Iroh
//! endpoints on one host, relays disabled, forced onto the direct loopback path —
//! the same model as `tests/migration_telemetry.rs`), so they are gated behind the
//! `dev-relay` feature like the rest of the loopback suite. The real cross-device
//! behaviours (a stolen device severed over the wire, a real pairing handshake, a
//! real push wakeup) are downstream tasks' Tier-2 (209/207/P5) and the Phase-2
//! Tier-3 checklist.

use std::sync::Arc;

use concerto_transport::{
    ApiDispatcher, DeviceId, NoiseDuplex, TransportConfig, TransportError, TransportHandle,
    WakeupPayload,
};

// ---------------------------------------------------------------------------
// A trivial dispatcher (the handle needs an `ApiDispatcher`; these tests do not
// drive gRPC through it, so it serves nothing — the loopback gRPC round-trip is
// already proven in `tests/loopback.rs` / `tests/migration_telemetry.rs`).
// ---------------------------------------------------------------------------

struct NoopDispatcher;

impl ApiDispatcher for NoopDispatcher {
    fn serve_connection(
        &self,
        _io: NoiseDuplex,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), TransportError>> + Send>>
    {
        Box::pin(async { Ok(()) })
    }
}

fn handle() -> TransportHandle<NoopDispatcher> {
    TransportHandle::new([3u8; 32], Arc::new(NoopDispatcher))
}

fn lan_only_cfg() -> TransportConfig {
    TransportConfig {
        relay_url: None,
        disable_remote: false,
        direct_addr: None,
    }
}

fn assert_lifecycle_err(err: TransportError) {
    assert!(
        matches!(err, TransportError::Lifecycle(_)),
        "expected a Lifecycle error, got {err:?}"
    );
}

// ===========================================================================
// Lifecycle: start / stop (no network beyond binding a loopback endpoint).
// ===========================================================================

#[tokio::test]
async fn start_then_stop_is_clean_and_idempotent() {
    let h = handle();

    // Methods error cleanly before start.
    assert_lifecycle_err(h.current_relay().await.unwrap_err());
    assert_lifecycle_err(h.nat_stats().await.unwrap_err());

    h.start(lan_only_cfg()).await.expect("first start");

    // A second start while running is a clean Lifecycle error (never a double
    // endpoint bind).
    assert_lifecycle_err(h.start(lan_only_cfg()).await.unwrap_err());

    h.stop().await.expect("stop");
    // stop is idempotent.
    h.stop().await.expect("second stop is a no-op");

    // After stop, delegating methods error cleanly again.
    assert_lifecycle_err(h.current_relay().await.unwrap_err());

    // And the handle can be restarted (start rebuilds the endpoint).
    h.start(lan_only_cfg()).await.expect("restart");
    h.current_relay()
        .await
        .expect("current_relay after restart");
    h.stop().await.expect("final stop");
}

// ===========================================================================
// current_relay / switch_relay.
// ===========================================================================

#[tokio::test]
async fn switch_relay_is_reflected_in_current_relay() {
    let h = handle();
    h.start(lan_only_cfg()).await.expect("start");

    let before = h.current_relay().await.expect("current_relay");
    assert!(before.url.is_none(), "no relay configured initially");
    assert!(!before.remote_disabled);

    let url = url::Url::parse("https://relay.example.com").expect("url");
    h.switch_relay(url.clone()).await.expect("switch_relay");

    let after = h.current_relay().await.expect("current_relay");
    assert_eq!(
        after.url.as_deref(),
        Some("https://relay.example.com/"),
        "current_relay reflects the switched URL"
    );

    h.stop().await.expect("stop");
}

#[tokio::test]
async fn switch_relay_refused_under_disable_remote() {
    let h = handle();
    h.start(TransportConfig {
        relay_url: None,
        disable_remote: true,
        direct_addr: None,
    })
    .await
    .expect("start lan-only");

    let relay = h.current_relay().await.expect("current_relay");
    assert!(relay.remote_disabled, "remote disabled is surfaced");

    let url = url::Url::parse("https://relay.example.com").expect("url");
    let err = h.switch_relay(url).await.unwrap_err();
    assert!(
        matches!(err, TransportError::RemoteDisabled(_)),
        "switch_relay refused under disable_remote, got {err:?}"
    );

    h.stop().await.expect("stop");
}

// ===========================================================================
// listen_pairing / close_pairing.
// ===========================================================================

#[tokio::test]
async fn listen_pairing_then_close_pairing() {
    let h = handle();
    h.start(lan_only_cfg()).await.expect("start");

    let token_hash = [9u8; 32];
    let listener = h.listen_pairing(token_hash).await.expect("listen_pairing");
    assert_eq!(
        listener.token_hash(),
        token_hash,
        "the listener is bound to the supplied token hash"
    );

    // Re-opening replaces the prior listener (a fresh token hash).
    let listener2 = h.listen_pairing([1u8; 32]).await.expect("re-listen");
    assert_eq!(listener2.token_hash(), [1u8; 32]);

    h.close_pairing().await.expect("close_pairing");
    // close is idempotent.
    h.close_pairing().await.expect("second close is a no-op");

    h.stop().await.expect("stop");
}

#[tokio::test]
async fn pairing_before_start_errors_cleanly() {
    let h = handle();
    // `PairingListener` is not `Debug`, so match the error arm directly rather
    // than via `unwrap_err`.
    match h.listen_pairing([0u8; 32]).await {
        Ok(_) => panic!("listen_pairing must error before start"),
        Err(e) => assert_lifecycle_err(e),
    }
    assert_lifecycle_err(h.close_pairing().await.unwrap_err());
}

// ===========================================================================
// nat_stats.
// ===========================================================================

#[tokio::test]
async fn nat_stats_reads_live_counters() {
    let h = handle();
    h.start(lan_only_cfg()).await.expect("start");

    let stats = h.nat_stats().await.expect("nat_stats");
    // A freshly-started transport with no sessions reports zero everywhere.
    assert_eq!(stats.direct_today, 0);
    assert_eq!(stats.relayed_today, 0);
    assert_eq!(stats.lan_today, 0);
    assert_eq!(stats.direct_percent(), 0);
    assert!(stats.by_client_kind.is_empty());

    h.stop().await.expect("stop");
}

// ===========================================================================
// send_wakeup_hint: routes opaque ID-only bytes to the push-hint channel.
// ===========================================================================

#[tokio::test]
async fn send_wakeup_hint_routes_to_push_hint_channel() {
    let h = handle();
    h.start(lan_only_cfg()).await.expect("start");

    // The P5 push backend drains the push-hint channel (companion accessor).
    let mut rx = h
        .take_wakeup_receiver()
        .await
        .expect("take_wakeup_receiver")
        .expect("receiver present (not yet taken)");

    let device = DeviceId("device-abc".into());
    let payload = WakeupPayload::new(b"opaque-id-only".to_vec());
    h.send_wakeup_hint(device.clone(), payload.clone())
        .await
        .expect("send_wakeup_hint enqueues");

    let hint = rx.recv().await.expect("the hint is routed to the channel");
    assert_eq!(hint.device_id, device, "routed to the right device");
    assert_eq!(
        hint.payload, payload.bytes,
        "the opaque ID-only bytes are carried unchanged"
    );

    h.stop().await.expect("stop");
}

#[tokio::test]
async fn send_wakeup_hint_before_start_errors_cleanly() {
    let h = handle();
    let err = h
        .send_wakeup_hint(DeviceId("x".into()), WakeupPayload::default())
        .await
        .unwrap_err();
    assert_lifecycle_err(err);
}

// ===========================================================================
// close_sessions_for_device: removes the targeted device's session.
//
// Driven against a REAL in-process loopback session (the `dev-relay` double):
// a client connects over the direct loopback path, establishing a server-side
// `ActiveSession`; the handle's `close_sessions_for_device` then severs it, and
// the serve loop's true-drop path emits `session_closed` (observed via the
// companion telemetry subscription).
// ===========================================================================

#[cfg(feature = "dev-relay")]
mod with_session {
    use super::*;
    use std::pin::Pin;
    use std::time::Duration;

    use concerto_transport::{
        connect_channel, direct_endpoint_addr, TransportTelemetry, MAX_MESSAGE_SIZE,
    };
    use tonic::transport::Server;
    use tonic::{Request, Response, Status};

    pub mod pb {
        tonic::include_proto!("concerto.transport.loopback.v1");
    }

    use pb::loopback_client::LoopbackClient;
    use pb::loopback_server::{Loopback, LoopbackServer};
    use pb::{EchoReply, EchoRequest};

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
            Ok(Response::new(
                Box::pin(futures::stream::empty()) as Self::FirehoseStream
            ))
        }
    }

    /// A real dispatcher that serves the trivial Loopback service so the client
    /// can establish (and keep) a session over the adapter.
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

    fn drain(
        rx: &mut tokio::sync::broadcast::Receiver<TransportTelemetry>,
    ) -> Vec<TransportTelemetry> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }

    /// A real loopback session brought up against the handle's endpoint is
    /// severed by `close_sessions_for_device`: the targeted device's session is
    /// removed and the serve loop's true-drop path emits `session_closed` (the
    /// Task-209 `SessionCloser` sever, observed via the companion telemetry +
    /// `nat_stats` seams). Driven on loopback — the real over-the-wire teardown is
    /// Tier-3 (the Phase-2 checklist's "revoke a device" line).
    #[tokio::test]
    async fn close_sessions_for_device_severs_the_targeted_session() {
        let core_seed = [11u8; 32];
        let core_noise_pub = concerto_identity::NoiseStatic::from_private(core_seed)
            .unwrap()
            .public();

        let h: TransportHandle<LoopbackDispatcher> =
            TransportHandle::new(core_seed, Arc::new(LoopbackDispatcher));
        h.start(TransportConfig {
            relay_url: None,
            disable_remote: true,
            direct_addr: None,
        })
        .await
        .expect("start");

        // Observe lifecycle telemetry via the companion subscription.
        let mut telem = h.subscribe_telemetry().await.expect("subscribe_telemetry");

        // Dial the handle's endpoint over the direct loopback path.
        let server_endpoint = h.endpoint().await.expect("endpoint");
        let server_addr = direct_endpoint_addr(&server_endpoint)
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
        let mut client = LoopbackClient::new(channel)
            .max_decoding_message_size(MAX_MESSAGE_SIZE)
            .max_encoding_message_size(MAX_MESSAGE_SIZE);

        // Establish the server-side session.
        client
            .echo(Request::new(EchoRequest {
                payload: b"open".to_vec(),
            }))
            .await
            .expect("echo");
        tokio::time::sleep(Duration::from_millis(200)).await;

        // One session is live; capture its device id from the session_opened event.
        let opened = drain(&mut telem);
        let device_id = opened
            .iter()
            .find_map(|e| match e {
                TransportTelemetry::SessionOpened { device_id, .. } => Some(device_id.clone()),
                _ => None,
            })
            .expect("session_opened emitted on establishment");
        let stats = h.nat_stats().await.expect("nat_stats");
        assert_eq!(
            stats.direct_today + stats.relayed_today + stats.lan_today,
            1,
            "exactly one session counted before the sever"
        );

        // Sever it through the FROZEN façade method. This removes the session
        // from the registry AND closes the underlying Iroh connection, so the
        // device's open streams are torn down (the < 1 s revocation sever,
        // `design/12 §7.3`).
        let _ = drain(&mut telem); // clear the open events.
        h.close_sessions_for_device(device_id.clone())
            .await
            .expect("close_sessions_for_device");

        // The real observable of the sever: the client's connection is now closed,
        // so a subsequent RPC over it fails (its streams were torn down). The
        // session was removed from the server registry synchronously inside the
        // call; the connection close propagates to the client.
        let mut severed = false;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if client
                .echo(Request::new(EchoRequest {
                    payload: b"after-sever".to_vec(),
                }))
                .await
                .is_err()
            {
                severed = true;
                break;
            }
        }
        assert!(
            severed,
            "after close_sessions_for_device the device's streams are torn down (RPC fails)"
        );

        // Severing an unknown device is a clean no-op success (idempotent).
        h.close_sessions_for_device(DeviceId("ghost".into()))
            .await
            .expect("close on unknown device is a clean no-op");

        h.stop().await.expect("stop");
    }
}
