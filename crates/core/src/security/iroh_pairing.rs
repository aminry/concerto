//! Core-side **Noise-XX pairing responder** over the Iroh `0x03` pairing
//! channel (`design/11 §3.3`, `design/12 §3.3`, Task 217.5).
//!
//! Task 207 built the [`PairingCoordinator`] (token mint → signature verify →
//! cert issuance → `devices` INSERT) and proved it over a synthetic
//! `tokio::io::duplex` (`crates/core/tests/pairing.rs`). Task 212 built the
//! transport's [`ChannelTag::Pairing`](concerto_transport::ChannelTag) (`0x03`)
//! routing + the [`PairingListener`] that hands the Core a raw
//! [`IrohDuplex`](concerto_transport)-backed byte stream per inbound pairing
//! attempt. **Both deferred wiring the two together onto a booted Core.** This
//! module is that glue: it drives the Noise XX **responder** over the real
//! `0x03` stream and feeds the decrypted [`CompletePairingInput`] to the
//! coordinator, then writes the minted [`SignedDeviceCert`] back to the device.
//!
//! # The on-wire pairing framing (locked by Task 217.5)
//!
//! Task 207 froze the Noise XX handshake (`Noise_XXpsk3_25519_AESGCM_SHA256`
//! over the 32-byte token) and the signed-payload layout
//! (`pairing_token || nonce || device_pubkey`). It did **not** freeze how the
//! `PairingRequest` is framed on the wire — the loopback double invented its own
//! length-prefixed framing. This task locks the framing the **Core answers** on
//! the `0x03` channel (the contract Task 220 + mobile/web pairing build against):
//!
//! 1. Both ends run the three Noise XX messages, each as a **4-byte big-endian
//!    length prefix + bytes** frame (matching the loopback double's framing).
//! 2. The device sends one encrypted frame: the `PairingRequest`, laid out
//!    `device_pubkey(32) || nonce(32) || signature(64) || device_name(utf8)`.
//! 3. The Core replies with one encrypted frame: the on-wire signed cert
//!    (`cert_bytes || signature`) on success, or a single `0x00` byte on any
//!    rejection (so the device sees a clean "pairing refused" rather than a
//!    hang).
//!
//! # Single-listener model (`design/11 §5.1`)
//!
//! [`IrohTransport::listen_pairing`](concerto_transport) keeps **one** listener,
//! gated on one token hash, replacing any prior. So the responder tracks the
//! most-recently-started pairing's raw token as the active one and runs the
//! Noise XX responder with it; the coordinator still enforces the ≤3-active /
//! one-shot / 60 s-TTL token rules at `complete_pairing`. Concurrent pairings are
//! last-wins at the listener, matching the transport's replace-on-`listen_pairing`
//! semantics.

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use concerto_identity::NoiseHandshake;
use concerto_transport::{IrohDuplex, IrohTransport, PairingListener};

use crate::security::pairing::{CompletePairingInput, PairingChallenge, PairingCoordinator};
use concerto_error::{Error, Result};

/// The single-byte "pairing refused" reply the Core writes back on any rejection
/// (bad signature, expired/consumed token, issue failure). **FROZEN** —
/// distinguishable from a valid cert (which is always ≥ the cert-CBOR length, far
/// longer than one byte) so the device can surface a clean error.
pub const PAIRING_REFUSED_BYTE: u8 = 0x00;

/// Drives the Core-side Noise XX pairing handshake over the Iroh `0x03` channel,
/// bridging it to the Task-207 [`PairingCoordinator`].
///
/// Built once at boot from the live [`IrohTransport`] + the boot
/// [`PairingCoordinator`] (`design/11 §5.1`'s single-listener model). Each
/// [`Self::start_pairing`] mints a token, opens (replaces) the transport's `0x03`
/// listener for it, and spawns a fresh accept task driving the Noise XX over the
/// raw token — aborting any prior pairing's task so the replace-on-`listen_pairing`
/// semantics are honoured. Held behind an `Arc` so the `Devices.StartPairing`
/// handler wiring (Task 220) drives one shared responder.
pub struct IrohPairingResponder {
    transport: Arc<IrohTransport>,
    coordinator: Arc<PairingCoordinator>,
    /// Root shutdown token (the runtime's); every per-pairing accept task is a
    /// child so they all tear down with Core.
    shutdown: CancellationToken,
    /// The currently-armed pairing's accept task — aborted + replaced by the next
    /// `start_pairing` (single-listener model). `None` until the first pairing.
    accept_task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl IrohPairingResponder {
    /// Build the responder over the live transport + the boot coordinator, tied
    /// to the runtime `shutdown` token.
    pub fn new(
        transport: Arc<IrohTransport>,
        coordinator: Arc<PairingCoordinator>,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            transport,
            coordinator,
            shutdown,
            accept_task: std::sync::Mutex::new(None),
        }
    }

    /// Mint a pairing token (via the coordinator), open (replace) the transport's
    /// `0x03` listener for it, and spawn the accept task that runs the Noise XX
    /// responder over the raw token. Returns the QR [`PairingChallenge`] the
    /// `Devices.StartPairing` handler maps to proto.
    ///
    /// This is the seam the runtime pairing-start path (Task 220) drives instead
    /// of the bare `PairingCoordinator::start_pairing`, so starting a pairing also
    /// arms the Iroh listener. The coordinator owns the token rules (≤3-active /
    /// one-shot / 60 s TTL); this only arms the listener + the accept task.
    pub fn start_pairing(self: &Arc<Self>) -> Result<PairingChallenge> {
        let challenge = self.coordinator.start_pairing()?;
        let token = challenge.pairing_token;
        // Open (replace) the transport's single pairing listener, gated on the
        // token hash; the accept task runs Noise XX with the raw token.
        let listener = self.transport.listen_pairing(blake2b_token_hash(&token));
        let responder = Arc::clone(self);
        let shutdown = self.shutdown.clone();
        let task = tokio::spawn(async move {
            responder.accept_loop(listener, token, shutdown).await;
        });
        // Abort + replace any prior pairing's accept task (single-listener model).
        if let Some(prev) = self
            .accept_task
            .lock()
            .expect("accept task lock")
            .replace(task)
        {
            prev.abort();
        }
        Ok(challenge)
    }

    /// Close the active pairing listener and abort its accept task. Idempotent.
    pub fn stop_pairing(&self) {
        self.transport.close_pairing();
        if let Some(prev) = self.accept_task.lock().expect("accept task lock").take() {
            prev.abort();
        }
    }

    /// Accept loop for one armed pairing: handle each inbound `0x03` duplex on a
    /// spawned task (so one slow/hostile device cannot stall a retry) using the
    /// armed raw `token`. Ends when the listener closes (replaced/closed) or
    /// `shutdown` fires.
    async fn accept_loop(
        self: Arc<Self>,
        mut listener: PairingListener,
        token: [u8; 32],
        shutdown: CancellationToken,
    ) {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                maybe = listener.accept() => {
                    let Some(duplex) = maybe else { break };
                    let responder = Arc::clone(&self);
                    tokio::spawn(async move {
                        if let Err(err) = responder.handle_one(duplex, token).await {
                            tracing::warn!(%err, "iroh pairing attempt failed");
                        }
                    });
                }
            }
        }
    }

    /// Handle one inbound `0x03` pairing attempt: run the Noise XX responder over
    /// `token`, decode the device's encrypted `PairingRequest`, drive the
    /// coordinator, and write the minted cert (or the refusal byte) back.
    async fn handle_one(&self, mut duplex: IrohDuplex, token: [u8; 32]) -> Result<()> {
        // --- Noise XX responder over the one-shot token --------------------
        let mut hs = NoiseHandshake::responder(&token)
            .map_err(|e| Error::Pairing(format!("pairing.noise_init: {e}")))?;
        // -> e
        let m1 = read_frame(&mut duplex).await?;
        hs.read_message(&m1)
            .map_err(|e| Error::Pairing(format!("pairing.noise_m1: {e}")))?;
        // <- e, ee, s, es
        let m2 = hs
            .write_message(&[])
            .map_err(|e| Error::Pairing(format!("pairing.noise_m2: {e}")))?;
        write_frame(&mut duplex, &m2).await?;
        // -> s, se (psk mixed here)
        let m3 = read_frame(&mut duplex).await?;
        hs.read_message(&m3)
            .map_err(|e| Error::Pairing(format!("pairing.noise_m3: {e}")))?;

        let mut transport = hs
            .into_transport()
            .map_err(|e| Error::Pairing(format!("pairing.noise_transport: {e}")))?;

        // --- Decrypt + decode the device's PairingRequest ------------------
        let ct = read_frame(&mut duplex).await?;
        let req = transport
            .read_message(&ct)
            .map_err(|e| Error::Pairing(format!("pairing.decrypt: {e}")))?;
        // The raw token is NOT carried in the request frame — the device proves
        // possession via the Noise PSK + the signed payload. The coordinator needs
        // it to re-derive `pairing_token || nonce || device_pubkey`, so we stamp
        // the responder's active `token` into the input here.
        let mut input = decode_pairing_request(&req)?;
        input.pairing_token = token.to_vec();

        // --- Drive the coordinator (verify → consume → issue → INSERT) -----
        match self.coordinator.complete_pairing(input).await {
            Ok(outcome) => {
                // Reply with the on-wire signed cert; the device persists it and
                // presents it under `concerto-device-cert` on every connect.
                let reply = transport
                    .write_message(&outcome.signed_device_cert)
                    .map_err(|e| Error::Pairing(format!("pairing.encrypt_cert: {e}")))?;
                write_frame(&mut duplex, &reply).await?;
                let _ = duplex.flush().await;
                tracing::info!("iroh pairing: device cert issued over 0x03 channel");
                Ok(())
            }
            Err(e) => {
                // Send a clean refusal so the device does not hang, then surface
                // the error to the caller (logged). The coordinator already
                // emitted the `DevicePairingFailed` audit.
                if let Ok(reply) = transport.write_message(&[PAIRING_REFUSED_BYTE]) {
                    let _ = write_frame(&mut duplex, &reply).await;
                    let _ = duplex.flush().await;
                }
                Err(e)
            }
        }
    }
}

/// Decode the device's `PairingRequest` frame into the coordinator's input. The
/// FROZEN layout (Task 217.5): `device_pubkey(32) || nonce(32) || signature(64)
/// || device_name(utf8)`. A short/garbled frame is a pairing protocol error.
fn decode_pairing_request(bytes: &[u8]) -> Result<CompletePairingInput> {
    if bytes.len() < 128 {
        return Err(Error::Pairing(format!(
            "pairing.bad_request: {} bytes (expected ≥ 128)",
            bytes.len()
        )));
    }
    let device_pubkey: [u8; 32] = bytes[0..32]
        .try_into()
        .map_err(|_| Error::Pairing("pairing.bad_request_pubkey".to_string()))?;
    let nonce = bytes[32..64].to_vec();
    let signature: [u8; 64] = bytes[64..128]
        .try_into()
        .map_err(|_| Error::Pairing("pairing.bad_request_sig".to_string()))?;
    let device_name = String::from_utf8(bytes[128..].to_vec())
        .map_err(|_| Error::Pairing("pairing.bad_request_name".to_string()))?;
    // `pairing_token` is stamped by the caller from the responder's active token
    // (it rides the Noise PSK, not the request frame).
    Ok(CompletePairingInput {
        device_pubkey,
        device_name,
        nonce,
        signature,
        pairing_token: Vec::new(),
    })
}

/// `BLAKE2b-256(raw_token)` — the listener-gating token hash (mirrors the
/// coordinator's `hash_token`, kept private there).
fn blake2b_token_hash(token: &[u8; 32]) -> [u8; 32] {
    use blake2::digest::consts::U32;
    use blake2::{Blake2b, Digest};
    let mut hasher = Blake2b::<U32>::new();
    hasher.update(token);
    hasher.finalize().into()
}

/// Write a 4-byte big-endian length-prefixed frame (the `0x03`-channel framing
/// locked by Task 217.5, matching the Task-207 loopback double).
async fn write_frame(duplex: &mut IrohDuplex, bytes: &[u8]) -> Result<()> {
    let len: u32 = bytes
        .len()
        .try_into()
        .map_err(|_| Error::Pairing("pairing.frame_too_large".to_string()))?;
    duplex
        .write_all(&len.to_be_bytes())
        .await
        .map_err(|e| Error::Pairing(format!("pairing.write_len: {e}")))?;
    duplex
        .write_all(bytes)
        .await
        .map_err(|e| Error::Pairing(format!("pairing.write_body: {e}")))?;
    duplex
        .flush()
        .await
        .map_err(|e| Error::Pairing(format!("pairing.flush: {e}")))?;
    Ok(())
}

/// Read a 4-byte big-endian length-prefixed frame.
async fn read_frame(duplex: &mut IrohDuplex) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    duplex
        .read_exact(&mut len)
        .await
        .map_err(|e| Error::Pairing(format!("pairing.read_len: {e}")))?;
    let n = u32::from_be_bytes(len) as usize;
    let mut buf = vec![0u8; n];
    duplex
        .read_exact(&mut buf)
        .await
        .map_err(|e| Error::Pairing(format!("pairing.read_body: {e}")))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(device_pubkey: &[u8; 32], nonce: &[u8; 32], sig: &[u8; 64], name: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(device_pubkey);
        out.extend_from_slice(nonce);
        out.extend_from_slice(sig);
        out.extend_from_slice(name.as_bytes());
        out
    }

    #[test]
    fn decode_pairing_request_roundtrips_layout() {
        let device_pubkey = [1u8; 32];
        let nonce = [2u8; 32];
        let sig = [3u8; 64];
        let bytes = frame(&device_pubkey, &nonce, &sig, "Phone");
        let input = decode_pairing_request(&bytes).expect("decode");
        assert_eq!(input.device_pubkey, device_pubkey);
        assert_eq!(input.nonce, nonce.to_vec());
        assert_eq!(input.signature, sig);
        assert_eq!(input.device_name, "Phone");
        // The token is stamped by the caller, not the frame.
        assert!(input.pairing_token.is_empty());
    }

    #[test]
    fn decode_pairing_request_rejects_short_frame() {
        let err = decode_pairing_request(&[0u8; 64]).expect_err("short frame rejected");
        assert!(err.to_string().contains("pairing.bad_request"), "got {err}");
    }

    #[test]
    fn decode_pairing_request_rejects_non_utf8_name() {
        let mut bytes = frame(&[1u8; 32], &[2u8; 32], &[3u8; 64], "");
        bytes.push(0xff); // invalid UTF-8 device-name byte
        let err = decode_pairing_request(&bytes).expect_err("bad name rejected");
        assert!(
            err.to_string().contains("pairing.bad_request_name"),
            "got {err}"
        );
    }

    #[test]
    fn token_hash_is_deterministic_blake2b256() {
        let token = [7u8; 32];
        let a = blake2b_token_hash(&token);
        let b = blake2b_token_hash(&token);
        assert_eq!(a, b, "deterministic");
        assert_ne!(a, [0u8; 32], "a real digest, not the empty placeholder");
        // A different token hashes differently (the listener gating is by hash).
        assert_ne!(a, blake2b_token_hash(&[8u8; 32]));
    }
}
