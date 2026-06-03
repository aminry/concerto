//! The hand-rolled tonic-0.12 ↔ Iroh-bidi-stream duplex adapter (Task 212,
//! productionizing `spikes/tonic-iroh/src/iroh_adapter.rs`).
//!
//! The FROZEN type declarations — [`IrohDuplex`](crate::api::IrohDuplex),
//! [`NoiseDuplex`](crate::api::NoiseDuplex),
//! [`IrohConnector`](crate::api::IrohConnector) — live in [`crate::api`] (the
//! interface-generator convention); this module holds their `Async{Read,Write}`
//! / `Connected` / tower `Service` impls and the channel-tag + Noise handshake
//! helpers.
//!
//! # The four spike-102 gotchas (`design/11 §3.1.1`), all applied here
//!
//! 1. **Inherent-vs-trait `poll_*` shadowing** — `SendStream` / `RecvStream`
//!    each expose an *inherent* `poll_write`/`poll_read` AND the
//!    `tokio::io::Async{Write,Read}` trait method. We disambiguate with
//!    fully-qualified trait syntax (`AsyncWrite::poll_write(Pin::new(..), ..)`);
//!    a bare call binds the inherent one (wrong error type) and won't compile.
//! 2. **One gRPC connection == one Iroh bidi stream** — each peer-opened bidi
//!    stream maps to a fresh `serve_with_incoming` with a single-element
//!    incoming stream (the "QUIC stream pool for gRPC", `design/11 §3.3`). The
//!    serve loop lives in [`crate::endpoint`].
//! 3. **Acceptor priming** — the connector writes the channel-tag byte
//!    immediately (the priming write the spike did with a zero-byte flush) so
//!    the server's `accept_bi()` wakes promptly.
//! 4. **Message-size ceilings** — both ends lift Tonic's default 4 MiB
//!    decode/encode limit to [`crate::channels::MAX_MESSAGE_SIZE`] (64 MiB),
//!    applied where the generated services are registered.
//!
//! # Noise layering (`design/11 §6.1` — the spike-deferred integration)
//!
//! Layering order: `Iroh QUIC stream → channel-tag read → Noise IK (208) →
//! tonic adapter → shared dispatch`. The spike's stub had Noise OUT (§3); this
//! is the net-new integration. The decision: the **raw** [`IrohDuplex`] is
//! wrapped by a [`NoiseDuplex`] that runs every Tonic byte through the
//! established [`concerto_identity::NoiseSession`] — Noise wraps the byte duplex
//! *before* it reaches Tonic. `NoiseDuplex` is what `serve_iroh` / the connector
//! hand to Tonic on the API channel; the pairing channel uses the raw duplex
//! (Noise XX is the pairing handshake itself, Task 207).

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::ready;
use http::Uri;
use hyper_util::rt::TokioIo;
use iroh::endpoint::{RecvStream, SendStream};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tonic::transport::server::Connected;

use crate::api::{ChannelTag, IrohConnector, IrohDuplex, NoiseDuplex};
use crate::channels::NOISE_PLAINTEXT_CHUNK;
use crate::error::{Result, TransportError};

impl IrohDuplex {
    /// Wrap an accepted/opened `(send, recv)` bidi-stream pair.
    pub fn new(send: SendStream, recv: RecvStream) -> Self {
        Self { send, recv }
    }

    /// Split into the underlying halves (used by the Noise handshake before
    /// re-joining them under a [`NoiseDuplex`]).
    pub fn into_halves(self) -> (SendStream, RecvStream) {
        (self.send, self.recv)
    }
}

// Gotcha #1: fully-qualified trait syntax to avoid binding the inherent
// `poll_*` methods on `SendStream`/`RecvStream` (wrong error type).
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

/// Tonic requires every incoming IO to carry connection info. The transport keys
/// auth off the device-cert metadata the handler reads (Task 210) and tags the
/// proto `TransportKind::Iroh` in the listener (Task 201), not off this struct,
/// so `()` is sufficient — but the impl is mandatory for `serve_with_incoming`.
impl Connected for IrohDuplex {
    type ConnectInfo = ();
    fn connect_info(&self) -> Self::ConnectInfo {}
}

// ---------------------------------------------------------------------------
// NoiseDuplex
// ---------------------------------------------------------------------------

/// The Noise frame reader's state machine (referenced by
/// [`NoiseDuplex`](crate::api::NoiseDuplex), declared `pub(crate)` so the field
/// can live in `api.rs`).
pub(crate) enum ReadState {
    /// Reading the 2-byte big-endian length prefix.
    Len { buf: [u8; 2], filled: usize },
    /// Reading `buf.len()` ciphertext bytes.
    Body { buf: Vec<u8>, filled: usize },
}

impl NoiseDuplex {
    /// Wrap a raw [`IrohDuplex`] with an established Noise session.
    pub fn new(inner: IrohDuplex, session: concerto_identity::NoiseSession) -> Self {
        Self {
            inner,
            session,
            read_plain: Vec::new(),
            read_plain_pos: 0,
            read_state: ReadState::Len {
                buf: [0u8; 2],
                filled: 0,
            },
            write_buf: Vec::new(),
            write_pos: 0,
        }
    }

    /// Drain the pending ciphertext buffer to the inner stream.
    fn poll_flush_write(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.write_pos < self.write_buf.len() {
            let n = ready!(AsyncWrite::poll_write(
                Pin::new(&mut self.inner),
                cx,
                &self.write_buf[self.write_pos..]
            ))?;
            if n == 0 {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "noise inner write returned 0",
                )));
            }
            self.write_pos += n;
        }
        self.write_buf.clear();
        self.write_pos = 0;
        Poll::Ready(Ok(()))
    }
}

impl Connected for NoiseDuplex {
    type ConnectInfo = ();
    fn connect_info(&self) -> Self::ConnectInfo {}
}

impl AsyncRead for NoiseDuplex {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        loop {
            // Drain buffered plaintext first.
            if this.read_plain_pos < this.read_plain.len() {
                let avail = &this.read_plain[this.read_plain_pos..];
                let n = avail.len().min(buf.remaining());
                buf.put_slice(&avail[..n]);
                this.read_plain_pos += n;
                if this.read_plain_pos == this.read_plain.len() {
                    this.read_plain.clear();
                    this.read_plain_pos = 0;
                }
                return Poll::Ready(Ok(()));
            }

            match &mut this.read_state {
                ReadState::Len { buf: lbuf, filled } => {
                    let mut rb = ReadBuf::new(&mut lbuf[*filled..]);
                    ready!(AsyncRead::poll_read(Pin::new(&mut this.inner), cx, &mut rb))?;
                    let got = rb.filled().len();
                    if got == 0 {
                        // Clean EOF on a frame boundary.
                        return Poll::Ready(Ok(()));
                    }
                    *filled += got;
                    if *filled == 2 {
                        let len = u16::from_be_bytes(*lbuf) as usize;
                        this.read_state = ReadState::Body {
                            buf: vec![0u8; len],
                            filled: 0,
                        };
                    }
                }
                ReadState::Body { buf: bbuf, filled } => {
                    if *filled < bbuf.len() {
                        let mut rb = ReadBuf::new(&mut bbuf[*filled..]);
                        ready!(AsyncRead::poll_read(Pin::new(&mut this.inner), cx, &mut rb))?;
                        let got = rb.filled().len();
                        if got == 0 {
                            return Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "noise frame truncated",
                            )));
                        }
                        *filled += got;
                    }
                    if *filled == bbuf.len() {
                        let ciphertext = std::mem::take(bbuf);
                        this.read_state = ReadState::Len {
                            buf: [0u8; 2],
                            filled: 0,
                        };
                        // Drop-the-connection path (`design/12 §6.3`) on any
                        // AEAD/replay failure.
                        let plain = this
                            .session
                            .decrypt(&ciphertext)
                            .map_err(|e| io::Error::other(TransportError::from(e).to_string()))?;
                        this.read_plain = plain;
                        this.read_plain_pos = 0;
                        // Loop to drain the freshly-decrypted plaintext.
                    }
                }
            }
        }
    }
}

impl AsyncWrite for NoiseDuplex {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        // Flush any pending ciphertext before accepting more plaintext.
        ready!(this.poll_flush_write(cx))?;

        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }

        let chunk = &data[..data.len().min(NOISE_PLAINTEXT_CHUNK)];
        let frame = this
            .session
            .encrypt(chunk)
            .map_err(|e| io::Error::other(TransportError::from(e).to_string()))?;
        let len: u16 = frame
            .len()
            .try_into()
            .map_err(|_| io::Error::other("noise frame exceeds u16 length prefix"))?;
        this.write_buf.clear();
        this.write_pos = 0;
        this.write_buf.extend_from_slice(&len.to_be_bytes());
        this.write_buf.extend_from_slice(&frame);

        // Try to flush immediately; partial is fine (the plaintext is reported
        // consumed and the ciphertext finishes on later flushes).
        let _ = this.poll_flush_write(cx)?;
        Poll::Ready(Ok(chunk.len()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        ready!(this.poll_flush_write(cx))?;
        AsyncWrite::poll_flush(Pin::new(&mut this.inner), cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = &mut *self;
        ready!(this.poll_flush_write(cx))?;
        AsyncWrite::poll_shutdown(Pin::new(&mut this.inner), cx)
    }
}

// ---------------------------------------------------------------------------
// IrohConnector (client side)
// ---------------------------------------------------------------------------

impl tower::Service<Uri> for IrohConnector {
    type Response = TokioIo<NoiseDuplex>;
    type Error = io::Error;
    type Future = Pin<
        Box<
            dyn std::future::Future<Output = std::result::Result<Self::Response, Self::Error>>
                + Send,
        >,
    >;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<std::result::Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        let conn = self.conn.clone();
        let local = self.local_static.clone();
        let remote_pub = self.remote_static_pub;
        Box::pin(async move {
            let (send, recv) = conn.open_bi().await.map_err(io::Error::other)?;
            let duplex = IrohDuplex::new(send, recv);
            // Channel-tag byte = the acceptor-priming write (gotcha #3): one byte
            // wakes the server's accept_bi immediately AND demuxes the channel.
            let duplex = write_channel_tag(duplex, ChannelTag::Api)
                .await
                .map_err(io::Error::from)?;
            // Noise IK initiator handshake inside the stream.
            let noise = handshake_initiator(duplex, &local, &remote_pub)
                .await
                .map_err(io::Error::from)?;
            Ok(TokioIo::new(noise))
        })
    }
}

// ---------------------------------------------------------------------------
// Channel-tag + Noise handshake helpers (run over the raw duplex)
// ---------------------------------------------------------------------------

/// Write the channel-tag byte at the head of a freshly-opened stream and flush
/// it (the acceptor-priming write). Returns the duplex for chaining.
pub async fn write_channel_tag(mut duplex: IrohDuplex, tag: ChannelTag) -> Result<IrohDuplex> {
    duplex
        .write_all(&[tag.as_byte()])
        .await
        .map_err(|e| TransportError::Channel(format!("write tag: {e}")))?;
    duplex
        .flush()
        .await
        .map_err(|e| TransportError::Channel(format!("flush tag: {e}")))?;
    Ok(duplex)
}

/// Read the channel-tag byte the opener wrote, returning the decoded tag plus
/// the duplex for the matching handler.
pub async fn read_channel_tag(mut duplex: IrohDuplex) -> Result<(ChannelTag, IrohDuplex)> {
    let mut b = [0u8; 1];
    duplex
        .read_exact(&mut b)
        .await
        .map_err(|e| TransportError::Channel(format!("read tag: {e}")))?;
    let tag = ChannelTag::from_byte(b[0])?;
    Ok((tag, duplex))
}

/// Run the Noise IK **initiator** handshake (device side) over a raw duplex and
/// return it re-wrapped under the established session.
///
/// Drives the two IK messages (`-> e, es, s, ss` / `<- e, ee, se`) directly over
/// the byte stream with length-prefixed framing, then composes the session with
/// the duplex.
pub async fn handshake_initiator(
    mut duplex: IrohDuplex,
    local: &concerto_identity::NoiseStatic,
    remote_static_pub: &[u8; 32],
) -> Result<NoiseDuplex> {
    let mut hs = concerto_identity::NoiseIkHandshake::initiator(local, remote_static_pub)?;
    let m1 = hs.write_message(&[])?;
    write_hs_frame(&mut duplex, &m1).await?;
    let m2 = read_hs_frame(&mut duplex).await?;
    hs.read_message(&m2)?;
    if !hs.is_handshake_finished() {
        return Err(TransportError::Noise(
            "IK initiator handshake unfinished after two messages".into(),
        ));
    }
    let session = hs.into_session(std::time::Instant::now())?;
    Ok(NoiseDuplex::new(duplex, session))
}

/// Run the Noise IK **responder** handshake (Core side) over a raw duplex and
/// return it re-wrapped under the established session.
pub async fn handshake_responder(
    mut duplex: IrohDuplex,
    local: &concerto_identity::NoiseStatic,
) -> Result<NoiseDuplex> {
    let mut hs = concerto_identity::NoiseIkHandshake::responder(local)?;
    let m1 = read_hs_frame(&mut duplex).await?;
    hs.read_message(&m1)?;
    let m2 = hs.write_message(&[])?;
    write_hs_frame(&mut duplex, &m2).await?;
    if !hs.is_handshake_finished() {
        return Err(TransportError::Noise(
            "IK responder handshake unfinished after two messages".into(),
        ));
    }
    let session = hs.into_session(std::time::Instant::now())?;
    Ok(NoiseDuplex::new(duplex, session))
}

/// Write a length-prefixed handshake frame (`u16` BE length + bytes).
async fn write_hs_frame(duplex: &mut IrohDuplex, msg: &[u8]) -> Result<()> {
    let len: u16 = msg
        .len()
        .try_into()
        .map_err(|_| TransportError::Noise("handshake message exceeds u16".into()))?;
    duplex.write_all(&len.to_be_bytes()).await?;
    duplex.write_all(msg).await?;
    duplex.flush().await?;
    Ok(())
}

/// Read a length-prefixed handshake frame.
async fn read_hs_frame(duplex: &mut IrohDuplex) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 2];
    duplex.read_exact(&mut len_buf).await?;
    let len = u16::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    duplex.read_exact(&mut buf).await?;
    Ok(buf)
}
