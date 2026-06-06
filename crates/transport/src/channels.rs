//! The three logical channels and their on-stream channel-tag framing
//! (`design/11 §3.3`, Task 212).
//!
//! A paired (Core, Device) pair drives **three logical channels** over the one
//! Iroh endpoint, each an Iroh QUIC **bidi stream**:
//!
//! 1. **API** — the long-lived gRPC traffic pool (the same Tonic services UDS
//!    serves). One gRPC connection == one Iroh bidi stream; the API channel opens
//!    many per Iroh `Connection`.
//! 2. **Push-hint** — lightweight, opt-in; the wakeup-fetch channel Task 217's
//!    `send_wakeup_hint` + `design/14` use.
//! 3. **Pairing** — short-lived, once per device, gated by the pairing token
//!    (`design/12 §3.3` / Task 207). Surfaced via `listen_pairing(token_hash)`.
//!
//! # Channel-tag framing — FROZEN wire contract
//!
//! Every opened bidi stream begins with a **single channel-tag byte** the opener
//! writes and the acceptor reads *before* the stream is handed to Tonic /
//! pairing / push-hint handling. This is what lets one Iroh endpoint multiplex
//! all three channels: the acceptor demultiplexes on the first byte.
//!
//! ```text
//! byte 0x01 → API        (gRPC over the Noise-wrapped adapter)
//! byte 0x02 → PushHint   (wakeup-fetch)
//! byte 0x03 → Pairing    (Noise XX over the one-shot token — Task 207 drives)
//! byte 0x04 → Webhook    (relay-originated inbound webhook — NO Noise; Task 315)
//! ```
//!
//! The `0x04` Webhook channel (`design/11 §3.4.1`) is deliberately **non-Noise**:
//! the peer is GitHub-via-relay, not a paired device, so no device cert / Noise
//! IK handshake exists. Its authenticity floor is the per-repo HMAC the Core
//! verifies on the body; the serve loop reads the `WebhookEnvelope` off the
//! **raw** duplex and hands it to the Core's `WebhookSink` (Task 315).
//!
//! The tag byte doubles as the **acceptor-priming** write (spike gotcha #3):
//! writing it immediately wakes the server's `accept_bi()` without waiting for
//! the first HTTP/2 frame. (The spike sent a zero-byte flush; here the one-byte
//! tag *is* the priming write.)
//!
//! The [`ChannelTag`](crate::api::ChannelTag) enum + its `from_byte` decoder are
//! declared in [`crate::api`] (the frozen surface); this module holds the
//! decoder body + the message-size ceiling.

use crate::error::{Result, TransportError};

/// The gRPC message-size ceiling the adapter lifts Tonic's default 4 MiB
/// decode/encode limit to on **both** ends (spike gotcha #4, `design/11
/// §3.1.1`). **FROZEN at 64 MiB**.
pub const MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

/// Max plaintext bytes per Noise frame: a Noise transport message is ≤ 65535 B
/// incl. the 16-byte AEAD tag, so the [`NoiseDuplex`](crate::api::NoiseDuplex)
/// chunks each write into ≤ this many plaintext bytes (round 64000, comfortably
/// under the cap; matches the 208 "≤ 64 KiB Noise frames" intent).
pub const NOISE_PLAINTEXT_CHUNK: usize = 64_000;

/// Decode a channel tag from its wire byte (the body of
/// [`ChannelTag::from_byte`](crate::api::ChannelTag::from_byte)). Unknown bytes
/// are a protocol error.
pub(crate) fn tag_from_byte(b: u8) -> Result<crate::api::ChannelTag> {
    use crate::api::ChannelTag;
    match b {
        0x01 => Ok(ChannelTag::Api),
        0x02 => Ok(ChannelTag::PushHint),
        0x03 => Ok(ChannelTag::Pairing),
        0x04 => Ok(ChannelTag::Webhook),
        other => Err(TransportError::Channel(format!(
            "unknown channel tag byte 0x{other:02x}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use crate::api::ChannelTag;
    use crate::channels::{MAX_MESSAGE_SIZE, NOISE_PLAINTEXT_CHUNK};

    #[test]
    fn tag_bytes_are_frozen() {
        assert_eq!(ChannelTag::Api.as_byte(), 0x01);
        assert_eq!(ChannelTag::PushHint.as_byte(), 0x02);
        assert_eq!(ChannelTag::Pairing.as_byte(), 0x03);
        // Task 315: the FROZEN `0x04` Webhook tag joins `0x01`/`0x02`/`0x03`.
        assert_eq!(ChannelTag::Webhook.as_byte(), 0x04);
    }

    #[test]
    fn tag_roundtrips_and_rejects_unknown() {
        for tag in [
            ChannelTag::Api,
            ChannelTag::PushHint,
            ChannelTag::Pairing,
            ChannelTag::Webhook,
        ] {
            assert_eq!(ChannelTag::from_byte(tag.as_byte()).unwrap(), tag);
        }
        assert!(ChannelTag::from_byte(0x00).is_err());
        assert!(ChannelTag::from_byte(0x05).is_err());
        assert!(ChannelTag::from_byte(0xff).is_err());
    }

    #[test]
    fn size_constants_are_frozen() {
        assert_eq!(MAX_MESSAGE_SIZE, 64 * 1024 * 1024);
        // A Noise transport message is ≤ 65535 B incl. the 16-byte AEAD tag, so
        // the per-frame plaintext chunk must stay ≤ 65519.
        const _: () = assert!(NOISE_PLAINTEXT_CHUNK <= 65_519);
    }
}
