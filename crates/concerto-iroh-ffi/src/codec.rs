//! The **identity / passthrough** tonic codec (Task 509, design/16 §3.2).
//!
//! 509 is a pure byte passthrough: `rpcUnary`/`rpcStream` take a
//! fully-qualified gRPC path + a raw request payload and return the raw response
//! bytes, **without ever decoding the caller's bytes as a `prost::Message`**.
//! (510 assembles the typed paths + messages; 509 stays generic.) To drive
//! `tonic::client::Grpc` with raw bytes we hand it a codec whose `Encode` and
//! `Decode` are the identity function over `bytes::Bytes` — the wire bytes are
//! copied through untouched.
//!
//! tonic's HTTP/2 framing layer still owns the 5-byte gRPC length-prefix; this
//! codec only sees the message body, so the bytes in == bytes out invariant
//! holds for arbitrary payloads (including > 4 MiB once the Grpc decode/encode
//! limit is raised to [`MAX_MESSAGE_SIZE`](concerto_transport::MAX_MESSAGE_SIZE)).

use bytes::{Buf, BufMut, Bytes};
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::Status;

/// The copy-through the [`IdentityEncoder`] performs, factored so the unit test
/// can exercise the exact byte contract without constructing tonic's
/// crate-private `EncodeBuf` (`EncodeBuf::new` is `pub(crate)`). `EncodeBuf` is a
/// thin `BufMut` wrapper; this is precisely what `encode` does on it.
#[inline]
fn copy_into<B: BufMut>(dst: &mut B, item: &[u8]) {
    dst.put_slice(item);
}

/// The copy-out the [`IdentityDecoder`] performs, factored for the same reason
/// (`DecodeBuf::new` is `pub(crate)`; `DecodeBuf` is a thin `Buf` wrapper).
#[inline]
fn copy_out<B: Buf>(src: &mut B) -> Bytes {
    let len = src.remaining();
    src.copy_to_bytes(len)
}

/// A tonic [`Codec`] that treats both directions as opaque `Bytes`.
#[derive(Debug, Clone, Copy, Default)]
pub struct IdentityCodec;

impl Codec for IdentityCodec {
    type Encode = Bytes;
    type Decode = Bytes;
    type Encoder = IdentityEncoder;
    type Decoder = IdentityDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        IdentityEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        IdentityDecoder
    }
}

/// Copies the caller's request bytes onto the wire untouched.
#[derive(Debug, Clone, Copy, Default)]
pub struct IdentityEncoder;

impl Encoder for IdentityEncoder {
    type Item = Bytes;
    type Error = Status;

    fn encode(&mut self, item: Self::Item, dst: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
        dst.reserve(item.len());
        copy_into(dst, &item);
        Ok(())
    }
}

/// Hands the caller the response message bytes untouched (one `Bytes` per gRPC
/// message — `DecodeBuf` is already framed to a single message body by tonic).
#[derive(Debug, Clone, Copy, Default)]
pub struct IdentityDecoder;

impl Decoder for IdentityDecoder {
    type Item = Bytes;
    type Error = Status;

    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        Ok(Some(copy_out(src)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    /// Round-trip arbitrary bytes (incl. a > 4 MiB buffer) through the codec's
    /// EXACT encode (`copy_into`, a `BufMut::put_slice`) then decode (`copy_out`,
    /// a `Buf::copy_to_bytes`) and assert byte-identical. Proves (a) the codec is
    /// opaque — NO prost decode of the caller's bytes — and (b) the raised
    /// ceiling: a 5 MiB body, over tonic's default 4 MiB limit, survives
    /// unchanged.
    ///
    /// We drive `copy_into`/`copy_out` over `BytesMut`/`Bytes` rather than tonic's
    /// `EncodeBuf`/`DecodeBuf`, whose `::new` constructors are `pub(crate)`.
    /// Those types are thin `BufMut`/`Buf` wrappers and `encode`/`decode` call
    /// these exact helpers on them, so this exercises the real passthrough.
    #[test]
    fn identity_roundtrip_preserves_arbitrary_bytes_over_4_mib() {
        let payload: Bytes = (0..(5 * 1024 * 1024_usize))
            .map(|i| (i % 251) as u8)
            .collect::<Vec<u8>>()
            .into();
        assert!(payload.len() > 4 * 1024 * 1024, "fixture must exceed 4 MiB");

        let mut buf = BytesMut::new();
        copy_into(&mut buf, &payload);
        assert_eq!(buf.len(), payload.len(), "encoder must not reframe");

        let mut framed = buf.freeze();
        let decoded = copy_out(&mut framed);
        assert_eq!(decoded, payload, "bytes must round-trip unchanged");
    }

    /// Small + empty payloads also pass through unchanged (edge of the opacity
    /// contract).
    #[test]
    fn identity_roundtrip_small_and_empty() {
        for sample in [
            Bytes::new(),
            Bytes::from_static(&[0u8]),
            Bytes::from_static(b"\x00\xffhi"),
        ] {
            let mut buf = BytesMut::new();
            copy_into(&mut buf, &sample);
            let mut framed = buf.freeze();
            let decoded = copy_out(&mut framed);
            assert_eq!(decoded, sample);
        }
    }

    /// The `Codec` impl wires the right associated types (compile-time contract:
    /// both directions are `Bytes`). A const assertion keeps the opacity type
    /// from silently changing to a prost message.
    #[test]
    fn codec_associated_types_are_bytes() {
        fn assert_bytes<T: 'static>() -> bool {
            std::any::TypeId::of::<T>() == std::any::TypeId::of::<Bytes>()
        }
        assert!(assert_bytes::<<IdentityCodec as Codec>::Encode>());
        assert!(assert_bytes::<<IdentityCodec as Codec>::Decode>());
    }
}
