// Unix-only: exercises the `UnixStream`-based relay (the crate is `#![cfg(unix)]`).
#![cfg(unix)]

use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

#[tokio::test]
async fn relay_copies_both_directions() {
    let dir = tempfile::tempdir().unwrap();
    let sock: PathBuf = dir.path().join("mcp.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    let server = tokio::spawn(async move {
        let (mut conn, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 64];
        let n = conn.read(&mut buf).await.unwrap();
        let mut out = buf[..n].to_vec();
        out.extend_from_slice(b"-pong\n");
        conn.write_all(&out).await.unwrap();
        conn.flush().await.unwrap();
        // Dropping conn here closes the socket → triggers EOF on sock_r,
        // so the down direction (socket→output) wins select! and relay exits.
    });

    // Use a duplex so the up direction (input→socket) stays open/pending while
    // the server responds and closes. That lets the down direction win select!.
    let (mut stdin_writer, stdin_reader) = tokio::io::duplex(64);
    stdin_writer.write_all(b"ping\n").await.unwrap();
    // Keep stdin_writer alive (not dropped) so up direction stays pending.

    let mut output: Vec<u8> = Vec::new();
    concerto_maestro_bridge::relay(&sock, stdin_reader, &mut output)
        .await
        .unwrap();

    server.await.unwrap();
    assert_eq!(output, b"ping\n-pong\n");
}

#[tokio::test]
async fn relay_exits_when_socket_closes_with_stdin_open() {
    let dir = tempfile::tempdir().unwrap();
    let sock: PathBuf = dir.path().join("mcp.sock");
    let listener = UnixListener::bind(&sock).unwrap();

    let server = tokio::spawn(async move {
        let (mut conn, _) = listener.accept().await.unwrap();
        conn.write_all(b"hello\n").await.unwrap();
        conn.flush().await.unwrap();
        // drop conn → closes the socket while client stdin is still open
    });

    // keep the write end alive so the stdin side never reaches EOF
    let (_stdin_writer, stdin_reader) = tokio::io::duplex(64);
    let mut output = Vec::new();
    let res = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        concerto_maestro_bridge::relay(&sock, stdin_reader, &mut output),
    )
    .await;

    server.await.unwrap();
    assert!(
        res.is_ok(),
        "relay must exit when the socket closes, not hang"
    );
    res.unwrap().unwrap();
    assert_eq!(output, b"hello\n");
}
