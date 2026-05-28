//! Integration tests for the Task 13 gRPC server over UDS.
//!
//! As of Task 17 these tests use the shared `concerto-test-harness`
//! crate to spawn a real `concerto-core` subprocess instead of booting
//! the `Runtime` + `ApiServerActor` in-process. The exception is
//! `stale_socket_file_is_replaced`, which exercises the actor's
//! socket-cleanup branch directly: the harness only owns its own
//! tempdir AFTER `spawn()` returns, so the stale-socket pre-state has
//! to be planted by the test itself with a hand-rolled `Runtime`.
//!
//! See `tasks/17-integration-test-harness.md` and
//! `crates/core/tests/grpc_runtime.rs` (Task 13 original) for context.
//!
//! Unix-only (the locked surface is UDS; Windows named-pipe support
//! is V1.0).

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;

use concerto_core::api_server::{ApiServerActor, ApiServerConfig};
use concerto_core::runtime::{Runtime, RuntimeConfig, StartOutcome};
use concerto_proto::v1::TransportKind;
use concerto_test_harness::CoreUnderTest;
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread")]
async fn get_capabilities_returns_uds_transport() {
    let core = CoreUnderTest::spawn().await.expect("spawn");

    let mut client = core.runtime_client().await.expect("client");
    let caps = client
        .get_server_capabilities(())
        .await
        .expect("rpc")
        .into_inner();

    assert_eq!(caps.transport_kind, TransportKind::Uds as i32);
    assert_eq!(caps.schema_version, "concerto.v1");
    assert_eq!(caps.server_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(caps.core_host_os, std::env::consts::OS);
    assert!(!caps.core_hostname.is_empty());
    let limits = caps.limits.expect("limits populated");
    assert_eq!(limits.max_concurrent_streams, 256);
    assert_eq!(limits.max_payload_bytes, 16 * 1024 * 1024);

    let socket_path = core.socket_path.clone();
    core.shutdown().await.expect("shutdown");
    // After shutdown the socket file must be gone (cleanup is
    // best-effort but the happy path always removes it).
    assert!(
        !socket_path.exists(),
        "socket file should be removed on clean shutdown"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn get_status_reports_uptime() {
    let core = CoreUnderTest::spawn().await.expect("spawn");

    // Give uptime at least one second of wall clock to register.
    tokio::time::sleep(Duration::from_millis(1100)).await;

    let mut client = core.runtime_client().await.expect("client");
    let st = client.get_status(()).await.expect("rpc").into_inner();

    assert!(st.uptime_seconds >= 1, "uptime should be >=1");
    assert_eq!(st.version, env!("CARGO_PKG_VERSION"));
    assert!(st.started_at.is_some());

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn socket_permissions_are_owner_only() {
    let core = CoreUnderTest::spawn().await.expect("spawn");

    let md = std::fs::metadata(&core.socket_path).expect("stat socket");
    // Mask off the file-type bits; only the permission bits matter.
    let mode = md.permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "socket permissions must be 0600, got {mode:o}");

    core.shutdown().await.expect("shutdown");
}

/// Stale-socket recovery is exercised by booting an in-process Runtime
/// against a pre-populated tempdir. The harness's `spawn()` owns its
/// tempdir and only returns it once the Core is up; planting a stale
/// socket before the Core binds is therefore not expressible via the
/// harness's surface. Keeping this one test in-process is the documented
/// drift in Task 17's Handoff Notes.
#[tokio::test(flavor = "multi_thread")]
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
        assert!(
            socket_path.exists(),
            "stale socket should be on disk for the test setup"
        );
    }

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
