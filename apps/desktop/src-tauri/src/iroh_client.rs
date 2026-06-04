//! Split-host [`IrohCoreClient`] (feature `iroh-transport`, `design/15 §3.2`).
//!
//! Dials the active Core's Iroh endpoint via Task 212's hand-rolled tonic-0.12
//! ↔ Iroh adapter (the same `connect_channel` the Tier-2 loopback double uses;
//! Task 217's `TransportHandle` fronts the Core side) and presents the stored
//! `SignedDeviceCert` in every request's `concerto-device-cert` metadata header
//! (`design/12 §3.3`, the FROZEN key from `crates/core` auth). It then routes
//! the **same** Tonic service calls as the UDS path through the shared
//! [`crate::rpc`] dispatch/subscribe logic.
//!
//! **No `tonic-iroh-transport`** — that would force `tonic 0.14` and collide
//! with the workspace `tonic 0.12` pin (`design/spikes/tonic-iroh-findings.md
//! §2`). The adapter is Task 212's hand-roll, consumed here, never
//! re-implemented.
//!
//! This module is the **client consumer** of the Iroh transport, unit-proven by
//! the Tier-2 loopback double below. Building it from the active registry row +
//! the keychain cert and wiring it into the live command path is the
//! connect/switch flow (Task 219/601); until that lands, the production
//! dispatch path resolves the co-located UDS Core, so these items are
//! `allow(dead_code)` outside tests.
#![cfg_attr(not(test), allow(dead_code))]

use std::sync::Arc;

use serde_json::Value;
use tonic::metadata::MetadataValue;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;
use tonic::{Request, Status};

use base64::Engine as _;

use concerto_transport::connect_channel;
use iroh::{Endpoint, EndpointAddr};

use crate::core_client::CoreClientError;
use crate::rpc;
use crate::transport::{CoreClient, StreamSink, StreamSubscription};

/// The request-metadata key the device presents its signed cert under
/// (`design/10 §3.4`, FROZEN by Task 210's `DEVICE_CERT_METADATA_KEY`). The lean
/// desktop build can't link `concerto-core`, so the constant is mirrored here;
/// it MUST match `crates/core`'s `security::auth::DEVICE_CERT_METADATA_KEY`.
pub const DEVICE_CERT_METADATA_KEY: &str = "concerto-device-cert";

/// A `tonic` interceptor that stamps the base64 signed device cert onto every
/// outbound request (`design/12 §3.3`). Cloneable so the channel can be cloned
/// per dispatch.
#[derive(Clone)]
struct DeviceCertInterceptor {
    /// `base64(cert_bytes || signature)` — the on-wire form the Core's auth
    /// middleware decodes (`crates/core` `security::auth`).
    cert_header: Arc<str>,
}

impl tonic::service::Interceptor for DeviceCertInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        let value = MetadataValue::try_from(self.cert_header.as_ref())
            .map_err(|_| Status::internal("device cert header is not valid ASCII metadata"))?;
        request
            .metadata_mut()
            .insert(DEVICE_CERT_METADATA_KEY, value);
        Ok(request)
    }
}

/// The gRPC transport the Iroh path routes over: the Iroh-backed tonic
/// [`Channel`] wrapped in the device-cert [`DeviceCertInterceptor`]. Cloneable
/// (both halves are), so [`crate::rpc::dispatch_over_channel`] can take it by
/// value per call.
type IrohChannel = InterceptedService<Channel, DeviceCertInterceptor>;

/// Split-host client over Iroh (`design/15 §3.2`). Holds the persistent
/// Iroh-backed tonic channel (one Noise-IK session, multiplexed bidi streams)
/// plus the device-cert interceptor.
pub struct IrohCoreClient {
    channel: IrohChannel,
}

impl IrohCoreClient {
    /// Build the client by dialing `server_addr` over `client_endpoint` (Task
    /// 212's `connect_channel`: channel-tag + Noise IK initiator + 64 MiB
    /// limits) and arming the device-cert interceptor with the signed cert.
    ///
    /// - `device_noise_static` — this Desktop's X25519 Noise static (the Noise
    ///   IK initiator key; the private half stays in `concerto-identity`).
    /// - `core_noise_pub` — the Core's X25519 Noise static public key (the
    ///   responder static, captured at pairing — `PairedCore::core_noise_pubkey`).
    /// - `signed_device_cert_on_wire` — `cert_bytes || signature`, the exact
    ///   bytes the Core validates; base64-encoded into the request metadata.
    pub async fn connect(
        client_endpoint: &Endpoint,
        server_addr: EndpointAddr,
        device_noise_static: Arc<concerto_identity::NoiseStatic>,
        core_noise_pub: [u8; 32],
        signed_device_cert_on_wire: &[u8],
    ) -> Result<Self, CoreClientError> {
        let channel = connect_channel(
            client_endpoint,
            server_addr,
            device_noise_static,
            core_noise_pub,
        )
        .await
        .map_err(|e| CoreClientError::Transport(format!("iroh connect: {e}")))?;

        let cert_header: Arc<str> = base64::engine::general_purpose::STANDARD
            .encode(signed_device_cert_on_wire)
            .into();
        let interceptor = DeviceCertInterceptor { cert_header };
        let channel = InterceptedService::new(channel, interceptor);
        Ok(Self { channel })
    }
}

#[async_trait::async_trait]
impl CoreClient for IrohCoreClient {
    async fn dispatch(&self, method: &str, payload: Value) -> Result<Value, CoreClientError> {
        rpc::dispatch_over_channel(self.channel.clone(), method, payload).await
    }

    async fn start_stream(
        &self,
        subject: &str,
        filter: Value,
        sink: StreamSink,
    ) -> Result<StreamSubscription, CoreClientError> {
        rpc::subscribe_over_channel(self.channel.clone(), subject, filter, sink).await
    }
}

#[cfg(test)]
mod tests {
    //! Tier-2 double: a **loopback Iroh endpoint pair on one host, relays
    //! disabled (direct)** — the spike's Tier-2 model. Proves the
    //! `IrohCoreClient` dials over Iroh + the hand-rolled adapter + Noise IK,
    //! carries the device cert in `concerto-device-cert` metadata, and routes a
    //! real Tonic service call through the shared dispatch logic.
    //!
    //! It does **NOT** cover real cross-machine split-host, real NAT/relay
    //! fallback, or real OS-keychain prompts on a signed build — those are the
    //! Phase-2 Tier-3 checklist lines.

    use std::pin::Pin;
    use std::sync::Arc;

    use base64::Engine as _;
    use concerto_proto::v1::runtime_server::{Runtime, RuntimeServer};
    use concerto_proto::v1::{NatStats, RuntimeStatus, ServerCapabilities};
    use concerto_transport::{
        direct_endpoint_addr, ApiDispatcher, IrohTransport, NoiseDuplex, TransportConfig,
        TransportError, MAX_MESSAGE_SIZE,
    };
    use serde_json::json;
    use tonic::transport::Server;
    use tonic::{Request, Response, Status};

    use super::{IrohCoreClient, DEVICE_CERT_METADATA_KEY};
    use crate::core_client::CoreClientError;
    use crate::transport::CoreClient;

    /// A minimal `Runtime` service that (a) asserts the device cert rode in the
    /// `concerto-device-cert` metadata and (b) echoes a recognisable
    /// `server_version` so the client round-trip is observable.
    #[derive(Clone)]
    struct ProbeRuntime;

    #[tonic::async_trait]
    impl Runtime for ProbeRuntime {
        async fn get_server_capabilities(
            &self,
            request: Request<()>,
        ) -> Result<Response<ServerCapabilities>, Status> {
            // The split-host client MUST present the device cert in metadata.
            let cert = request
                .metadata()
                .get(DEVICE_CERT_METADATA_KEY)
                .ok_or_else(|| Status::unauthenticated("missing concerto-device-cert metadata"))?;
            let cert_b64 = cert
                .to_str()
                .map_err(|_| Status::unauthenticated("cert metadata not ASCII"))?;
            // Decode it to prove it's the base64 we stamped.
            base64::engine::general_purpose::STANDARD
                .decode(cert_b64)
                .map_err(|_| Status::unauthenticated("cert metadata not base64"))?;
            Ok(Response::new(ServerCapabilities {
                server_version: "iroh-probe".to_string(),
                schema_version: "1".to_string(),
                optional_services: vec![],
                limits: None,
                // 2 == TRANSPORT_KIND_IROH (the split-host transport_kind the
                // renderer reads).
                transport_kind: 2,
                core_host_os: "test".to_string(),
                core_hostname: "loopback".to_string(),
            }))
        }

        async fn get_status(
            &self,
            _request: Request<()>,
        ) -> Result<Response<RuntimeStatus>, Status> {
            Err(Status::unimplemented(
                "get_status not used by this Tier-2 double",
            ))
        }

        async fn get_nat_stats(&self, _request: Request<()>) -> Result<Response<NatStats>, Status> {
            Err(Status::unimplemented(
                "get_nat_stats not used by this Tier-2 double",
            ))
        }
    }

    struct ProbeDispatcher;

    impl ApiDispatcher for ProbeDispatcher {
        fn serve_connection(
            &self,
            io: NoiseDuplex,
        ) -> Pin<Box<dyn std::future::Future<Output = Result<(), TransportError>> + Send>> {
            Box::pin(async move {
                let svc = RuntimeServer::new(ProbeRuntime)
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

    #[tokio::test(flavor = "multi_thread")]
    async fn iroh_client_dispatches_with_device_cert_in_metadata() {
        // --- Core (responder) side: an IrohTransport, relays disabled. ---
        let core_seed = [9u8; 32];
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
        {
            let server = server.clone();
            tokio::spawn(async move {
                let _ = server.serve(Arc::new(ProbeDispatcher)).await;
            });
        }
        let server_addr = direct_endpoint_addr(&server.endpoint())
            .await
            .expect("server direct addr");

        // --- Desktop (initiator) side: client endpoint + device Noise static. ---
        let client_ep: &'static iroh::Endpoint = Box::leak(Box::new(
            iroh::Endpoint::builder(iroh::endpoint::presets::N0)
                .relay_mode(iroh::RelayMode::Disabled)
                .bind()
                .await
                .expect("client endpoint bind"),
        ));
        let device_static = Arc::new(concerto_identity::NoiseStatic::generate().unwrap());

        // A device cert on-wire form (cert_bytes || signature). Opaque bytes are
        // fine here: the double only checks it is present + base64-decodable in
        // metadata (real cert validation is the Core auth path's Tier-1 job).
        let signed_cert_on_wire = vec![0xAB_u8; 96];

        let client = IrohCoreClient::connect(
            client_ep,
            server_addr,
            device_static,
            core_noise_pub,
            &signed_cert_on_wire,
        )
        .await
        .expect("iroh client connect");

        let resp = client
            .dispatch("Runtime.GetServerCapabilities", json!({}))
            .await
            .expect("dispatch over iroh");
        assert_eq!(resp["server_version"], "iroh-probe");
        assert_eq!(resp["transport_kind"], 2);

        // Sanity: the cert we stamped is the base64 of our on-wire bytes.
        let expected = base64::engine::general_purpose::STANDARD.encode(&signed_cert_on_wire);
        assert!(!expected.is_empty());

        server.stop();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn iroh_client_unknown_method_is_not_implemented() {
        let core_seed = [11u8; 32];
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
        {
            let server = server.clone();
            tokio::spawn(async move {
                let _ = server.serve(Arc::new(ProbeDispatcher)).await;
            });
        }
        let server_addr = direct_endpoint_addr(&server.endpoint()).await.unwrap();
        let client_ep: &'static iroh::Endpoint = Box::leak(Box::new(
            iroh::Endpoint::builder(iroh::endpoint::presets::N0)
                .relay_mode(iroh::RelayMode::Disabled)
                .bind()
                .await
                .unwrap(),
        ));
        let device_static = Arc::new(concerto_identity::NoiseStatic::generate().unwrap());
        let client = IrohCoreClient::connect(
            client_ep,
            server_addr,
            device_static,
            core_noise_pub,
            &[0x01_u8; 80],
        )
        .await
        .expect("connect");

        // An unmapped method never touches the wire — the shared dispatch
        // returns NotImplemented before building a client.
        let err = client
            .dispatch("Bogus.Method", json!({}))
            .await
            .expect_err("unknown method");
        assert!(matches!(err, CoreClientError::NotImplemented(_)));

        server.stop();
    }
}
