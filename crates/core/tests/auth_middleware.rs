//! Tier-1 tests for Task 210 — the auth middleware: the device-cert tower
//! interceptor (Iroh/WSS path) + the UDS peer-uid fast path, both landing in the
//! IDENTICAL handler surface with a uniform request-scoped `DeviceContext`.
//!
//! **Test doubles (Tier 1):**
//! - the cert path is driven by an **injected `Arc<dyn DeviceCertIssuer>`** — a
//!   real `LocalCoreIssuer` for the happy path (so the cert genuinely round-trips
//!   sign→validate) and a canned-`Err` stub for the failure variants — plus an
//!   **injected `ConnTransport(Iroh)` tag** so the cert path runs WITHOUT a live
//!   Iroh listener (the Task-201 seam pattern; the real Iroh listener is Task
//!   212);
//! - the UDS peer-uid path is exercised over a **real Unix socket** so
//!   `UdsConnectInfo`/`peer_cred` is genuinely present: the live test process
//!   connecting to its own socket is a same-uid peer (→ accepted), and a second
//!   server whose interceptor pins a non-owner Core uid rejects the very same
//!   connection (→ `UNAUTHENTICATED`).
//!
//! It also proves the **Task-209 startup-mirror gap is closed**: a device
//! revoked on disk, after a simulated "restart" (a FRESH empty revoked set
//! rebuilt only from the `devices` table via `mirror_revoked_devices`), is still
//! rejected by `validate` as `Revoked`.
//!
//! What this does **NOT** cover (→ Phase-2 Tier-3 manual checklist): a **real**
//! Iroh connection presenting a real cert over the wire (Task 212), the full
//! split-host auth round-trip (Task 220), and Windows named-pipe peer
//! attestation (a documented V1.0 gated gap).

#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use concerto_core::security::auth::{
    encode_cert_metadata, mirror_revoked_devices, AuthInterceptor, DEVICE_CERT_METADATA_KEY,
    LOCAL_UDS_DEVICE_ID,
};
use concerto_identity::{
    device_id, new_revoked_set, DeviceCertIssuer, IdentityError, KeyPair, LocalCoreIssuer,
    PairingRequest, PublicKey, RevokedSet, SignedDeviceCert,
};
use concerto_persist::{Persistence, PersistenceConfig};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Fixtures.
// ---------------------------------------------------------------------------

async fn open_persistence() -> (TempDir, Arc<Persistence>) {
    let tmp = TempDir::new().expect("tempdir");
    let cfg = PersistenceConfig {
        db_path: tmp.path().join("concerto.db"),
        max_readers: 2,
    };
    let persistence = Arc::new(Persistence::open(cfg).await.expect("open persistence"));
    (tmp, persistence)
}

fn core_issuer(revoked: RevokedSet) -> (Arc<dyn DeviceCertIssuer>, PublicKey) {
    let core_pub = KeyPair::from_seed(&[0x11u8; 32]).verifying_key();
    let issuer = LocalCoreIssuer::new(KeyPair::from_seed(&[0x11u8; 32]), core_pub, revoked);
    (Arc::new(issuer), core_pub)
}

/// The on-wire signed cert form `cert_bytes || signature` (exactly what
/// `complete_pairing` returns and the device presents under the metadata key).
fn on_wire(signed: &SignedDeviceCert) -> Vec<u8> {
    let mut v = signed.cert_bytes.clone();
    v.extend_from_slice(&signed.signature);
    v
}

async fn issue_cert(issuer: &Arc<dyn DeviceCertIssuer>, device_seed: u8, name: &str) -> Vec<u8> {
    let device = KeyPair::from_seed(&[device_seed; 32]);
    let req = PairingRequest {
        device_pubkey: device.verifying_key().to_bytes(),
        device_name: name.to_string(),
    };
    on_wire(&issuer.issue(req).await.expect("issue cert"))
}

/// Build an Iroh-tagged request carrying the base64 cert under the frozen key.
fn iroh_request_with_cert(raw_cert: &[u8]) -> tonic::Request<()> {
    let mut req = tonic::Request::new(());
    req.extensions_mut()
        .insert(concerto_core::conn_transport::ConnTransport(
            concerto_proto::v1::TransportKind::Iroh,
        ));
    req.metadata_mut().insert(
        DEVICE_CERT_METADATA_KEY,
        encode_cert_metadata(raw_cert).parse().unwrap(),
    );
    req
}

fn concerto_code(status: &tonic::Status) -> Option<String> {
    use prost::Message;
    if status.details().is_empty() {
        return None;
    }
    concerto_proto::v1::ConcertoError::decode(status.details())
        .ok()
        .map(|e| e.code)
}

// ---------------------------------------------------------------------------
// Cert path (Iroh) — valid round-trip through a real LocalCoreIssuer.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cert_path_valid_real_issuer_injects_admin_context() {
    let revoked = new_revoked_set();
    let (issuer, _pub) = core_issuer(revoked);
    let interceptor = AuthInterceptor::new(Some(Arc::clone(&issuer)));

    let raw = issue_cert(&issuer, 0x07, "Real Phone").await;
    let out = interceptor
        .authenticate(iroh_request_with_cert(&raw))
        .expect("a freshly-issued cert authenticates");

    let ctx = concerto_core::security::auth::device_context(&out)
        .expect("DeviceContext injected on the cert path");
    assert_eq!(
        ctx.device_id,
        device_id(&KeyPair::from_seed(&[0x07u8; 32]).verifying_key().to_bytes())
    );
    assert_eq!(ctx.device_name, "Real Phone");
    assert_eq!(ctx.capabilities, vec!["admin".to_string()]);
    // Identical shape to the UDS path's pseudo-cert: same `capabilities` field.
    assert_ne!(
        ctx.device_id, LOCAL_UDS_DEVICE_ID,
        "a real cert never carries the local-uds sentinel"
    );
}

// ---------------------------------------------------------------------------
// Cert path (Iroh) — failure variants map to the FROZEN auth statuses.
// ---------------------------------------------------------------------------

/// Canned-`Err` issuer stub for the failure-variant matrix.
struct ErrIssuer(IdentityError);

#[async_trait::async_trait]
impl DeviceCertIssuer for ErrIssuer {
    async fn issue(&self, _req: PairingRequest) -> concerto_identity::Result<SignedDeviceCert> {
        unreachable!("issue not exercised")
    }
    fn validate(&self, _raw: &[u8]) -> concerto_identity::Result<concerto_identity::DeviceContext> {
        Err(clone_identity_err(&self.0))
    }
    fn supported_capabilities(&self) -> &'static [&'static str] {
        &["admin"]
    }
}

fn clone_identity_err(e: &IdentityError) -> IdentityError {
    match e {
        IdentityError::Expired => IdentityError::Expired,
        IdentityError::Revoked => IdentityError::Revoked,
        IdentityError::WrongCore => IdentityError::WrongCore,
        IdentityError::BadSignature => IdentityError::BadSignature,
        _ => IdentityError::BadSignature,
    }
}

fn err_interceptor(e: IdentityError) -> AuthInterceptor {
    AuthInterceptor::new(Some(Arc::new(ErrIssuer(e))))
}

#[tokio::test]
async fn cert_path_expired_is_unauthenticated_invalid_cert() {
    let err = err_interceptor(IdentityError::Expired)
        .authenticate(iroh_request_with_cert(b"x"))
        .expect_err("expired rejects");
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
    assert_eq!(concerto_code(&err).as_deref(), Some("auth.invalid_cert"));
}

#[tokio::test]
async fn cert_path_revoked_is_permission_denied_revoked() {
    let err = err_interceptor(IdentityError::Revoked)
        .authenticate(iroh_request_with_cert(b"x"))
        .expect_err("revoked rejects");
    assert_eq!(err.code(), tonic::Code::PermissionDenied);
    assert_eq!(concerto_code(&err).as_deref(), Some("auth.revoked"));
}

#[tokio::test]
async fn cert_path_missing_header_is_invalid_cert() {
    let interceptor = err_interceptor(IdentityError::BadSignature);
    let mut req = tonic::Request::new(());
    req.extensions_mut()
        .insert(concerto_core::conn_transport::ConnTransport(
            concerto_proto::v1::TransportKind::Iroh,
        ));
    let err = interceptor
        .authenticate(req)
        .expect_err("missing header rejects");
    assert_eq!(err.code(), tonic::Code::Unauthenticated);
    assert_eq!(concerto_code(&err).as_deref(), Some("auth.invalid_cert"));
}

// ---------------------------------------------------------------------------
// UDS peer-uid fast path — over a REAL Unix socket.
// ---------------------------------------------------------------------------

/// Build a tonic `RuntimeServer` over a UnixListener with the given auth
/// interceptor, returning the bound socket path + a shutdown handle.
async fn serve_runtime_with_auth(
    interceptor: AuthInterceptor,
    socket_path: std::path::PathBuf,
) -> tokio_util::sync::CancellationToken {
    use concerto_proto::v1::runtime_server::RuntimeServer;
    use tokio::net::UnixListener;
    use tokio_stream::wrappers::UnixListenerStream;

    let handler = concerto_core::handlers::runtime::RuntimeHandler::new(
        Arc::new(std::time::SystemTime::now()),
        concerto_core::supervisor::SupervisorView::default(),
    );

    let listener = UnixListener::bind(&socket_path).expect("bind uds");
    let shutdown = tokio_util::sync::CancellationToken::new();
    let shutdown_for_task = shutdown.clone();
    let auth = interceptor;
    #[allow(clippy::result_large_err)]
    let auth_interceptor = move |req: tonic::Request<()>| auth.authenticate(req);
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .layer(tonic::service::interceptor(auth_interceptor))
            .add_service(RuntimeServer::new(handler))
            .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async move {
                shutdown_for_task.cancelled().await
            })
            .await
            .expect("serve");
    });
    shutdown
}

async fn connect(socket_path: std::path::PathBuf) -> tonic::transport::Channel {
    use tonic::transport::{Endpoint, Uri};
    use tower::service_fn;

    Endpoint::try_from("http://[::]:50051")
        .unwrap()
        .connect_with_connector(service_fn(move |_: Uri| {
            let path = socket_path.clone();
            async move {
                let stream = tokio::net::UnixStream::connect(path).await?;
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
            }
        }))
        .await
        .expect("connect uds")
}

#[tokio::test(flavor = "multi_thread")]
async fn uds_same_uid_peer_is_accepted() {
    use concerto_proto::v1::runtime_client::RuntimeClient;

    let tmp = TempDir::new().expect("tempdir");
    let sock = tmp.path().join("core.sock");

    // Default constructor reads the live process's geteuid; the test connects to
    // its own socket → same uid → accepted.
    let shutdown = serve_runtime_with_auth(AuthInterceptor::new(None), sock.clone()).await;
    // Let the listener bind.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut client = RuntimeClient::new(connect(sock.clone()).await);
    let caps = client
        .get_server_capabilities(())
        .await
        .expect("same-uid rpc succeeds")
        .into_inner();
    assert_eq!(caps.schema_version, "concerto.v1");

    shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread")]
async fn uds_non_owner_uid_is_rejected() {
    use concerto_proto::v1::runtime_client::RuntimeClient;

    let tmp = TempDir::new().expect("tempdir");
    let sock = tmp.path().join("core.sock");

    // Pin a Core uid that the live test process's real peer uid cannot match
    // (u32::MAX is never a real euid), so the very same same-process connection
    // is refused — proving the gate over a real socket.
    let interceptor = AuthInterceptor::with_core_uid(None, u32::MAX);
    let shutdown = serve_runtime_with_auth(interceptor, sock.clone()).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut client = RuntimeClient::new(connect(sock.clone()).await);
    let err = client
        .get_server_capabilities(())
        .await
        .expect_err("non-owner uid must be rejected");
    assert_eq!(err.code(), tonic::Code::Unauthenticated);

    shutdown.cancel();
}

// ---------------------------------------------------------------------------
// Task-209 startup-mirror gap closed: revoked survives a "restart".
// ---------------------------------------------------------------------------

#[tokio::test]
async fn revoked_device_stays_revoked_across_a_restart() {
    let (_tmp, persistence) = open_persistence().await;

    // --- "First boot": establish identity, issue + insert + revoke a device. ---
    let revoked_run1 = new_revoked_set();
    let (issuer1, _pub) = core_issuer(Arc::clone(&revoked_run1));
    let device = KeyPair::from_seed(&[0x33u8; 32]);
    let device_pubkey = device.verifying_key().to_bytes();
    let id_raw = device_id(&device_pubkey);
    let id_hex = hex::encode(id_raw);

    // The cert this device will keep presenting on reconnect.
    let req = PairingRequest {
        device_pubkey,
        device_name: "Stolen Phone".to_string(),
    };
    let raw_cert = on_wire(&issuer1.issue(req).await.expect("issue"));

    // Persist the row AS REVOKED (mimics a `RevokeDevice` that happened in the
    // previous run: `revoked_at` is set on disk).
    {
        let mut writer = persistence.writer().await;
        sqlx::query(
            "INSERT INTO devices (id, name, public_key, paired_at, revoked_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id_hex)
        .bind("Stolen Phone")
        .bind(&device_pubkey[..])
        .bind(1000i64)
        .bind(2000i64)
        .execute(&mut *writer)
        .await
        .expect("insert revoked device");
    }

    // --- Simulated RESTART: a brand-new, EMPTY revoked set (the bug: it forgets
    // the revocation). Without the mirror, the cert would validate fine. ---
    let revoked_run2: RevokedSet = new_revoked_set();
    let (issuer2, _pub2) = core_issuer(Arc::clone(&revoked_run2));

    // Sanity: before the mirror runs, the empty set lets the cert through.
    assert!(
        issuer2.validate(&raw_cert).is_ok(),
        "without the startup mirror the revoked device would be accepted — this is the Task-209 gap"
    );

    // Run the Task-210 startup mirror (what boot now does before the auth path
    // goes live): rebuild the revoked set from the DB.
    let restored = mirror_revoked_devices(&persistence, &revoked_run2)
        .await
        .expect("mirror revoked devices");
    assert_eq!(restored, 1, "exactly the one revoked row mirrored");

    // After the mirror, the SAME cert is rejected as Revoked across the restart.
    assert!(
        matches!(issuer2.validate(&raw_cert), Err(IdentityError::Revoked)),
        "after the startup mirror the previously-revoked device stays revoked"
    );

    // And through the auth interceptor it maps to PERMISSION_DENIED/auth.revoked.
    let interceptor = AuthInterceptor::new(Some(Arc::clone(&issuer2)));
    let err = interceptor
        .authenticate(iroh_request_with_cert(&raw_cert))
        .expect_err("revoked cert rejected at the auth layer");
    assert_eq!(err.code(), tonic::Code::PermissionDenied);
    assert_eq!(concerto_code(&err).as_deref(), Some("auth.revoked"));
}
