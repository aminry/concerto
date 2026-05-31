//! Hand-rolled Tonic-over-Iroh adapter (the load-bearing part of Task 102).
//!
//! Tonic can serve over, and connect through, any connected transport that
//! looks like a `tokio::io::AsyncRead + AsyncWrite`. Iroh 0.98's bidirectional
//! QUIC stream is exactly that: `iroh::endpoint::SendStream` implements
//! `tokio::io::AsyncWrite` and `iroh::endpoint::RecvStream` implements
//! `tokio::io::AsyncRead` (both via the underlying `noq`/iroh-quinn fork). We
//! combine one accepted/opened bidi stream into a single [`IrohDuplex`] that is
//! both halves at once, then:
//!
//!   * feed a `Stream<Item = Result<IrohDuplex, _>>` of accepted streams to
//!     tonic's `Server::serve_with_incoming` (server side), and
//!   * hand a per-call connector that opens a fresh bidi stream to tonic's
//!     `Endpoint::connect_with_connector` (client side).
//!
//! Each gRPC "connection" maps to one Iroh **bidi stream** (not one Iroh
//! connection): tonic multiplexes HTTP/2 over the single byte duplex we give
//! it, and QUIC multiplexes many of those duplexes over one Iroh connection.
//! That is the design choice Task 212 inherits — documented in the findings.
//!
//! This is throwaway measurement code, NOT the production adapter (Task 212).

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use anyhow::{Context as _, Result};
use http::Uri;
use hyper_util::rt::TokioIo;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tonic::transport::server::Connected;

/// One accepted/opened Iroh bidi stream presented as a single
/// `AsyncRead + AsyncWrite` duplex for Tonic.
///
/// Reads are served from the [`RecvStream`] half, writes from the
/// [`SendStream`] half. Both halves are independently pollable, so a naive
/// delegation is correct: HTTP/2 (what Tonic speaks) drives reads and writes
/// concurrently and never blocks one on the other within this type.
pub struct IrohDuplex {
    send: SendStream,
    recv: RecvStream,
}

impl IrohDuplex {
    pub fn new(send: SendStream, recv: RecvStream) -> Self {
        Self { send, recv }
    }
}

// NOTE: `iroh::endpoint::{SendStream, RecvStream}` each expose an *inherent*
// `poll_*` method as well as the `tokio::io::Async{Read,Write}` trait method of
// the same name. A bare `Pin::new(&mut x).poll_read(..)` resolves to the
// inherent one (wrong error type), so we disambiguate with fully-qualified
// trait syntax. This shadowing is the single sharpest piece of adapter friction
// — flagged in the findings for Task 212.
impl AsyncRead for IrohDuplex {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        AsyncRead::poll_read(Pin::new(&mut self.recv), cx, buf)
    }
}

impl AsyncWrite for IrohDuplex {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        AsyncWrite::poll_write(Pin::new(&mut self.send), cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.send), cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.send), cx)
    }
}

/// Tonic requires every incoming IO to carry connection info. The spike does
/// not key any behavior off the peer, so `()` is sufficient — but the impl is
/// mandatory for `serve_with_incoming`.
impl Connected for IrohDuplex {
    type ConnectInfo = ();

    fn connect_info(&self) -> Self::ConnectInfo {}
}

/// Client-side connector: a tower `Service<Uri>` that, for each gRPC channel
/// connect, opens a fresh bidi stream on the shared Iroh [`Connection`] and
/// hands Tonic the resulting duplex (wrapped in `TokioIo` so it satisfies
/// hyper's `rt::Read + rt::Write`, which is what `connect_with_connector`
/// expects). A tiny zero-byte priming write is sent so the server's
/// `accept_bi()` resolves promptly — Iroh defers surfacing a peer-opened bidi
/// stream to the acceptor until the opener writes.
#[derive(Clone)]
pub struct IrohConnector {
    conn: Connection,
}

impl IrohConnector {
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }
}

impl tower::Service<Uri> for IrohConnector {
    type Response = TokioIo<IrohDuplex>;
    type Error = io::Error;
    type Future =
        Pin<Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        let conn = self.conn.clone();
        Box::pin(async move {
            let (mut send, recv) = conn.open_bi().await.map_err(io::Error::other)?;
            // Prime the stream so the server's acceptor wakes immediately.
            tokio::io::AsyncWriteExt::flush(&mut send)
                .await
                .map_err(io::Error::other)?;
            Ok(TokioIo::new(IrohDuplex::new(send, recv)))
        })
    }
}

/// Accept the next inbound bidi stream on an Iroh [`Connection`] and present it
/// as an [`IrohDuplex`] ready for `serve_with_incoming`.
pub async fn accept_duplex(conn: &Connection) -> Result<IrohDuplex> {
    let (send, recv) = conn.accept_bi().await.context("accept_bi on iroh conn")?;
    Ok(IrohDuplex::new(send, recv))
}
