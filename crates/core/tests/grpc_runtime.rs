//! Integration tests for the Task 13 gRPC server over UDS.
//!
//! These tests:
//!
//! 1. Start a real `Runtime`, spawn the `ApiServerActor`, connect a
//!    Tonic client over `unix://<core.sock>`, and assert `GetServerCapabilities`
//!    + `GetStatus` return live values.
//! 2. Pre-populate a stale socket file at `<config_dir>/core.sock`
//!    before start; verify the actor replaces it and listens
//!    successfully.
//! 3. Verify the socket file has `0600` permissions on disk.
//!
//! Unix-only (the locked surface is UDS; Windows named-pipe support
//! is V1.0).

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use concerto_core::api_server::{ApiServerActor, ApiServerConfig};
use concerto_core::runtime::{Runtime, RuntimeConfig, StartOutcome};
use concerto_proto::v1::runtime_client::RuntimeClient;
use concerto_proto::v1::TransportKind;
use tempfile::TempDir;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::UnixStream;
use tonic::transport::{Endpoint, Uri};

/// Wrapper used by the test client to plug a `UnixStream` into Tonic's
/// `connect_with_connector` API. Tonic wants a type that implements
/// `hyper::rt::Read + Write`; `hyper_util::rt::TokioIo` adapts a tokio
/// stream to that contract.
async fn unix_connect(
    socket_path: PathBuf,
) -> Result<impl AsyncRead + AsyncWrite + Send + Unpin + 'static, std::io::Error> {
    UnixStream::connect(socket_path).await
}

/// Bring up a real `Runtime` + `ApiServerActor` in `tmp`, return the
/// runtime and the absolute socket path. The caller is responsible
/// for `runtime.stop().await` when done — otherwise the persistence
/// pool leaks.
async fn boot_with_api(tmp: &TempDir) -> (Runtime, PathBuf) {
    let cfg = RuntimeConfig {
        data_dir: tmp.path().join("data"),
        config_dir: tmp.path().join("config"),
        shutdown_grace: Duration::from_secs(2),
    };
    // Ensure config_dir exists for the socket path; Runtime::start
    // only creates data_dir.
    tokio::fs::create_dir_all(&cfg.config_dir).await.unwrap();
    let socket_path = cfg.config_dir.join("core.sock");

    let mut runtime = match Runtime::start(cfg).await.expect("Runtime::start") {
        StartOutcome::Started(r) => r,
        StartOutcome::AlreadyRunning { pid } => panic!("unexpected AlreadyRunning(pid={pid})"),
    };

    let started_at = runtime.started_at();
    let view = runtime
        .supervisor()
        .expect("supervisor present at boot")
        .view();
    let factory_started = Arc::clone(&started_at);
    let factory_view = view.clone();
    let cfg = ApiServerConfig {
        socket_path: socket_path.clone(),
    };
    runtime
        .supervisor_mut()
        .expect("supervisor present")
        .spawn::<ApiServerActor, _>(
            move || ApiServerActor::new(Arc::clone(&factory_started), factory_view.clone()),
            cfg,
        )
        .await
        .expect("spawn ApiServerActor");

    // Wait until the socket file appears — the actor binds inside its
    // own task and there's no synchronous handshake. Cap at 5s.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !socket_path.exists() {
        if std::time::Instant::now() > deadline {
            panic!("socket file never appeared at {}", socket_path.display());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    (runtime, socket_path)
}

/// Build a Tonic `RuntimeClient` connected to `socket_path`.
async fn connect_client(socket_path: PathBuf) -> RuntimeClient<tonic::transport::Channel> {
    // The URI is a placeholder — Tonic requires *something* parseable
    // for the authority but we override the connector below to route
    // every connection to our UDS path.
    let endpoint = Endpoint::try_from("http://[::1]:50051")
        .unwrap()
        .connect_timeout(Duration::from_secs(2));
    let channel = endpoint
        .connect_with_connector(tower::service_fn(move |_: Uri| {
            let p = socket_path.clone();
            async move {
                let stream = unix_connect(p).await?;
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
            }
        }))
        .await
        .expect("connect to UDS");
    RuntimeClient::new(channel)
}

#[tokio::test]
async fn get_capabilities_returns_uds_transport() {
    let tmp = TempDir::new().unwrap();
    let (runtime, socket_path) = boot_with_api(&tmp).await;

    let mut client = connect_client(socket_path.clone()).await;
    let resp = client.get_server_capabilities(()).await.expect("rpc");
    let caps = resp.into_inner();

    assert_eq!(caps.transport_kind, TransportKind::Uds as i32);
    assert_eq!(caps.schema_version, "concerto.v1");
    assert_eq!(caps.server_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(caps.core_host_os, std::env::consts::OS);
    assert!(!caps.core_hostname.is_empty());
    let limits = caps.limits.expect("limits populated");
    assert_eq!(limits.max_concurrent_streams, 256);
    assert_eq!(limits.max_payload_bytes, 16 * 1024 * 1024);

    runtime.stop().await.expect("stop");
    // After stop, the socket must be gone (cleanup is best-effort but
    // the happy path always removes it).
    assert!(
        !socket_path.exists(),
        "socket file should be removed on clean shutdown"
    );
}

#[tokio::test]
async fn get_status_reports_uptime() {
    let tmp = TempDir::new().unwrap();
    let (runtime, socket_path) = boot_with_api(&tmp).await;

    // Give uptime at least one second of wall clock to register.
    tokio::time::sleep(Duration::from_millis(1100)).await;

    let mut client = connect_client(socket_path).await;
    let resp = client.get_status(()).await.expect("rpc");
    let st = resp.into_inner();

    assert!(st.uptime_seconds >= 1, "uptime should be >=1");
    assert_eq!(st.version, env!("CARGO_PKG_VERSION"));
    assert!(st.started_at.is_some());

    runtime.stop().await.expect("stop");
}

#[tokio::test]
async fn stale_socket_file_is_replaced() {
    let tmp = TempDir::new().unwrap();
    let config_dir = tmp.path().join("config");
    tokio::fs::create_dir_all(&config_dir).await.unwrap();
    let socket_path = config_dir.join("core.sock");

    // Create a real stale socket the way a crashed process would: bind
    // and then drop the listener WITHOUT removing the file.
    {
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        drop(listener);
        // `UnixListener` does not remove the file on drop, so the
        // sentinel is still on disk.
        assert!(
            socket_path.exists(),
            "stale socket should be on disk for the test setup"
        );
    }

    // Boot a Runtime that points at the same config_dir and verify the
    // ApiServerActor replaces the stale socket and serves.
    let cfg = RuntimeConfig {
        data_dir: tmp.path().join("data"),
        config_dir: config_dir.clone(),
        shutdown_grace: Duration::from_secs(2),
    };
    let mut runtime = match Runtime::start(cfg).await.expect("Runtime::start") {
        StartOutcome::Started(r) => r,
        StartOutcome::AlreadyRunning { pid } => panic!("unexpected AlreadyRunning(pid={pid})"),
    };
    let started_at = runtime.started_at();
    let view = runtime.supervisor().expect("supervisor").view();
    let factory_started = Arc::clone(&started_at);
    let factory_view = view.clone();
    runtime
        .supervisor_mut()
        .expect("supervisor present")
        .spawn::<ApiServerActor, _>(
            move || ApiServerActor::new(Arc::clone(&factory_started), factory_view.clone()),
            ApiServerConfig {
                socket_path: socket_path.clone(),
            },
        )
        .await
        .expect("spawn after stale socket");

    // The actor should have replaced the stale socket and started
    // listening; verify by connecting.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut ok = false;
    while std::time::Instant::now() < deadline {
        if let Ok(stream) = tokio::net::UnixStream::connect(&socket_path).await {
            drop(stream);
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(ok, "could not connect to UDS after stale-socket recovery");

    runtime.stop().await.expect("stop");
}

#[tokio::test]
async fn socket_permissions_are_owner_only() {
    let tmp = TempDir::new().unwrap();
    let (runtime, socket_path) = boot_with_api(&tmp).await;

    let md = std::fs::metadata(&socket_path).expect("stat socket");
    // Mask off the file-type bits; only the permission bits matter.
    let mode = md.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "socket permissions must be 0600, got {mode:o}");

    runtime.stop().await.expect("stop");
}
