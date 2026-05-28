//! Integration test for Task 11: spawn `concerto-core` as a subprocess,
//! verify the pid file, race a second instance, then SIGTERM the first
//! and verify the pid file is cleaned up.
//!
//! This test is Unix-only: it relies on `kill(SIGTERM)` and on the
//! UDS-style file-system layout that V0.1 ships. On Windows the
//! equivalent test is V1.0 work.

#![cfg(unix)]

use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// Find the just-built `concerto-core` binary. `CARGO_BIN_EXE_<bin>` is
/// the cargo-managed way to locate the binary for integration tests.
fn core_bin() -> &'static str {
    env!("CARGO_BIN_EXE_concerto-core")
}

/// Poll until `cond` returns true or the deadline elapses.
fn wait_until(deadline: Instant, mut cond: impl FnMut() -> bool) -> bool {
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        thread::sleep(Duration::from_millis(50));
    }
    cond()
}

#[test]
fn second_instance_exits_zero_first_instance_cleans_pid_on_sigterm() {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().join("data");
    let config_dir = tmp.path().join("config");
    let pid_path = config_dir.join("core.pid");

    // --- Spawn first instance ---------------------------------------------
    let mut first = Command::new(core_bin())
        .env("CONCERTO_DATA_DIR", &data_dir)
        .env("CONCERTO_CONFIG_DIR", &config_dir)
        // Keep logs out of the developer's real ~/concerto/logs.
        .env("HOME", tmp.path())
        // Reduce noise; the test asserts on side effects, not stdout.
        .env("RUST_LOG", "info")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn first concerto-core");

    // Wait for the pid file to appear (= Runtime::start finished).
    let appeared = wait_until(Instant::now() + Duration::from_secs(20), || {
        pid_path.exists()
    });
    if !appeared {
        // Capture stderr for diagnostics before we give up.
        let _ = first.kill();
        let out = first.wait_with_output().ok();
        panic!(
            "first instance never wrote pid file at {}\nstderr:\n{}",
            pid_path.display(),
            out.as_ref()
                .map(|o| String::from_utf8_lossy(&o.stderr).into_owned())
                .unwrap_or_default()
        );
    }

    // Verify the pid file content: it should be valid JSON with the
    // expected fields, and `pid` should match the spawned child.
    let raw = std::fs::read_to_string(&pid_path).expect("read pid file");
    let parsed: serde_json::Value = serde_json::from_str(raw.trim()).expect("pid file is JSON");
    let recorded_pid = parsed
        .get("pid")
        .and_then(|v| v.as_u64())
        .expect("pid field");
    assert_eq!(
        recorded_pid as u32,
        first.id(),
        "pid file records the first instance's PID"
    );
    assert!(
        parsed.get("version").and_then(|v| v.as_str()).is_some(),
        "pid file has a version string"
    );
    assert!(
        parsed
            .get("start_epoch_secs")
            .and_then(|v| v.as_u64())
            .is_some(),
        "pid file has a start_epoch_secs"
    );

    // --- Spawn second instance, expect clean exit 0 -----------------------
    let second_out = Command::new(core_bin())
        .env("CONCERTO_DATA_DIR", &data_dir)
        .env("CONCERTO_CONFIG_DIR", &config_dir)
        .env("HOME", tmp.path())
        .env("RUST_LOG", "info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run second concerto-core");

    assert!(
        second_out.status.success(),
        "second instance should exit 0 — got {:?}\nstderr:\n{}",
        second_out.status,
        String::from_utf8_lossy(&second_out.stderr)
    );

    // The pid file should NOT have been corrupted by the second instance.
    let raw_after =
        std::fs::read_to_string(&pid_path).expect("read pid file after second instance");
    assert_eq!(
        raw, raw_after,
        "second instance must not modify the existing pid file"
    );

    // --- SIGTERM the first instance, verify clean shutdown ----------------
    // SAFETY: kill(pid, SIGTERM) on our own child. The PID is owned by us
    // until we reap it via .wait().
    let rc = unsafe { libc::kill(first.id() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(rc, 0, "kill(SIGTERM) succeeded");

    let exit = first.wait().expect("wait on first instance");
    assert!(
        exit.success(),
        "first instance should exit 0 after SIGTERM — got {exit:?}"
    );

    // Pid file should be gone.
    let gone = wait_until(Instant::now() + Duration::from_secs(5), || {
        !pid_path.exists()
    });
    assert!(
        gone,
        "pid file at {} should be removed after SIGTERM",
        pid_path.display()
    );
}
