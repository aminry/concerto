//! Tier-2 end-to-end test for the Iroh transport wired into the Core's gRPC
//! server (Task 212).
//!
//! Drives the **real** `serve_iroh` path: two Iroh endpoints on one host, relays
//! disabled (LAN-only), the Core's real `Runtime` service over the hand-rolled
//! adapter + Noise IK, with a real device cert presented under the
//! `concerto-device-cert` metadata header so the Task-210 auth path runs the
//! cert-validation branch (chosen off the `ConnTransport(TransportKind::Iroh)`
//! tag the Iroh listener injects). Asserts
//! `GetServerCapabilities.transport_kind == IROH` — proving the Tonic stack runs
//! unmodified over Iroh with the `IROH` tag flowing through to the handler, **no
//! per-transport handler branching**.
//!
//! This is the in-Rust twin of the `transport-loopback` smoke capability.
//!
//! Does **NOT** cover: real-NAT hole-punch, a real WAN relay, real migration —
//! those are Tier-3 (Phase-2 manual checklist).

#![cfg(unix)]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use concerto_core::api_server::{serve_iroh, CoreServiceSet};
use concerto_core::supervisor::SupervisorView;
use concerto_identity::{
    new_revoked_set, DeviceCertIssuer, KeyPair, LocalCoreIssuer, NoiseStatic, PairingRequest,
    SignedDeviceCert,
};
use concerto_proto::v1::runtime_client::RuntimeClient;
use concerto_proto::v1::TransportKind;
use concerto_transport::{connect_channel, direct_endpoint_addr, IrohTransport, TransportConfig};

/// The on-wire signed cert form `cert_bytes || signature`.
fn on_wire(signed: &SignedDeviceCert) -> Vec<u8> {
    let mut v = signed.cert_bytes.clone();
    v.extend_from_slice(&signed.signature);
    v
}

#[tokio::test(flavor = "multi_thread")]
async fn get_capabilities_over_iroh_reports_iroh_transport() {
    // --- Core identity + cert issuer (the cert path's trust root) ----------
    let core_key = KeyPair::from_seed(&[0x11u8; 32]);
    let core_pub = KeyPair::from_seed(&[0x11u8; 32]).verifying_key();
    let issuer: Arc<dyn DeviceCertIssuer> =
        Arc::new(LocalCoreIssuer::new(core_key, core_pub, new_revoked_set()));

    // --- Issue a device cert (the device the client presents) --------------
    let device = KeyPair::from_seed(&[0x22u8; 32]);
    let signed = issuer
        .issue(PairingRequest {
            device_pubkey: device.verifying_key().to_bytes(),
            device_name: "iroh-test-device".into(),
        })
        .await
        .expect("issue cert");
    let cert_on_wire = on_wire(&signed);

    // --- The Core's Noise static (responder); device pre-loads its pub ------
    let core_noise_seed = [0x33u8; 32];
    let core_noise_pub = NoiseStatic::from_private(core_noise_seed).unwrap().public();

    // --- Start the Iroh transport (LAN-only → relays off, loopback path) ----
    let transport = Arc::new(
        IrohTransport::start(
            TransportConfig {
                relay_url: None,
                disable_remote: true,
                direct_addr: None,
            },
            core_noise_seed,
        )
        .await
        .expect("start transport"),
    );

    // --- The Core's real Runtime service over serve_iroh -------------------
    let started_at = Arc::new(SystemTime::now());
    let mut services = CoreServiceSet::runtime_only(started_at, SupervisorView::default());
    services.auth_issuer = Some(issuer);

    {
        let transport = transport.clone();
        tokio::spawn(async move {
            let _ = serve_iroh(transport, services).await;
        });
    }

    // --- Client endpoint (relay disabled) + dial ---------------------------
    let server_addr = direct_endpoint_addr(&transport.endpoint())
        .await
        .expect("server addr");
    let client_ep = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
        .relay_mode(iroh::RelayMode::Disabled)
        .bind()
        .await
        .expect("client endpoint");
    let device_static = Arc::new(NoiseStatic::generate().unwrap());

    let channel = connect_channel(&client_ep, server_addr, device_static, core_noise_pub)
        .await
        .expect("connect channel");

    // Attach the device cert under the frozen metadata key on every request
    // (the Iroh auth path validates it).
    let cert_value: tonic::metadata::MetadataValue<_> =
        concerto_core::security::auth::encode_cert_metadata(&cert_on_wire)
            .parse()
            .unwrap();
    // The tonic `Interceptor` trait fixes the `Err` type to `tonic::Status`
    // (large) — the same fixed shape the api_server's interceptors carry.
    #[allow(clippy::result_large_err)]
    let attach_cert = move |mut req: tonic::Request<()>| -> std::result::Result<
        tonic::Request<()>,
        tonic::Status,
    > {
        req.metadata_mut().insert(
            concerto_core::security::auth::DEVICE_CERT_METADATA_KEY,
            cert_value.clone(),
        );
        Ok(req)
    };
    let mut client = RuntimeClient::with_interceptor(channel, attach_cert);

    // --- The assertion: GetServerCapabilities reports IROH -----------------
    let caps = tokio::time::timeout(Duration::from_secs(10), client.get_server_capabilities(()))
        .await
        .expect("rpc did not stall (acceptor priming)")
        .expect("get_server_capabilities")
        .into_inner();

    assert_eq!(
        caps.transport_kind,
        TransportKind::Iroh as i32,
        "the handler must report IROH for a request that arrived on the Iroh listener"
    );
    assert_eq!(caps.schema_version, "concerto.v1");

    transport.stop();
}
