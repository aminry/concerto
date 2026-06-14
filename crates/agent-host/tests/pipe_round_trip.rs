//! Integration test for `concerto-agent-host --io-mode pipe`.
//!
//! Spawns the host binary wrapping `/bin/cat` as the agent CLI in pipe mode
//! (plain piped stdio, no PTY). Drives the protocol from the Core side:
//! send `Hello`, expect `Ready`, send `StdinBytes { data: b"ping\n" }`, then
//! drain frames until a `StdoutBytes` chunk containing `ping` arrives.
//!
//! This proves the pipe-mode wiring end-to-end: the host spawns `/bin/cat`
//! with piped stdio, the StdinBytes pump writes to cat's stdin, cat echoes
//! it back, and the StdoutBytes pump delivers it as a frame. If the pipe
//! pumps were broken, no `StdoutBytes` would arrive and the test times out.
//!
//! Unix-only: same rationale as echo_round_trip.rs.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use concerto_agent_host::api::HostFrame;
use concerto_agent_host::bridge::{read_frame, write_frame, FrameError};
use tempfile::TempDir;
use tokio::net::UnixStream;

struct Harness {
    _dir: TempDir,
    socket: PathBuf,
    child: std::process::Child,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_host_pipe(agent_bin: &str, cookie_hex: &str) -> Harness {
    let dir = TempDir::new().expect("tempdir");
    let socket = dir.path().join("host.sock");
    let final_info = dir.path().join("final.json");
    let bin = assert_cmd::cargo::cargo_bin("concerto-agent-host");
    let mut cmd = Command::new(bin);
    cmd.arg("--io-mode")
        .arg("pipe")
        .arg("--agent-bin")
        .arg(agent_bin)
        .arg("--cwd")
        .arg(dir.path())
        .arg("--socket")
        .arg(&socket)
        .arg("--cookie")
        .arg(cookie_hex)
        .arg("--final-info")
        .arg(&final_info);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd.spawn().expect("spawn concerto-agent-host");
    Harness {
        _dir: dir,
        socket,
        child,
    }
}

async fn wait_for_socket(path: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if path.exists() {
            return;
        }
        if Instant::now() > deadline {
            panic!("socket {:?} did not appear within 60s", path);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn connect(socket: &std::path::Path) -> UnixStream {
    let deadline = Instant::now() + Duration::from_secs(30);
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
async fn pipe_round_trip() {
    let cookie_bytes = [0x55u8; 32];
    let cookie_hex = hex::encode(cookie_bytes);
    let mut h = spawn_host_pipe("/bin/cat", &cookie_hex);
    wait_for_socket(&h.socket).await;

    let mut stream = connect(&h.socket).await;
    let (mut reader, mut writer) = stream.split();

    // Handshake: send Hello, expect Ready.
    let hello = HostFrame::Hello {
        core_version: "test".into(),
        expected_cookie: cookie_bytes,
        last_seq: 0,
    };
    write_frame(&mut writer, &hello).await.expect("send Hello");

    let ready = read_frame(&mut reader).await.expect("read Ready");
    match ready {
        HostFrame::Ready { .. } => {}
        other => panic!("expected Ready, got {other:?}"),
    }

    // Send stdin bytes to /bin/cat — it echoes them verbatim on stdout.
    let stdin_frame = HostFrame::StdinBytes {
        data: b"ping\n".to_vec(),
    };
    write_frame(&mut writer, &stdin_frame)
        .await
        .expect("send StdinBytes");

    // Drain frames until we see a StdoutBytes containing "ping".
    let mut stdout = Vec::new();
    let drain_deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if Instant::now() > drain_deadline {
            panic!("did not see StdoutBytes with 'ping' within 120s; stdout so far: {stdout:?}");
        }

        let r = tokio::time::timeout(Duration::from_secs(90), read_frame(&mut reader)).await;
        let frame = match r {
            Ok(Ok(f)) => f,
            Ok(Err(FrameError::Eof)) => break,
            Ok(Err(e)) => panic!("read_frame: {e}"),
            Err(_) => panic!("timeout waiting for StdoutBytes; stdout so far: {stdout:?}"),
        };
        match frame {
            HostFrame::StdoutBytes { data, .. } => {
                stdout.extend_from_slice(&data);
                if stdout.windows(4).any(|w| w == b"ping") {
                    break;
                }
            }
            HostFrame::AgentExited { .. } => {
                // cat exited before we saw output — shouldn't happen but
                // check the buffer.
                break;
            }
            HostFrame::Pong => {}
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    let stdout_str = String::from_utf8_lossy(&stdout);
    assert!(
        stdout_str.contains("ping"),
        "expected 'ping' in stdout, got {stdout_str:?}"
    );

    // /bin/cat keeps stdin open until EOF.  Killing the host is the
    // clean teardown path here; Drop will also kill it, but doing it
    // explicitly here ensures we don't block the test runner.
    let _ = h.child.kill();
    let _ = h.child.wait();
}
