//! Tier-2 end-to-end test for the Iroh transport wired into Core **boot**
//! (Task 217.5).
//!
//! **Double:** two Iroh endpoints on one host, relays disabled (direct
//! loopback), a real booted Core with the Iroh listener enabled
//! (`CONCERTO_ENABLE_IROH=1`) + keychain-isolated (`CONCERTO_KEYCHAIN_SERVICE`).
//! It drives the *real* boot path (`boot::start` → `build_iroh_transport` →
//! `serve_iroh` + the Core-side Noise-XX pairing responder over the `0x03`
//! channel) and proves, on one machine, in CI:
//!
//! 1. **Boot spawn** — booting with the toggle on brings up a dialable Iroh
//!    endpoint alongside the UDS server.
//! 2. **Pairing over the real `0x03` channel** — a second in-process Iroh
//!    endpoint opens the pairing channel, completes the Noise XX over the
//!    one-shot token, and receives a `SignedDeviceCert` minted by the booted
//!    `PairingCoordinator`.
//! 3. **Authenticated IROH RPC** — the paired device dials the API channel,
//!    presents its cert, and `GetServerCapabilities` reports
//!    `transport_kind == IROH` (the Task-210 cert path ran).
//! 4. **Revoke teardown** — `RevokeDevice` over that authenticated channel runs
//!    the live `IrohSessionCloser`, severing the device's open Iroh session.
//!
//! What this double does **NOT** cover (→ Phase-2 Tier-3 manual checklist): real
//! cross-machine split-host, real NAT/relay hole-punch, QUIC migration, or
//! throughput budgets. Those are physical/external and signed off at the phase
//! gate (`design/11 §10`).
//!
//! Note on the revoke proof: in a naturally-accepted Iroh session the transport
//! keys the session on the peer's **Iroh endpoint id** (`serve_conn`), while the
//! `SessionCloser` is handed the device's **cert fingerprint** — the
//! fingerprint↔endpoint-id binding is a 210/212 follow-up (see the task Handoff).
//! Here we register a fingerprint-keyed session on the live transport and prove
//! the real `RevokeDevice → DeviceManager → IrohSessionCloser →
//! IrohTransport::close_sessions_for_device` chain severs it.

// macOS-gated (not just `unix`): the booted Iroh path depends end-to-end on the
// OS keychain — the Core's Ed25519 identity (the cert issuer, Task 206) and its
// Noise static (Task 217.5) are both persisted there. The `keyring` backend only
// works on macOS in V1.0 (Linux Secret Service / Windows backends are a later
// port). On a keychain-less lane the issuer that *mints* the device cert and the
// one that *validates* it diverge (each `load_or_create` draws a fresh ephemeral
// key), so the authenticated RPC is rejected `auth.invalid_cert`. Same gate as
// `crates/keychain/tests/round_trip.rs` and the Task-218 keychain test; re-enable
// on Linux/Windows when their keychain backends land.
#![cfg(target_os = "macos")]

use std::sync::Arc;
use std::time::Duration;

use concerto_core::boot::{self, BootOutcome};
use concerto_core::runtime::RuntimeConfig;
use concerto_core::security::auth::{encode_cert_metadata, DEVICE_CERT_METADATA_KEY};
use concerto_identity::{device_id as derive_device_id, KeyPair, NoiseHandshake, NoiseStatic};
use concerto_proto::v1::devices_client::DevicesClient;
use concerto_proto::v1::runtime_client::RuntimeClient;
use concerto_proto::v1::{RevokeDeviceRequest, TransportKind};
use concerto_transport::api::write_channel_tag;
use concerto_transport::{
    connect_channel, direct_endpoint_addr, ChannelTag, ClientKind, DeviceId, IrohDuplex,
    IrohTransport, ALPN,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Length-framed write (4-byte BE length + bytes) — the `0x03`-channel framing
/// the Core's pairing responder locks (Task 217.5).
async fn write_frame(duplex: &mut IrohDuplex, bytes: &[u8]) {
    duplex
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await
        .expect("write len");
    duplex.write_all(bytes).await.expect("write body");
    duplex.flush().await.expect("flush");
}

async fn read_frame(duplex: &mut IrohDuplex) -> Vec<u8> {
    let mut len = [0u8; 4];
    duplex.read_exact(&mut len).await.expect("read len");
    let n = u32::from_be_bytes(len) as usize;
    let mut buf = vec![0u8; n];
    duplex.read_exact(&mut buf).await.expect("read body");
    buf
}

/// The device's `PairingRequest` frame layout the Core decodes:
/// `device_pubkey(32) || nonce(32) || signature(64) || device_name(utf8)`.
fn encode_pairing_request(
    device_pubkey: &[u8; 32],
    nonce: &[u8; 32],
    signature: &[u8; 64],
    device_name: &str,
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(device_pubkey);
    out.extend_from_slice(nonce);
    out.extend_from_slice(signature);
    out.extend_from_slice(device_name.as_bytes());
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn boot_pairs_over_iroh_then_authenticated_rpc_then_revoke_severs() {
    // --- Keychain isolation + Iroh toggle (KEYCHAIN-IN-CI hazard) ----------
    std::env::set_var(
        "CONCERTO_KEYCHAIN_SERVICE",
        format!("concerto-test-{}-irohboot", std::process::id()),
    );
    std::env::set_var("CONCERTO_ENABLE_IROH", "1");

    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().join("data");
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::create_dir_all(&config_dir).unwrap();

    let config = RuntimeConfig {
        data_dir,
        config_dir,
        shutdown_grace: Duration::from_secs(5),
    };

    let core = match boot::start(config).await.expect("boot::start") {
        BootOutcome::Started(c) => c,
        BootOutcome::AlreadyRunning { pid } => panic!("unexpected live instance pid={pid}"),
    };

    // --- The booted Core's live Iroh seam ---------------------------------
    let iroh = core
        .iroh()
        .expect("iroh listener enabled at boot (CONCERTO_ENABLE_IROH=1)");
    let server_transport: Arc<IrohTransport> = Arc::clone(&iroh.transport);
    let core_noise_pub = server_transport.core_noise_public();
    let server_addr = direct_endpoint_addr(&server_transport.endpoint())
        .await
        .expect("server iroh addr");

    // --- Arm a pairing (mints token + opens the 0x03 listener + accept task)
    let challenge = iroh
        .pairing_responder
        .start_pairing()
        .expect("start_pairing");
    let token = challenge.pairing_token;
    // The QR hint carries the live endpoint id so a client can discover it.
    assert_eq!(
        challenge.lan_endpoint,
        server_transport.endpoint_id().to_string(),
        "the pairing challenge carries the dialable Iroh endpoint id"
    );

    // --- Device endpoint (relay disabled) ---------------------------------
    let client_ep = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .relay_mode(iroh::RelayMode::Disabled)
        .bind()
        .await
        .expect("client endpoint");

    // --- Pair over the REAL 0x03 channel ----------------------------------
    let device_key = KeyPair::from_seed(&[0x42u8; 32]);
    let device_pubkey = device_key.verifying_key().to_bytes();
    let nonce = [0x24u8; 32];
    let signed_cert = {
        let conn = client_ep
            .connect(server_addr.clone(), ALPN)
            .await
            .expect("pair connect");
        let (send, recv) = conn.open_bi().await.expect("open pairing bidi");
        let duplex = IrohDuplex::new(send, recv);
        let mut duplex = write_channel_tag(duplex, ChannelTag::Pairing)
            .await
            .expect("write 0x03 tag");

        // Noise XX initiator over the one-shot token.
        let mut hs = NoiseHandshake::initiator(&token).expect("xx initiator");
        let m1 = hs.write_message(&[]).expect("m1");
        write_frame(&mut duplex, &m1).await;
        let m2 = read_frame(&mut duplex).await;
        hs.read_message(&m2).expect("read m2");
        let m3 = hs.write_message(&[]).expect("m3");
        write_frame(&mut duplex, &m3).await;
        let mut noise = hs.into_transport().expect("xx transport");

        // Sign `token || nonce || device_pubkey` and send the encrypted request.
        let mut payload = Vec::with_capacity(96);
        payload.extend_from_slice(&token);
        payload.extend_from_slice(&nonce);
        payload.extend_from_slice(&device_pubkey);
        let signature = device_key.sign(&payload).to_bytes();
        let req = encode_pairing_request(&device_pubkey, &nonce, &signature, "Iroh Boot Phone");
        let ct = noise.write_message(&req).expect("encrypt request");
        write_frame(&mut duplex, &ct).await;

        // Read the encrypted signed cert back.
        let reply_ct = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut duplex))
            .await
            .expect("cert reply did not stall");
        let signed_cert = noise.read_message(&reply_ct).expect("decrypt cert reply");
        assert!(
            signed_cert.len() > 1,
            "a refusal would be a single byte; got a real cert of {} bytes",
            signed_cert.len()
        );
        signed_cert
    };

    // --- Authenticated IROH RPC: GetServerCapabilities == IROH ------------
    let device_static = Arc::new(NoiseStatic::generate().expect("device noise static"));
    let channel = connect_channel(
        &client_ep,
        server_addr.clone(),
        device_static,
        core_noise_pub,
    )
    .await
    .expect("connect api channel");

    let cert_value: tonic::metadata::MetadataValue<_> =
        encode_cert_metadata(&signed_cert).parse().unwrap();
    #[allow(clippy::result_large_err)]
    let attach_cert = move |mut req: tonic::Request<()>| -> std::result::Result<
        tonic::Request<()>,
        tonic::Status,
    > {
        req.metadata_mut()
            .insert(DEVICE_CERT_METADATA_KEY, cert_value.clone());
        Ok(req)
    };

    let mut runtime_client = RuntimeClient::with_interceptor(channel.clone(), attach_cert.clone());
    let caps = tokio::time::timeout(
        Duration::from_secs(10),
        runtime_client.get_server_capabilities(()),
    )
    .await
    .expect("rpc did not stall")
    .expect("get_server_capabilities")
    .into_inner();
    assert_eq!(
        caps.transport_kind,
        TransportKind::Iroh as i32,
        "an authenticated request over the booted Iroh listener must report IROH"
    );

    // --- Revoke teardown ---------------------------------------------------
    // Register a fingerprint-keyed session on the live transport so the
    // SessionCloser (fingerprint→DeviceId) has a session to sever (see the
    // module note on the endpoint-id↔fingerprint binding follow-up). We use a
    // fresh device->server connection as the session's underlying connection.
    let fingerprint = derive_device_id(&device_pubkey);
    let session_device_id = DeviceId::from(fingerprint);
    let session_conn = client_ep
        .connect(server_addr, ALPN)
        .await
        .expect("session connect");
    server_transport.record_session_open(
        session_device_id.clone(),
        session_conn,
        ClientKind::DesktopSplitHost,
    );
    assert!(
        server_transport
            .session_paths()
            .iter()
            .any(|(id, _)| *id == session_device_id),
        "the fingerprint-keyed session is registered before revoke"
    );

    // Revoke over the authenticated Iroh channel → real DeviceManager →
    // IrohSessionCloser → IrohTransport::close_sessions_for_device.
    let mut devices_client = DevicesClient::with_interceptor(channel, attach_cert);
    let device_id_hex = hex::encode(fingerprint);
    tokio::time::timeout(
        Duration::from_secs(10),
        devices_client.revoke_device(RevokeDeviceRequest {
            device_id: device_id_hex,
        }),
    )
    .await
    .expect("revoke did not stall")
    .expect("revoke_device");

    assert!(
        !server_transport
            .session_paths()
            .iter()
            .any(|(id, _)| *id == session_device_id),
        "revoke must sever the device's Iroh session (SessionCloser ran)"
    );

    // --- Clean shutdown (no leaked endpoint) ------------------------------
    let token = core.shutdown_token();
    let join = tokio::spawn(async move { core.run_until_shutdown().await });
    token.cancel();
    let res = tokio::time::timeout(Duration::from_secs(10), join).await;
    assert!(res.is_ok(), "run_until_shutdown should return after cancel");
    res.unwrap().expect("join").expect("clean shutdown");
}
