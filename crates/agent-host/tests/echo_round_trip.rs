//! End-to-end integration test for `concerto-agent-host`.
//!
//! Spawns the host binary from `assert_cmd::cargo::cargo_bin`, wrapping
//! `echo hello` as the agent CLI. Drives the protocol from the Core side:
//! send `Hello`, expect `Ready`, expect a `StdoutBytes` chunk containing
//! `hello`, expect `AgentExited`. Also verifies cookie-mismatch handling
//! in a separate test.
//!
//! Unix-only — the binary itself only compiles on Unix in V0.1.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use concerto_agent_host::api::{FinalInfo, HostFrame};
use concerto_agent_host::bridge::{read_frame, write_frame, FrameError};
use tempfile::TempDir;
use tokio::net::UnixStream;

/// Build a fully-populated CLI invocation against the workspace's
/// agent-host binary, parameterised on cookie + agent program so each
/// test can vary just the bits it cares about.
struct Harness {
    _dir: TempDir,
    socket: PathBuf,
    final_info: PathBuf,
    child: std::process::Child,
}

impl Drop for Harness {
    fn drop(&mut self) {
        // Best-effort kill in case a test exits early.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_host(agent_bin: &str, agent_args: &[&str], cookie_hex: &str) -> Harness {
    let dir = TempDir::new().expect("tempdir");
    let socket = dir.path().join("host.sock");
    let final_info = dir.path().join("final.json");
    let bin = assert_cmd::cargo::cargo_bin("concerto-agent-host");
    let mut cmd = Command::new(bin);
    cmd.arg("--agent-bin")
        .arg(agent_bin)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--socket")
        .arg(&socket)
        .arg("--cookie")
        .arg(cookie_hex)
        .arg("--final-info")
        .arg(&final_info);
    for a in agent_args {
        cmd.arg("--agent-arg").arg(a);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd.spawn().expect("spawn concerto-agent-host");
    Harness {
        _dir: dir,
        socket,
        final_info,
        child,
    }
}

/// Block (in async) until the socket file exists and is bound. The host
/// creates it just after spawn so this normally returns in a few ms.
async fn wait_for_socket(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if path.exists() {
            return;
        }
        if Instant::now() > deadline {
            panic!("socket {:?} did not appear within 10s", path);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Connect to the bound host socket. Retries briefly in case `bind`
/// raced with `accept` start.
async fn connect(socket: &std::path::Path) -> UnixStream {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match UnixStream::connect(socket).await {
            Ok(s) => return s,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(e) => panic!("connect: {e}"),
        }
    }
}

#[tokio::test]
async fn echo_round_trip() {
    let cookie_bytes = [0x42u8; 32];
    let cookie_hex = hex::encode(cookie_bytes);
    let mut h = spawn_host("echo", &["hello"], &cookie_hex);
    wait_for_socket(&h.socket).await;

    // Confirm 0600 mode on the bound socket.
    let mode = std::fs::metadata(&h.socket).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "socket should be 0600, got {:o}", mode);

    let mut stream = connect(&h.socket).await;
    let (mut reader, mut writer) = stream.split();

    let hello = HostFrame::Hello {
        core_version: "test".into(),
        expected_cookie: cookie_bytes,
        last_seq: 0,
    };
    write_frame(&mut writer, &hello).await.expect("send Hello");

    // First frame back must be Ready.
    let ready = read_frame(&mut reader).await.expect("read Ready");
    match ready {
        HostFrame::Ready { .. } => {}
        other => panic!("expected Ready, got {other:?}"),
    }

    // Drain frames until AgentExited, accumulating stdout.
    let mut stdout = Vec::new();
    let mut exit: Option<(Option<i32>, Option<i32>)> = None;
    let drain_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if Instant::now() > drain_deadline {
            panic!("did not see AgentExited within 10s; stdout so far: {stdout:?}");
        }
        let r = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut reader)).await;
        let frame = match r {
            Ok(Ok(f)) => f,
            Ok(Err(FrameError::Eof)) => break,
            Ok(Err(e)) => panic!("read_frame: {e}"),
            Err(_) => panic!("timeout draining frames; stdout so far: {stdout:?}"),
        };
        match frame {
            HostFrame::StdoutBytes { data, .. } => stdout.extend(data),
            HostFrame::AgentExited { exit_code, signal } => {
                exit = Some((exit_code, signal));
                break;
            }
            HostFrame::Pong => {}
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    let stdout_str = String::from_utf8_lossy(&stdout);
    assert!(
        stdout_str.contains("hello"),
        "expected 'hello' in stdout, got {stdout_str:?}"
    );
    let (code, _signal) = exit.expect("AgentExited not received");
    assert_eq!(code, Some(0), "echo should exit 0");

    // Host should exit shortly after the child does.
    let exit_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match h.child.try_wait().expect("try_wait") {
            Some(status) => {
                assert!(status.success(), "host exited with {status:?}");
                break;
            }
            None if Instant::now() > exit_deadline => {
                panic!("host did not exit within 10s");
            }
            None => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }

    // Final-info JSON should exist and parse with the locked schema.
    let raw = std::fs::read_to_string(&h.final_info).expect("read final-info");
    let info: FinalInfo = serde_json::from_str(&raw).expect("parse final-info");
    assert_eq!(info.exit_code, Some(0));
    assert!(info.exited_at_unix_ms > 0);
}

#[tokio::test]
async fn rejects_wrong_cookie() {
    let cookie_bytes = [0x11u8; 32];
    let cookie_hex = hex::encode(cookie_bytes);
    let mut h = spawn_host("echo", &["hello"], &cookie_hex);
    wait_for_socket(&h.socket).await;

    let mut stream = connect(&h.socket).await;
    let (mut reader, mut writer) = stream.split();

    // Send Hello with the wrong cookie.
    let wrong = [0u8; 32];
    let hello = HostFrame::Hello {
        core_version: "test".into(),
        expected_cookie: wrong,
        last_seq: 0,
    };
    write_frame(&mut writer, &hello).await.expect("send Hello");

    // Expect CookieMismatch, then EOF.
    let resp = read_frame(&mut reader).await.expect("read CookieMismatch");
    assert!(
        matches!(resp, HostFrame::CookieMismatch),
        "expected CookieMismatch, got {resp:?}"
    );
    // The host closes the connection right after; subsequent reads should
    // either hit EOF or another protocol-level close. Either is fine.
    let _ = read_frame(&mut reader).await;

    // Cleanly tear down: the agent CLI is still alive on the host side
    // (we never advanced past Hello). Echo will exit anyway; wait for
    // the host to finish.
    let exit_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match h.child.try_wait().expect("try_wait") {
            Some(_status) => break,
            None if Instant::now() > exit_deadline => {
                // Kill the host so the test doesn't hang; this is fine —
                // the cookie-mismatch path is exercised already.
                let _ = h.child.kill();
                break;
            }
            None => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}
