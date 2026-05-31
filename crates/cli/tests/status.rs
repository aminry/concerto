// This integration test spawns a real Core over a Unix-domain socket via the
// Unix-only `concerto-test-harness`, so the whole target is empty off Unix.
// On the Windows CI lane (`--all-targets`) this makes the file compile to
// nothing instead of failing on the missing UDS transport / harness.
#![cfg(unix)]

//! Integration test: spawn a real `concerto-core` via the shared
//! `concerto-test-harness` and run the built `concerto` binary's `status`
//! subcommand against it over UDS.
//!
//! This proves the end-to-end path Task 109 ships: socket resolution →
//! `client::connect` UDS dial → `Runtime.GetServerCapabilities` + `GetStatus`
//! → rendering. It exercises the actual shipped binary (not just library
//! functions) via `assert_cmd`, so the clap wiring and `main`'s exit codes
//! are covered too.

use assert_cmd::Command;
use concerto_test_harness::CoreUnderTest;

/// `concerto --socket <core.sock> status` exits 0 and prints the version.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_against_live_core_prints_version() {
    let core = CoreUnderTest::spawn()
        .await
        .expect("spawn concerto-core via the test harness");

    let socket = core.socket_path.clone();

    // Run the built `concerto` binary in a blocking thread — `assert_cmd`
    // is synchronous and we're on a tokio worker.
    let assert = tokio::task::spawn_blocking(move || {
        Command::cargo_bin("concerto")
            .expect("locate the built `concerto` binary")
            .arg("--socket")
            .arg(&socket)
            .arg("status")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone()
    })
    .await
    .expect("join blocking status command");

    let stdout = String::from_utf8(assert).expect("status stdout is UTF-8");

    // The text renderer prints a `version:` line and the UDS transport.
    assert!(
        stdout.contains("version:"),
        "status output should include a version line; got:\n{stdout}"
    );
    assert!(
        stdout.contains("transport:") && stdout.contains("TRANSPORT_KIND_UDS"),
        "status output should report the UDS transport; got:\n{stdout}"
    );

    core.shutdown().await.expect("clean Core shutdown");
}

/// `concerto --socket <core.sock> --json status` emits valid JSON with the
/// version + UDS transport fields.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_json_emits_machine_readable_output() {
    let core = CoreUnderTest::spawn()
        .await
        .expect("spawn concerto-core via the test harness");

    let socket = core.socket_path.clone();

    let stdout_bytes = tokio::task::spawn_blocking(move || {
        Command::cargo_bin("concerto")
            .expect("locate the built `concerto` binary")
            .arg("--socket")
            .arg(&socket)
            .arg("--json")
            .arg("status")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone()
    })
    .await
    .expect("join blocking status command");

    let value: serde_json::Value =
        serde_json::from_slice(&stdout_bytes).expect("--json status output parses as JSON");

    assert!(
        value
            .get("version")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty()),
        "JSON status should carry a non-empty version; got: {value}"
    );
    assert_eq!(
        value.get("transport_kind").and_then(|v| v.as_str()),
        Some("TRANSPORT_KIND_UDS"),
        "JSON status should report the UDS transport; got: {value}"
    );

    core.shutdown().await.expect("clean Core shutdown");
}

/// With no Core listening, `concerto status` fails and names the socket path
/// it tried — the Core-down ergonomics Task 109 requires.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_without_core_names_the_socket() {
    let tempdir = tempdir_path();
    let socket = tempdir.join("does-not-exist.sock");
    let socket_for_cmd = socket.clone();

    let output = tokio::task::spawn_blocking(move || {
        Command::cargo_bin("concerto")
            .expect("locate the built `concerto` binary")
            .arg("--socket")
            .arg(&socket_for_cmd)
            .arg("status")
            .assert()
            .failure()
            .get_output()
            .stderr
            .clone()
    })
    .await
    .expect("join blocking status command");

    let stderr = String::from_utf8(output).expect("stderr is UTF-8");
    assert!(
        stderr.contains(&socket.display().to_string()),
        "Core-down error should name the socket path it tried; got:\n{stderr}"
    );
}

/// A throwaway directory path that does not need to exist (we only build a
/// non-existent socket path under it). Uses the OS temp dir.
fn tempdir_path() -> std::path::PathBuf {
    std::env::temp_dir()
}
