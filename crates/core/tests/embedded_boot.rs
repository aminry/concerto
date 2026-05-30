//! Proves `boot::start` produces a Core that serves gRPC over its UDS
//! and shuts down cleanly when its token is cancelled — the contract
//! the embedded desktop path depends on.

use std::time::Duration;

use concerto_core::boot::{self, BootOutcome};
use concerto_core::runtime::RuntimeConfig;

#[tokio::test(flavor = "multi_thread")]
async fn embedded_boot_serves_and_shuts_down() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data_dir = tmp.path().join("data");
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::create_dir_all(&config_dir).unwrap();

    let config = RuntimeConfig {
        data_dir: data_dir.clone(),
        config_dir: config_dir.clone(),
        shutdown_grace: Duration::from_secs(5),
    };

    let core = match boot::start(config).await.expect("boot::start") {
        BootOutcome::Started(c) => c,
        BootOutcome::AlreadyRunning { pid } => panic!("unexpected live instance pid={pid}"),
    };

    let sock = core.socket_path().to_path_buf();
    assert_eq!(sock, config_dir.join("core.sock"));

    // The gRPC server binds the UDS inside its supervised actor's `run`
    // loop, which runs concurrently with `boot::start` returning — the
    // same asynchronous bind the daemon has always done. Poll until the
    // socket appears (the test-harness `wait_for_socket` uses the same
    // pattern against the spawned daemon).
    let stream = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(s) = tokio::net::UnixStream::connect(&sock).await {
                break s;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    // Connecting (not just `sock.exists()`) proves the server is actually
    // accepting on the UDS — the "serves" half of this test's name.
    assert!(stream.is_ok(), "gRPC server should accept on the UDS shortly after boot");
    drop(stream);

    let token = core.shutdown_token();
    let join = tokio::spawn(async move { core.run_until_shutdown().await });

    token.cancel();
    let res = tokio::time::timeout(Duration::from_secs(10), join).await;
    assert!(res.is_ok(), "run_until_shutdown should return after cancel");
    res.unwrap().expect("join").expect("clean shutdown");
}
