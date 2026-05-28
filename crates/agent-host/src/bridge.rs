//! Length-prefixed CBOR frame codec for the host-bridge UDS.
//!
//! Wire layout, repeated on each frame:
//!
//! ```text
//! [u32 BE length] [CBOR(HostFrame)]
//! ```
//!
//! The length is the *body* size in bytes — it does not include the
//! length word itself. Frames larger than [`MAX_FRAME_BYTES`] are
//! rejected (the connection is torn down) per the OOM-guard rule in
//! `design/00 §7.3`.
//!
//! Both [`read_frame`] and [`write_frame`] are async and operate on any
//! `AsyncRead`/`AsyncWrite` half — that keeps the protocol layer
//! independent from whether the transport is `tokio::net::UnixStream` or
//! a piped duplex in tests.

use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::api::{HostFrame, MAX_FRAME_BYTES};

/// Errors the frame codec can surface. Distinct from the workspace
/// `Error` enum because the protocol layer needs to differentiate
/// "remote closed" (`Eof`) from "decode failed" (`Decode`) from "frame
/// exceeded cap" (`TooLarge`) — the connection loop above acts
/// differently on each.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// Underlying socket I/O error.
    #[error("frame io: {0}")]
    Io(#[from] io::Error),
    /// Peer closed the connection mid-frame (or before any byte
    /// arrived). Treated as a normal disconnect by the loop above.
    #[error("frame eof")]
    Eof,
    /// Length prefix exceeded [`MAX_FRAME_BYTES`]. The connection is
    /// closed with no further reads.
    #[error("frame too large: {0} bytes (cap {} bytes)", MAX_FRAME_BYTES)]
    TooLarge(usize),
    /// CBOR decode failed on the body bytes.
    #[error("frame decode: {0}")]
    Decode(String),
    /// CBOR encode failed before the body bytes could be written.
    #[error("frame encode: {0}")]
    Encode(String),
}

/// Read a single frame from `reader`. Returns `Err(FrameError::Eof)` if
/// the peer closed cleanly between frames.
pub async fn read_frame<R>(reader: &mut R) -> Result<HostFrame, FrameError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut len_buf = [0u8; 4];
    // Read the length prefix. A clean EOF on the very first byte is the
    // graceful-disconnect path; anything else (partial read followed by
    // EOF) is a protocol error.
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(FrameError::Eof),
        Err(e) => return Err(FrameError::Io(e)),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(len));
    }
    let mut body = vec![0u8; len];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|e| match e.kind() {
            io::ErrorKind::UnexpectedEof => FrameError::Eof,
            _ => FrameError::Io(e),
        })?;
    let frame: HostFrame =
        ciborium::from_reader(body.as_slice()).map_err(|e| FrameError::Decode(e.to_string()))?;
    Ok(frame)
}

/// Encode and write a single frame to `writer`. The writer is *not*
/// flushed automatically — callers that need backpressure-safe delivery
/// (the connection loop) flush after each frame.
pub async fn write_frame<W>(writer: &mut W, frame: &HostFrame) -> Result<(), FrameError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut body = Vec::with_capacity(256);
    ciborium::into_writer(frame, &mut body).map_err(|e| FrameError::Encode(e.to_string()))?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge(body.len()));
    }
    let len = (body.len() as u32).to_be_bytes();
    writer.write_all(&len).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn round_trip_hello() {
        let (mut a, mut b) = duplex(8192);
        let frame = HostFrame::Hello {
            core_version: "test".into(),
            expected_cookie: [7u8; 32],
            last_seq: 42,
        };
        write_frame(&mut a, &frame).await.unwrap();
        let got = read_frame(&mut b).await.unwrap();
        match got {
            HostFrame::Hello {
                core_version,
                expected_cookie,
                last_seq,
            } => {
                assert_eq!(core_version, "test");
                assert_eq!(expected_cookie, [7u8; 32]);
                assert_eq!(last_seq, 42);
            }
            other => panic!("unexpected frame: {other:?}"),
        }
    }

    #[tokio::test]
    async fn eof_on_clean_close() {
        let (a, mut b) = duplex(8192);
        drop(a);
        let err = read_frame(&mut b).await.unwrap_err();
        assert!(matches!(err, FrameError::Eof));
    }

    #[tokio::test]
    async fn rejects_oversize_length() {
        let (mut a, mut b) = duplex(8192);
        // Synthesize a length prefix above MAX_FRAME_BYTES.
        let len = (MAX_FRAME_BYTES as u32 + 1).to_be_bytes();
        a.write_all(&len).await.unwrap();
        a.flush().await.unwrap();
        let err = read_frame(&mut b).await.unwrap_err();
        assert!(matches!(err, FrameError::TooLarge(_)));
    }
}
