//! Device pairing over the `0x03` channel (Task 509, design/16 — the sixth
//! primitive 511 consumes).
//!
//! This is `tools/pair-dial`'s `pair_over_iroh` ported **exactly**: connect the
//! ALPN, `open_bi`, wrap an [`IrohDuplex`], write the FROZEN `0x03` Pairing
//! channel tag, run the Noise XX initiator handshake over the one-shot token,
//! then send the encrypted `PairingRequest` and read back the encrypted
//! `SignedDeviceCert`. The wire layout — `device_pubkey(32) || nonce(32) ||
//! signature(64) || device_name` with `signature = device_key.sign(token ||
//! nonce || device_pubkey)`, and 4-byte-BE length-prefixed framing — is the
//! frozen contract the Core's pairing responder locks (Task 217.5).

use concerto_identity::{KeyPair, NoiseHandshake};
use concerto_transport::api::write_channel_tag;
use concerto_transport::{ChannelTag, IrohDuplex, ALPN};
use iroh::{Endpoint, EndpointAddr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::IrohFfiError;

/// `device_pubkey(32) || nonce(32) || signature(64) || device_name(utf8)` — the
/// encrypted `PairingRequest` body the Core decodes (Task 217.5 framing, ported
/// verbatim from pair-dial).
pub fn encode_pairing_request(
    device_pubkey: &[u8; 32],
    nonce: &[u8; 32],
    signature: &[u8; 64],
    device_name: &str,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(128 + device_name.len());
    out.extend_from_slice(device_pubkey);
    out.extend_from_slice(nonce);
    out.extend_from_slice(signature);
    out.extend_from_slice(device_name.as_bytes());
    out
}

/// The signature input the device signs: `token(32) || nonce(32) ||
/// device_pubkey(32)` (Task 217.5). Broken out so the byte-layout test can
/// assert the ordering against hand-built expected bytes.
pub fn pairing_signature_input(
    token: &[u8; 32],
    nonce: &[u8; 32],
    device_pubkey: &[u8; 32],
) -> Vec<u8> {
    let mut payload = Vec::with_capacity(96);
    payload.extend_from_slice(token);
    payload.extend_from_slice(nonce);
    payload.extend_from_slice(device_pubkey);
    payload
}

/// Run the Noise-XX pairing handshake over the `0x03` channel and return the
/// on-wire signed device cert (`cert_bytes || sig`). A `<= 1`-byte reply is a
/// refusal. Ported verbatim from `tools/pair-dial`'s `pair_over_iroh`.
pub async fn pair_over_iroh(
    client_ep: &Endpoint,
    server_addr: &EndpointAddr,
    token: &[u8; 32],
    device_key: &KeyPair,
    device_pubkey: &[u8; 32],
    nonce: &[u8; 32],
    device_name: &str,
) -> Result<Vec<u8>, IrohFfiError> {
    let conn = client_ep
        .connect(server_addr.clone(), ALPN)
        .await
        .map_err(|e| IrohFfiError::Pairing(format!("pair connect: {e}")))?;
    let (send, recv) = conn
        .open_bi()
        .await
        .map_err(|e| IrohFfiError::Pairing(format!("open bidi: {e}")))?;
    let duplex = IrohDuplex::new(send, recv);
    let mut duplex = write_channel_tag(duplex, ChannelTag::Pairing)
        .await
        .map_err(|e| IrohFfiError::Pairing(format!("write 0x03 tag: {e}")))?;

    // Noise XX initiator over the one-shot token.
    let mut hs = NoiseHandshake::initiator(token)
        .map_err(|e| IrohFfiError::Pairing(format!("xx initiator: {e}")))?;
    let m1 = hs
        .write_message(&[])
        .map_err(|e| IrohFfiError::Pairing(format!("m1: {e}")))?;
    write_frame(&mut duplex, &m1).await?;
    let m2 = read_frame(&mut duplex).await?;
    hs.read_message(&m2)
        .map_err(|e| IrohFfiError::Pairing(format!("read m2: {e}")))?;
    let m3 = hs
        .write_message(&[])
        .map_err(|e| IrohFfiError::Pairing(format!("m3: {e}")))?;
    write_frame(&mut duplex, &m3).await?;
    let mut noise = hs
        .into_transport()
        .map_err(|e| IrohFfiError::Pairing(format!("xx transport: {e}")))?;

    // Sign `token || nonce || device_pubkey`, send the encrypted request.
    let payload = pairing_signature_input(token, nonce, device_pubkey);
    let signature = device_key.sign(&payload).to_bytes();
    let req = encode_pairing_request(device_pubkey, nonce, &signature, device_name);
    let ct = noise
        .write_message(&req)
        .map_err(|e| IrohFfiError::Pairing(format!("encrypt request: {e}")))?;
    write_frame(&mut duplex, &ct).await?;

    // Read the encrypted signed cert (a refusal would be a single byte).
    let reply_ct = read_frame(&mut duplex).await?;
    let signed_cert = noise
        .read_message(&reply_ct)
        .map_err(|e| IrohFfiError::Pairing(format!("decrypt cert reply: {e}")))?;
    if signed_cert.len() <= 1 {
        return Err(IrohFfiError::Pairing(
            "pairing refused (single-byte reply, not a cert)".to_string(),
        ));
    }
    Ok(signed_cert)
}

/// 4-byte-BE length + body — the `0x03`-channel framing the Core's pairing
/// responder locks (Task 217.5).
async fn write_frame(duplex: &mut IrohDuplex, bytes: &[u8]) -> Result<(), IrohFfiError> {
    duplex
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await
        .map_err(|e| IrohFfiError::Pairing(format!("pair: write len: {e}")))?;
    duplex
        .write_all(bytes)
        .await
        .map_err(|e| IrohFfiError::Pairing(format!("pair: write body: {e}")))?;
    duplex
        .flush()
        .await
        .map_err(|e| IrohFfiError::Pairing(format!("pair: flush: {e}")))?;
    Ok(())
}

async fn read_frame(duplex: &mut IrohDuplex) -> Result<Vec<u8>, IrohFfiError> {
    let mut len = [0u8; 4];
    duplex
        .read_exact(&mut len)
        .await
        .map_err(|e| IrohFfiError::Pairing(format!("pair: read len: {e}")))?;
    let n = u32::from_be_bytes(len) as usize;
    let mut buf = vec![0u8; n];
    duplex
        .read_exact(&mut buf)
        .await
        .map_err(|e| IrohFfiError::Pairing(format!("pair: read body: {e}")))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The PairingRequest byte layout + the signature-input ordering must match
    /// the frozen wire (asserted against hand-built expected bytes).
    #[test]
    fn pairing_request_byte_layout_is_frozen() {
        let device_pubkey = [0xAAu8; 32];
        let nonce = [0xBBu8; 32];
        let signature = [0xCCu8; 64];
        let name = "Test Device";

        let encoded = encode_pairing_request(&device_pubkey, &nonce, &signature, name);

        // Hand-build the expected bytes: pubkey(32) || nonce(32) || sig(64) ||
        // name(utf8) — in EXACTLY this order, no separators, no length prefixes.
        let mut expected = Vec::new();
        expected.extend_from_slice(&device_pubkey); // [0..32)
        expected.extend_from_slice(&nonce); // [32..64)
        expected.extend_from_slice(&signature); // [64..128)
        expected.extend_from_slice(name.as_bytes()); // [128..)
        assert_eq!(encoded, expected);

        // Field-boundary assertions (defends against silent reordering).
        assert_eq!(&encoded[0..32], &device_pubkey, "pubkey is bytes [0..32)");
        assert_eq!(&encoded[32..64], &nonce, "nonce is bytes [32..64)");
        assert_eq!(&encoded[64..128], &signature[..], "sig is bytes [64..128)");
        assert_eq!(&encoded[128..], name.as_bytes(), "name is the tail");
        assert_eq!(encoded.len(), 128 + name.len());
    }

    /// The signature INPUT ordering is `token || nonce || device_pubkey` (a
    /// DIFFERENT order from the request body, which is pubkey-first). Asserting
    /// both guards against accidentally signing the request-body order.
    #[test]
    fn pairing_signature_input_ordering_is_frozen() {
        let token = [0x11u8; 32];
        let nonce = [0x22u8; 32];
        let device_pubkey = [0x33u8; 32];

        let input = pairing_signature_input(&token, &nonce, &device_pubkey);

        let mut expected = Vec::new();
        expected.extend_from_slice(&token); // [0..32)
        expected.extend_from_slice(&nonce); // [32..64)
        expected.extend_from_slice(&device_pubkey); // [64..96)
        assert_eq!(input, expected);
        assert_eq!(input.len(), 96);
        assert_eq!(&input[0..32], &token, "signature input is TOKEN-first");
        assert_eq!(&input[32..64], &nonce);
        assert_eq!(&input[64..96], &device_pubkey);
    }
}
