//! The `0x04` Webhook channel framing — read/write the FROZEN
//! [`WebhookEnvelope`] over a raw [`IrohDuplex`] (`design/11 §3.4.1`, Task 315).
//!
//! The relay (`crates/relay`) writes an envelope after the `0x04` channel-tag
//! byte; the Core's serve loop reads it off the **raw** (non-Noise) duplex and
//! hands it to the Core-supplied [`WebhookSink`](crate::api::WebhookSink). The
//! framing is five length-prefixed fields (big-endian `u32` length + bytes), in
//! field order `delivery_id, signature_256, event_type, endpoint_id, body`. The
//! `0x04` channel deliberately runs no Noise IK — the authenticity floor is the
//! per-repo HMAC the Core verifies on `body` (`design/11 §3.3`/§3.9).

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::api::{
    IrohDuplex, WebhookAck, WebhookEnvelope, MAX_MESSAGE_SIZE, MAX_WEBHOOK_BODY_SIZE,
};
use crate::error::{Result, TransportError};

/// Write one length-prefixed UTF-8/opaque field: a big-endian `u32` length
/// followed by the bytes.
async fn write_field(duplex: &mut IrohDuplex, bytes: &[u8]) -> Result<()> {
    let len = u32::try_from(bytes.len())
        .map_err(|_| TransportError::Channel("webhook field exceeds u32 length".into()))?;
    duplex
        .write_all(&len.to_be_bytes())
        .await
        .map_err(|e| TransportError::Channel(format!("write webhook field len: {e}")))?;
    duplex
        .write_all(bytes)
        .await
        .map_err(|e| TransportError::Channel(format!("write webhook field: {e}")))?;
    Ok(())
}

/// Read one length-prefixed field, bounding the declared length to `max` before
/// allocating (so a malformed/hostile length cannot make us buffer unboundedly).
async fn read_field(duplex: &mut IrohDuplex, max: usize, what: &str) -> Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    duplex
        .read_exact(&mut len_buf)
        .await
        .map_err(|e| TransportError::Channel(format!("read webhook {what} len: {e}")))?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > max {
        return Err(TransportError::Channel(format!(
            "webhook {what} length {len} exceeds ceiling {max}"
        )));
    }
    let mut buf = vec![0u8; len];
    duplex
        .read_exact(&mut buf)
        .await
        .map_err(|e| TransportError::Channel(format!("read webhook {what}: {e}")))?;
    Ok(buf)
}

/// Read a length-prefixed UTF-8 field and decode it.
async fn read_str_field(duplex: &mut IrohDuplex, max: usize, what: &str) -> Result<String> {
    let bytes = read_field(duplex, max, what).await?;
    String::from_utf8(bytes)
        .map_err(|e| TransportError::Channel(format!("webhook {what} is not UTF-8: {e}")))
}

/// Write the [`WebhookEnvelope`] over the raw duplex (the relay-write side,
/// `design/11 §3.4.1`). The channel-tag byte `0x04` is written separately by the
/// caller (the acceptor-priming write); this writes only the five fields. Flushes
/// at the end so the Core's `accept_bi` wakes and reads a complete frame.
pub async fn write_envelope(duplex: &mut IrohDuplex, env: &WebhookEnvelope) -> Result<()> {
    if env.body.len() > MAX_WEBHOOK_BODY_SIZE {
        return Err(TransportError::Channel(format!(
            "webhook body {} exceeds {MAX_WEBHOOK_BODY_SIZE} ceiling",
            env.body.len()
        )));
    }
    write_field(duplex, env.delivery_id.as_bytes()).await?;
    write_field(duplex, env.signature_256.as_bytes()).await?;
    write_field(duplex, env.event_type.as_bytes()).await?;
    write_field(duplex, env.endpoint_id.as_bytes()).await?;
    write_field(duplex, &env.body).await?;
    duplex
        .flush()
        .await
        .map_err(|e| TransportError::Channel(format!("flush webhook envelope: {e}")))?;
    Ok(())
}

/// Read the [`WebhookEnvelope`] off the raw duplex (the Core-read side,
/// `design/11 §3.4.1`). The channel-tag byte has already been consumed by the
/// demux. The header strings are bounded by [`MAX_MESSAGE_SIZE`] (a header far
/// larger than any real GitHub header is a malformed frame); `body` is bounded by
/// the FROZEN [`MAX_WEBHOOK_BODY_SIZE`] (25 MiB) ceiling **before** the body is
/// read, so an oversized declared length is rejected without buffering it.
pub async fn read_envelope(duplex: &mut IrohDuplex) -> Result<WebhookEnvelope> {
    // Headers are tiny; cap them at the transport message ceiling so a malformed
    // length is rejected, while never being the real validator for the body.
    let delivery_id = read_str_field(duplex, MAX_MESSAGE_SIZE, "delivery_id").await?;
    let signature_256 = read_str_field(duplex, MAX_MESSAGE_SIZE, "signature_256").await?;
    let event_type = read_str_field(duplex, MAX_MESSAGE_SIZE, "event_type").await?;
    let endpoint_id = read_str_field(duplex, MAX_MESSAGE_SIZE, "endpoint_id").await?;
    let body = read_field(duplex, MAX_WEBHOOK_BODY_SIZE, "body").await?;
    Ok(WebhookEnvelope {
        delivery_id,
        signature_256,
        event_type,
        endpoint_id,
        body,
    })
}

/// Write the single-byte ack the relay maps to an HTTP status (`design/11
/// §3.4.1`) and flush it.
pub async fn write_ack(duplex: &mut IrohDuplex, ack: WebhookAck) -> Result<()> {
    duplex
        .write_all(&[ack.as_byte()])
        .await
        .map_err(|e| TransportError::Channel(format!("write webhook ack: {e}")))?;
    duplex
        .flush()
        .await
        .map_err(|e| TransportError::Channel(format!("flush webhook ack: {e}")))?;
    Ok(())
}

/// Read the single-byte ack the Core wrote (the relay-read side). Any byte other
/// than the three FROZEN values, or an early EOF, surfaces as an error the relay
/// maps to a `5xx` + drop + log.
pub async fn read_ack(duplex: &mut IrohDuplex) -> Result<WebhookAck> {
    let mut b = [0u8; 1];
    duplex
        .read_exact(&mut b)
        .await
        .map_err(|e| TransportError::Channel(format!("read webhook ack: {e}")))?;
    match b[0] {
        0x00 => Ok(WebhookAck::Accepted),
        0x01 => Ok(WebhookAck::Reject),
        0x02 => Ok(WebhookAck::Error),
        other => Err(TransportError::Channel(format!(
            "unknown webhook ack byte 0x{other:02x}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use crate::api::{WebhookAck, WebhookEnvelope};

    #[test]
    fn ack_bytes_are_frozen() {
        assert_eq!(WebhookAck::Accepted.as_byte(), 0x00);
        assert_eq!(WebhookAck::Reject.as_byte(), 0x01);
        assert_eq!(WebhookAck::Error.as_byte(), 0x02);
    }

    #[test]
    fn envelope_round_trips_in_memory() {
        // A pure framing round-trip over an in-memory duplex (no Iroh needed):
        // serialize the five fields with the FROZEN big-endian length prefixes and
        // parse them back. Mirrors the wire bytes `write_envelope`/`read_envelope`
        // move so the framing is provable without standing up two endpoints.
        let env = WebhookEnvelope {
            delivery_id: "12345678-aaaa-bbbb-cccc-ddddeeeeffff".into(),
            signature_256: "sha256=deadbeef".into(),
            event_type: "check_run".into(),
            endpoint_id: "abc123".into(),
            body: b"{\"hello\":\"world\"}".to_vec(),
        };
        let mut buf = Vec::new();
        for f in [
            env.delivery_id.as_bytes(),
            env.signature_256.as_bytes(),
            env.event_type.as_bytes(),
            env.endpoint_id.as_bytes(),
            &env.body,
        ] {
            buf.extend_from_slice(&(f.len() as u32).to_be_bytes());
            buf.extend_from_slice(f);
        }

        // Parse back with the same big-endian length discipline.
        let mut pos = 0usize;
        let mut take = || {
            let len = u32::from_be_bytes(buf[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            let out = buf[pos..pos + len].to_vec();
            pos += len;
            out
        };
        assert_eq!(take(), env.delivery_id.as_bytes());
        assert_eq!(take(), env.signature_256.as_bytes());
        assert_eq!(take(), env.event_type.as_bytes());
        assert_eq!(take(), env.endpoint_id.as_bytes());
        assert_eq!(take(), env.body);
        assert_eq!(pos, buf.len());
    }
}
