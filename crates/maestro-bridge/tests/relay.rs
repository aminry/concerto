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
    });

    let input = &b"ping\n"[..];
    let mut output: Vec<u8> = Vec::new();
    concerto_maestro_bridge::relay(&sock, input, &mut output)
        .await
        .unwrap();

    server.await.unwrap();
    assert_eq!(output, b"ping\n-pong\n");
}
