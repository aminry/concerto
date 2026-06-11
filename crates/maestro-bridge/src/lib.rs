//! `concerto-maestro-bridge` — a dumb stdio↔UDS relay. The Claude CLI spawns
//! this (named in the Maestro `.mcp.json`); it connects to the Core's
//! Maestro-MCP unix socket and copies bytes both directions. It has NO MCP
//! knowledge: MCP stdio framing (newline-delimited JSON-RPC) passes through
//! transparently. Unix-only (the Maestro is `#[cfg(unix)]`).

use std::path::Path;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;

/// Relay `input` → socket and socket → `output` concurrently until either side
/// reaches EOF. Generic over the streams so tests drive it with in-memory buffers.
pub async fn relay<R, W>(socket: &Path, mut input: R, mut output: W) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let stream = UnixStream::connect(socket).await?;
    let (mut sock_r, mut sock_w) = stream.into_split();

    tokio::select! {
        r = tokio::io::copy(&mut input, &mut sock_w) => {
            r?;
            let _ = sock_w.shutdown().await;
        }
        r = tokio::io::copy(&mut sock_r, &mut output) => {
            r?;
            let _ = output.shutdown().await;
        }
    }
    Ok(())
}
