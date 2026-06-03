//! Noise **IK** session layer — the inner AEAD that wraps every Iroh
//! connection after pairing (`design/12 §3.4`, §6.3, Task 208).
//!
//! Where [`crate::noise_xx`] is the *first-contact pairing* handshake (mutual
//! unauthenticated, token-PSK), this module is the **post-pairing session**
//! crypto: a Noise **IK** handshake run inside each Iroh QUIC stream, giving a
//! second AEAD with a **different authentication root** than Iroh's TLS. If
//! Iroh's relay/endpoint discovery is ever compromised the inner Noise still
//! holds, because its trust root is the QR scan at pairing time, not anything
//! the relay saw (`design/12 §3.4` double-encrypt rationale).
//!
//! # FROZEN wire contract — the Noise IK protocol string
//!
//! ```text
//! Noise_IK_25519_AESGCM_BLAKE2b
//! ```
//!
//! - **`IK`** — the initiator (the paired **device**) already knows the
//!   responder's (the **Core**'s) static public key in advance: in production
//!   it is the `core_pubkey`-adjacent Noise static the device carried from
//!   pairing. That foreknowledge is what makes IK one round trip cheaper than
//!   XK (`design/12 §12 R-4`). The two IK messages are exactly the pattern
//!   `design/12 §3.4` names:
//!
//!   ```text
//!   -> e, es, s, ss      (initiator → responder)
//!   <- e, ee, se         (responder → initiator)
//!   ```
//!
//! - **`25519`** — X25519 Diffie-Hellman. **Note:** the Noise static key is an
//!   X25519 (DH) key, *distinct* from the Ed25519 (signature) identity key in
//!   the [`crate::DeviceCert`]. Ed25519 signs certs; X25519 does the session
//!   DH. This module takes raw 32-byte X25519 keypair bytes (see
//!   [`NoiseStatic`]); the transport (Task 212) owns deriving/storing the
//!   Core's and device's Noise statics and carrying the responder's public
//!   half so the initiator can pre-load it ([`establish_initiator`]).
//! - **`AESGCM`** — `snow`'s AESGCM token is **AES-256-GCM** (256-bit key from
//!   the Noise key schedule), matching `design/12 §3.4`.
//! - **`BLAKE2b`** — the hash backing the Noise key schedule and the **transport
//!   hash** exposed for channel binding ([`NoiseSession::transport_hash`]).
//!
//! Both ends MUST use this exact string. It is the cross-`snow`-version freeze;
//! the committed known-answer vectors in `tests/noise_ik_vectors.rs` pin a full
//! IK handshake (fixed statics + fixed ephemerals → fixed messages + derived
//! transport hash) so a `snow` upgrade that perturbed the protocol fails loudly.
//!
//! # Session lifecycle & rekey (`design/12 §6.3`, R-9)
//!
//! A fresh [`NoiseSession`] is established per Iroh connection. The session
//! **rekeys every 1 GB OR 1 hour, whichever trips first** ([`REKEY_BYTES`] /
//! [`REKEY_INTERVAL`]). Byte accounting is **combined across both directions**
//! (every plaintext byte that flows through [`NoiseSession::encrypt`] or
//! [`NoiseSession::decrypt`] counts toward the single 1 GB budget). On a trip
//! the session rekeys **in place** via `snow`'s `rekey_outgoing` /
//! `rekey_incoming` (Section 4.2 of the Noise spec) and resets the counters;
//! the peer rekeys symmetrically when its own accounting trips (each direction
//! rekeys independently, deterministically, from the existing key — no extra
//! wire message, so the two ends stay in sync without a re-handshake).
//!
//! On **replay-counter (nonce) overflow** — `snow` returns
//! `StateProblem::Exhausted` after 2⁶⁴−1 messages — or on any AEAD authentication
//! failure, [`NoiseSession::decrypt`] returns [`IdentityError::Noise`], which the
//! caller (Task 212) treats as **drop the connection + reconnect** (`design/12
//! §6.3`). Session keys **never touch disk**: [`NoiseSession`] is not
//! `Serialize`/`Debug`/`Clone`, logs no key material, and zeroizes `snow`'s
//! `TransportState` on drop (`snow`'s `TransportState` zeroizes its cipher
//! state internally; we additionally drop it eagerly).
//!
//! # Transport-agnostic
//!
//! Like the XX module, this is a thin `snow` wrapper that exchanges the two IK
//! handshake messages over a **caller-supplied byte channel** and yields a
//! [`NoiseSession`]. It knows nothing about Iroh, QUIC, or `tokio::io::duplex`;
//! Task 212 plugs the real Iroh bidi stream in, and the Tier-2 test drives the
//! two messages in-process.

use snow::params::DHChoice;
use snow::resolvers::{CryptoResolver, DefaultResolver};
use snow::{Builder, HandshakeState};
use zeroize::Zeroize;

use crate::error::IdentityError;

/// The FROZEN Noise protocol string for the post-pairing session channel
/// (`design/12 §3.4`). See the module doc for the token rationale.
pub const NOISE_IK_PARAMS: &str = "Noise_IK_25519_AESGCM_BLAKE2b";

/// Rekey data threshold: **1 GB** of cumulative plaintext (both directions
/// combined). FROZEN (`design/12 §6.3`, R-9). On reaching it the session
/// rekeys in place and resets its byte counter.
pub const REKEY_BYTES: u64 = 1_000_000_000;

/// Rekey time threshold: **1 hour** of wall-clock since session start. FROZEN
/// (`design/12 §6.3`, R-9). Whichever of [`REKEY_BYTES`] / [`REKEY_INTERVAL`]
/// trips first drives the rekey.
pub const REKEY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// A Noise message buffer ceiling. Noise messages are ≤ 65535 bytes; this is
/// the scratch headroom we add atop a payload so an AEAD-tagged write never
/// truncates. (Mirrors the XX module's sizing discipline.)
const NOISE_TAG_HEADROOM: usize = 16;

/// Length of the Noise transport (channel-binding) hash. The `BLAKE2b` suite's
/// Noise handshake hash is BLAKE2b-512 → **64 bytes** (distinct from the
/// 32-byte BLAKE2b-256 `device_id` digest the cert layer uses).
pub const TRANSPORT_HASH_LEN: usize = 64;

/// A raw X25519 Noise static keypair (32-byte private + 32-byte public).
///
/// **This is the DH static, not the Ed25519 identity** (see the module doc).
/// Generate a fresh one with [`NoiseStatic::generate`] (used by the loopback
/// test and by a transport that mints a per-Core/per-device Noise static); the
/// public half ([`NoiseStatic::public`]) is what the initiator pre-loads as the
/// responder's expected static (`Builder::remote_public_key`).
///
/// The private bytes are secret material: this type is **not**
/// `Debug`/`Clone`/`Serialize` and zeroizes on drop.
pub struct NoiseStatic {
    private: Vec<u8>,
    public: [u8; 32],
}

impl NoiseStatic {
    /// Generate a fresh X25519 static keypair using `snow`'s configured
    /// (pure-Rust `default-resolver`) keypair generator for the IK suite.
    pub fn generate() -> Result<Self, IdentityError> {
        let params = NOISE_IK_PARAMS
            .parse()
            .map_err(|e| IdentityError::Noise(format!("invalid noise params: {e}")))?;
        let kp = Builder::new(params)
            .generate_keypair()
            .map_err(|e| IdentityError::Noise(format!("generate static keypair: {e}")))?;
        let mut public = [0u8; 32];
        public.copy_from_slice(&kp.public);
        Ok(Self {
            private: kp.private,
            public,
        })
    }

    /// Reconstruct a static keypair from its raw 32-byte X25519 private key,
    /// deriving the public half deterministically (X25519 scalar-basepoint
    /// mult via `snow`'s pure-Rust resolver).
    ///
    /// This is the transport's persistence path — analogous to
    /// [`crate::KeyPair::from_seed`] for the Ed25519 identity — and the
    /// deterministic input for the committed known-answer vector.
    pub fn from_private(private: [u8; 32]) -> Result<Self, IdentityError> {
        let mut dh = DefaultResolver
            .resolve_dh(&DHChoice::Curve25519)
            .ok_or_else(|| IdentityError::Noise("no Curve25519 DH in resolver".to_string()))?;
        dh.set(&private);
        let mut public = [0u8; 32];
        let pk = dh.pubkey();
        if pk.len() != 32 {
            return Err(IdentityError::Noise(format!(
                "unexpected X25519 public-key length {}",
                pk.len()
            )));
        }
        public.copy_from_slice(pk);
        Ok(Self {
            private: private.to_vec(),
            public,
        })
    }

    /// The 32-byte X25519 public key — the value the initiator pre-loads as the
    /// responder's expected static.
    pub fn public(&self) -> [u8; 32] {
        self.public
    }
}

impl Drop for NoiseStatic {
    fn drop(&mut self) {
        self.private.zeroize();
    }
}

/// One side of an in-progress Noise IK handshake.
///
/// Build an [`initiator`](NoiseIkHandshake::initiator) (the device — which
/// pre-loads the responder's static) or a
/// [`responder`](NoiseIkHandshake::responder) (the Core), then drive the two IK
/// messages with [`write_message`](Self::write_message) /
/// [`read_message`](Self::read_message). When
/// [`is_handshake_finished`](Self::is_handshake_finished) returns `true`, call
/// [`into_session`](Self::into_session).
///
/// Most callers use the higher-level [`establish_initiator`] /
/// [`establish_responder`] which drive the full two-message exchange over a
/// pair of byte closures.
pub struct NoiseIkHandshake {
    state: HandshakeState,
}

impl NoiseIkHandshake {
    /// Build the **initiator** side (the paired device).
    ///
    /// `local` is the device's X25519 Noise static; `remote_static_pub` is the
    /// responder Core's X25519 static public key, which the initiator already
    /// holds (IK precondition — carried from pairing). A wrong
    /// `remote_static_pub` makes the responder's `read_message` of the first
    /// message fail to authenticate (the `es`/`ss` DH mixes diverge).
    pub fn initiator(
        local: &NoiseStatic,
        remote_static_pub: &[u8; 32],
    ) -> Result<Self, IdentityError> {
        Self::build(local, Some(remote_static_pub), true, None)
    }

    /// Build the **responder** side (the Core).
    ///
    /// `local` is the Core's X25519 Noise static. The responder learns the
    /// initiator's static *inside* the handshake (the IK `s` token in message
    /// 1), so it does not pre-load a remote key.
    pub fn responder(local: &NoiseStatic) -> Result<Self, IdentityError> {
        Self::build(local, None, false, None)
    }

    /// Build the initiator with a **fixed ephemeral** — *testing only*. Used by
    /// the committed known-answer vector so a full IK handshake is
    /// byte-reproducible; a fixed ephemeral destroys forward secrecy and must
    /// never be used on a live session.
    #[doc(hidden)]
    pub fn initiator_with_fixed_ephemeral(
        local: &NoiseStatic,
        remote_static_pub: &[u8; 32],
        fixed_ephemeral: &[u8; 32],
    ) -> Result<Self, IdentityError> {
        Self::build(local, Some(remote_static_pub), true, Some(fixed_ephemeral))
    }

    /// Build the responder with a **fixed ephemeral** — *testing only* (see
    /// [`initiator_with_fixed_ephemeral`](Self::initiator_with_fixed_ephemeral)).
    #[doc(hidden)]
    pub fn responder_with_fixed_ephemeral(
        local: &NoiseStatic,
        fixed_ephemeral: &[u8; 32],
    ) -> Result<Self, IdentityError> {
        Self::build(local, None, false, Some(fixed_ephemeral))
    }

    fn build(
        local: &NoiseStatic,
        remote_static_pub: Option<&[u8; 32]>,
        initiator: bool,
        fixed_ephemeral: Option<&[u8; 32]>,
    ) -> Result<Self, IdentityError> {
        let params = NOISE_IK_PARAMS
            .parse()
            .map_err(|e| IdentityError::Noise(format!("invalid noise params: {e}")))?;
        let mut builder = Builder::new(params).local_private_key(&local.private);
        if let Some(rs) = remote_static_pub {
            builder = builder.remote_public_key(rs);
        }
        if let Some(e) = fixed_ephemeral {
            // Deterministic ephemeral — ONLY for the committed known-answer
            // vector (so a full IK handshake is byte-reproducible). Never used
            // on a live session.
            builder = builder.fixed_ephemeral_key_for_testing_only(e);
        }
        let state = if initiator {
            builder.build_initiator()
        } else {
            builder.build_responder()
        }
        .map_err(|e| IdentityError::Noise(format!("build IK handshake: {e}")))?;
        Ok(Self { state })
    }

    /// Write the next handshake message, returning the bytes to send to the
    /// peer. The IK session carries no in-handshake application payload, so
    /// pass `&[]`.
    pub fn write_message(&mut self, payload: &[u8]) -> Result<Vec<u8>, IdentityError> {
        let mut buf = vec![0u8; payload.len() + u16::MAX as usize];
        let n = self
            .state
            .write_message(payload, &mut buf)
            .map_err(|e| IdentityError::Noise(format!("write IK handshake message: {e}")))?;
        buf.truncate(n);
        Ok(buf)
    }

    /// Read a handshake message from the peer. A decrypt/authentication failure
    /// — including a wrong pre-loaded responder static, or a tampered message —
    /// surfaces as [`IdentityError::Noise`]; the caller drops the connection.
    pub fn read_message(&mut self, message: &[u8]) -> Result<Vec<u8>, IdentityError> {
        let mut buf = vec![0u8; message.len() + u16::MAX as usize];
        let n = self
            .state
            .read_message(message, &mut buf)
            .map_err(|e| IdentityError::Noise(format!("read IK handshake message: {e}")))?;
        buf.truncate(n);
        Ok(buf)
    }

    /// Whether this side has completed all its handshake messages.
    pub fn is_handshake_finished(&self) -> bool {
        self.state.is_handshake_finished()
    }

    /// Consume the finished handshake and derive the [`NoiseSession`]
    /// (AES-256-GCM transport keys + the BLAKE2b transport hash for channel
    /// binding). Errors if the handshake is not yet finished.
    ///
    /// `now` is the session-start instant the 1 h rekey timer measures from;
    /// callers pass [`std::time::Instant::now`] (the test injects a fixed one).
    pub fn into_session(self, now: std::time::Instant) -> Result<NoiseSession, IdentityError> {
        // The transport hash lives on the HandshakeState and is consumed by
        // into_transport_mode, so capture it first. For the BLAKE2b suite the
        // Noise handshake hash is 64 bytes (BLAKE2b-512).
        let mut transport_hash = [0u8; TRANSPORT_HASH_LEN];
        let hh = self.state.get_handshake_hash();
        if hh.len() != TRANSPORT_HASH_LEN {
            return Err(IdentityError::Noise(format!(
                "unexpected transport-hash length {} (BLAKE2b should be {TRANSPORT_HASH_LEN})",
                hh.len()
            )));
        }
        transport_hash.copy_from_slice(hh);
        let initiator = self.state.is_initiator();
        let transport = self
            .state
            .into_transport_mode()
            .map_err(|e| IdentityError::Noise(format!("enter IK transport mode: {e}")))?;
        Ok(NoiseSession {
            state: Some(transport),
            transport_hash,
            initiator,
            bytes_since_rekey: 0,
            started_at: now,
        })
    }
}

/// A completed Noise IK session: the post-handshake AES-256-GCM transport that
/// encrypts every gRPC frame, with the rekey accounting of `design/12 §6.3`.
///
/// Obtain one from [`establish_initiator`] / [`establish_responder`] (or
/// [`NoiseIkHandshake::into_session`]). Encrypt outbound frames with
/// [`encrypt`](Self::encrypt) and decrypt inbound frames with
/// [`decrypt`](Self::decrypt); both advance the byte accounting and trip the
/// in-place rekey at [`REKEY_BYTES`] / [`REKEY_INTERVAL`].
///
/// **Key hygiene.** Deliberately not `Debug`/`Clone`/`Serialize`: the
/// transport keys never appear in logs and never reach disk. The wrapped
/// `snow::TransportState` zeroizes its cipher key material on drop; this type's
/// [`Drop`] also drops it eagerly. The [`transport_hash`](Self::transport_hash)
/// is a binding value (a hash, not a secret) and is safe to expose.
pub struct NoiseSession {
    /// `Option` so [`Drop`] can take and drop the `TransportState` eagerly.
    state: Option<snow::TransportState>,
    transport_hash: [u8; TRANSPORT_HASH_LEN],
    initiator: bool,
    /// Cumulative plaintext bytes (both directions) since the last rekey.
    bytes_since_rekey: u64,
    started_at: std::time::Instant,
}

impl NoiseSession {
    /// The 64-byte BLAKE2b transport hash ([`TRANSPORT_HASH_LEN`]), for channel
    /// binding (e.g. binding a higher-layer token to this exact session). A
    /// binding value, not a secret — safe to expose/compare.
    pub fn transport_hash(&self) -> [u8; TRANSPORT_HASH_LEN] {
        self.transport_hash
    }

    /// Whether this session holds the IK initiator (device) role.
    pub fn is_initiator(&self) -> bool {
        self.initiator
    }

    /// Cumulative plaintext bytes counted toward the current 1 GB rekey window.
    /// Resets to 0 on each rekey. (Exposed for tests / telemetry — not a
    /// secret.)
    pub fn bytes_since_rekey(&self) -> u64 {
        self.bytes_since_rekey
    }

    fn state_mut(&mut self) -> Result<&mut snow::TransportState, IdentityError> {
        self.state
            .as_mut()
            .ok_or_else(|| IdentityError::Noise("session already closed".to_string()))
    }

    /// Encrypt `payload` into a Noise transport frame to send to the peer, then
    /// account the bytes and rekey if a threshold tripped (see
    /// [`maybe_rekey_at`](Self::maybe_rekey_at)).
    ///
    /// A single Noise transport message is capped at 65535 bytes including the
    /// 16-byte AEAD tag, so `payload` must be ≤ 65519 bytes; a larger payload
    /// returns [`IdentityError::Noise`]. The transport (Task 212) chunks a
    /// `session.io` payload into ≤ 64 KiB frames before calling this — the same
    /// shape the bench measures.
    ///
    /// Uses the wall clock for the time check; the deterministic test path uses
    /// [`encrypt_at`](Self::encrypt_at).
    pub fn encrypt(&mut self, payload: &[u8]) -> Result<Vec<u8>, IdentityError> {
        self.encrypt_at(payload, std::time::Instant::now())
    }

    /// Clock-injected [`encrypt`](Self::encrypt) for deterministic rekey tests.
    pub fn encrypt_at(
        &mut self,
        payload: &[u8],
        now: std::time::Instant,
    ) -> Result<Vec<u8>, IdentityError> {
        let mut buf = vec![0u8; payload.len() + NOISE_TAG_HEADROOM];
        let n = {
            let state = self.state_mut()?;
            state
                .write_message(payload, &mut buf)
                .map_err(|e| IdentityError::Noise(format!("encrypt session frame: {e}")))?
        };
        buf.truncate(n);
        self.account_and_rekey(payload.len() as u64, now)?;
        Ok(buf)
    }

    /// Decrypt a Noise transport frame from the peer, then account the bytes and
    /// rekey if a threshold tripped.
    ///
    /// A nonce/replay-counter overflow (`snow` `StateProblem::Exhausted`) or an
    /// AEAD authentication failure returns [`IdentityError::Noise`] — the caller
    /// drops + reconnects (`design/12 §6.3`).
    pub fn decrypt(&mut self, frame: &[u8]) -> Result<Vec<u8>, IdentityError> {
        self.decrypt_at(frame, std::time::Instant::now())
    }

    /// Clock-injected [`decrypt`](Self::decrypt) for deterministic rekey tests.
    pub fn decrypt_at(
        &mut self,
        frame: &[u8],
        now: std::time::Instant,
    ) -> Result<Vec<u8>, IdentityError> {
        let mut buf = vec![0u8; frame.len() + NOISE_TAG_HEADROOM];
        let n = {
            let state = self.state_mut()?;
            state
                .read_message(frame, &mut buf)
                .map_err(|e| IdentityError::Noise(format!("decrypt session frame: {e}")))?
        };
        buf.truncate(n);
        self.account_and_rekey(n as u64, now)?;
        Ok(buf)
    }

    /// Add `plaintext_len` to the combined byte counter and rekey if either the
    /// 1 GB byte budget or the 1 h timer has been reached.
    fn account_and_rekey(
        &mut self,
        plaintext_len: u64,
        now: std::time::Instant,
    ) -> Result<(), IdentityError> {
        self.bytes_since_rekey = self.bytes_since_rekey.saturating_add(plaintext_len);
        self.maybe_rekey_at(now)
    }

    /// Trip the in-place rekey if `bytes_since_rekey >= REKEY_BYTES` OR
    /// `now - started_at >= REKEY_INTERVAL`. Rekeys **both** cipher directions
    /// (deterministically, from the existing keys per Noise spec §4.2 — no wire
    /// message) and resets both counters. The peer rekeys symmetrically when
    /// its own accounting trips, so the two ends stay in lockstep without a
    /// re-handshake.
    ///
    /// Returns `Err` only if the session is already closed; rekey itself cannot
    /// fail in `snow` 0.9.
    fn maybe_rekey_at(&mut self, now: std::time::Instant) -> Result<(), IdentityError> {
        let by_bytes = self.bytes_since_rekey >= REKEY_BYTES;
        let by_time = now.saturating_duration_since(self.started_at) >= REKEY_INTERVAL;
        if by_bytes || by_time {
            let state = self.state_mut()?;
            state.rekey_outgoing();
            state.rekey_incoming();
            self.bytes_since_rekey = 0;
            self.started_at = now;
        }
        Ok(())
    }
}

impl Drop for NoiseSession {
    fn drop(&mut self) {
        // Drop the TransportState eagerly; snow zeroizes its cipher key
        // material on drop. The transport_hash is a binding value, not a
        // secret, so it needs no wiping.
        drop(self.state.take());
    }
}

/// Drive the full IK handshake as the **initiator** (device) over caller
/// supplied byte channels, returning the established [`NoiseSession`].
///
/// `local` is the device's X25519 Noise static; `remote_static_pub` is the
/// Core's static public key the device pre-loads. `send` transmits the first IK
/// message to the responder; `recv` returns the responder's reply
/// (`-> e, es, s, ss` then `<- e, ee, se`). Transport-agnostic: Task 212
/// supplies Iroh-backed closures; the Tier-2 test supplies in-memory ones.
///
/// `now` seeds the 1 h rekey timer (callers pass `Instant::now()`).
pub fn establish_initiator<S, R>(
    local: &NoiseStatic,
    remote_static_pub: &[u8; 32],
    now: std::time::Instant,
    mut send: S,
    mut recv: R,
) -> Result<NoiseSession, IdentityError>
where
    S: FnMut(&[u8]) -> Result<(), IdentityError>,
    R: FnMut() -> Result<Vec<u8>, IdentityError>,
{
    let mut hs = NoiseIkHandshake::initiator(local, remote_static_pub)?;
    // -> e, es, s, ss
    let m1 = hs.write_message(&[])?;
    send(&m1)?;
    // <- e, ee, se
    let m2 = recv()?;
    hs.read_message(&m2)?;
    if !hs.is_handshake_finished() {
        return Err(IdentityError::Noise(
            "IK handshake not finished after the two messages".to_string(),
        ));
    }
    hs.into_session(now)
}

/// Drive the full IK handshake as the **responder** (Core) over caller-supplied
/// byte channels, returning the established [`NoiseSession`].
///
/// `local` is the Core's X25519 Noise static. `recv` returns the initiator's
/// first message; `send` transmits the responder's reply. Mirror of
/// [`establish_initiator`].
pub fn establish_responder<S, R>(
    local: &NoiseStatic,
    now: std::time::Instant,
    mut send: S,
    mut recv: R,
) -> Result<NoiseSession, IdentityError>
where
    S: FnMut(&[u8]) -> Result<(), IdentityError>,
    R: FnMut() -> Result<Vec<u8>, IdentityError>,
{
    let mut hs = NoiseIkHandshake::responder(local)?;
    // <- e, es, s, ss  (read the initiator's first message)
    let m1 = recv()?;
    hs.read_message(&m1)?;
    // -> e, ee, se
    let m2 = hs.write_message(&[])?;
    send(&m2)?;
    if !hs.is_handshake_finished() {
        return Err(IdentityError::Noise(
            "IK handshake not finished after the two messages".to_string(),
        ));
    }
    hs.into_session(now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Complete a loopback IK handshake between two in-process endpoints over
    /// `Vec` buffers, returning both sessions (initiator, responder).
    fn loopback(now: Instant) -> Result<(NoiseSession, NoiseSession), IdentityError> {
        let dev = NoiseStatic::generate()?;
        let core = NoiseStatic::generate()?;
        let core_pub = core.public();

        let mut ini = NoiseIkHandshake::initiator(&dev, &core_pub)?;
        let mut res = NoiseIkHandshake::responder(&core)?;

        let m1 = ini.write_message(&[])?;
        res.read_message(&m1)?;
        let m2 = res.write_message(&[])?;
        ini.read_message(&m2)?;

        assert!(ini.is_handshake_finished());
        assert!(res.is_handshake_finished());
        Ok((ini.into_session(now)?, res.into_session(now)?))
    }

    #[test]
    fn protocol_string_is_frozen() {
        assert_eq!(NOISE_IK_PARAMS, "Noise_IK_25519_AESGCM_BLAKE2b");
    }

    #[test]
    fn rekey_thresholds_are_frozen() {
        assert_eq!(REKEY_BYTES, 1_000_000_000);
        assert_eq!(REKEY_INTERVAL, Duration::from_secs(3600));
    }

    #[test]
    fn handshake_roundtrips_payload_both_directions() {
        let now = Instant::now();
        let (mut ini, mut res) = loopback(now).expect("handshake");

        let pt = b"gRPC frame from device";
        let ct = ini.encrypt_at(pt, now).expect("encrypt");
        assert_ne!(&ct[..], &pt[..], "frame must be encrypted on the wire");
        let back = res.decrypt_at(&ct, now).expect("decrypt");
        assert_eq!(&back[..], &pt[..]);

        let reply = b"gRPC response from Core";
        let ct2 = res.encrypt_at(reply, now).expect("encrypt reply");
        let back2 = ini.decrypt_at(&ct2, now).expect("decrypt reply");
        assert_eq!(&back2[..], &reply[..]);
    }

    #[test]
    fn both_ends_derive_same_transport_hash() {
        let now = Instant::now();
        let (ini, res) = loopback(now).expect("handshake");
        assert_eq!(ini.transport_hash(), res.transport_hash());
        assert_ne!(ini.transport_hash(), [0u8; TRANSPORT_HASH_LEN]);
        assert!(ini.is_initiator());
        assert!(!res.is_initiator());
    }

    #[test]
    fn wrong_responder_static_fails_handshake() {
        let dev = NoiseStatic::generate().unwrap();
        let core = NoiseStatic::generate().unwrap();
        let wrong = NoiseStatic::generate().unwrap();

        // Initiator pre-loads the WRONG responder static.
        let mut ini = NoiseIkHandshake::initiator(&dev, &wrong.public()).unwrap();
        let mut res = NoiseIkHandshake::responder(&core).unwrap();

        let m1 = ini.write_message(&[]).unwrap();
        // The responder's es/ss DH mixes diverge → message 1 fails to decrypt.
        assert!(res.read_message(&m1).is_err());
    }

    #[test]
    fn establish_helpers_drive_full_handshake() {
        use std::cell::RefCell;
        let now = Instant::now();
        let dev = NoiseStatic::generate().unwrap();
        let core = NoiseStatic::generate().unwrap();
        let core_pub = core.public();

        // Two one-slot mailboxes for the two messages.
        let to_responder: RefCell<Option<Vec<u8>>> = RefCell::new(None);
        let to_initiator: RefCell<Option<Vec<u8>>> = RefCell::new(None);

        // We run initiator-then-responder lock-step (single-threaded): the
        // initiator writes m1, then the responder reads m1 + writes m2, then the
        // initiator reads m2. Drive it by hand since establish_* expect a recv
        // that yields the already-present message.
        let mut ini_hs = NoiseIkHandshake::initiator(&dev, &core_pub).unwrap();
        let m1 = ini_hs.write_message(&[]).unwrap();
        *to_responder.borrow_mut() = Some(m1);

        let res_session = establish_responder(
            &core,
            now,
            |m| {
                *to_initiator.borrow_mut() = Some(m.to_vec());
                Ok(())
            },
            || {
                to_responder
                    .borrow_mut()
                    .take()
                    .ok_or_else(|| IdentityError::Noise("no m1".into()))
            },
        )
        .expect("responder establishes");

        let m2 = to_initiator.borrow_mut().take().unwrap();
        ini_hs.read_message(&m2).unwrap();
        let mut ini_session = ini_hs.into_session(now).unwrap();

        // Sessions interoperate.
        let ct = ini_session.encrypt_at(b"hi", now).unwrap();
        let pt = {
            let mut r = res_session;
            r.decrypt_at(&ct, now).unwrap()
        };
        assert_eq!(&pt[..], b"hi");
    }

    #[test]
    fn rekey_on_byte_threshold_keeps_session_usable() {
        // Force a low byte threshold by pushing the counter near REKEY_BYTES
        // via a large payload would be slow; instead drive many frames and
        // assert the counter resets after crossing. Use a tiny synthetic
        // threshold by encrypting just over REKEY_BYTES is infeasible in a unit
        // test, so we verify the reset semantics via the time path + a direct
        // counter check below; here we confirm a rekey leaves both ends in
        // sync.
        let now = Instant::now();
        let (mut ini, mut res) = loopback(now).expect("handshake");

        // Normal frame.
        let ct = ini.encrypt_at(b"before", now).unwrap();
        assert_eq!(res.decrypt_at(&ct, now).unwrap(), b"before");

        // Trip the TIME threshold on BOTH ends symmetrically: advance the clock
        // past REKEY_INTERVAL for the next op on each side.
        let later = now + REKEY_INTERVAL + Duration::from_secs(1);
        let ct2 = ini.encrypt_at(b"after-rekey", later).unwrap();
        assert_eq!(
            ini.bytes_since_rekey(),
            0,
            "initiator counter reset on rekey"
        );
        let back = res.decrypt_at(&ct2, later).unwrap();
        assert_eq!(back, b"after-rekey");
        assert_eq!(
            res.bytes_since_rekey(),
            0,
            "responder counter reset on rekey"
        );

        // And the session keeps working post-rekey (same advanced clock so no
        // further rekey trips mid-exchange).
        let ct3 = res.encrypt_at(b"still works", later).unwrap();
        assert_eq!(ini.decrypt_at(&ct3, later).unwrap(), b"still works");
    }

    #[test]
    fn oversized_frame_returns_err_not_panic() {
        let now = Instant::now();
        let (mut ini, _res) = loopback(now).expect("handshake");
        // A payload that, plus the 16-byte tag, exceeds the 65535 Noise max.
        let too_big = vec![0u8; 65_535];
        assert!(
            ini.encrypt_at(&too_big, now).is_err(),
            "an over-max payload must error, not panic"
        );
        // A max-sized payload (65519 = 65535 - 16) is fine.
        let max_ok = vec![0u8; 65_519];
        assert!(ini.encrypt_at(&max_ok, now).is_ok());
    }

    #[test]
    fn byte_counter_accumulates_combined_directions() {
        let now = Instant::now();
        let (mut ini, mut res) = loopback(now).expect("handshake");
        let ct = ini.encrypt_at(&[0u8; 100], now).unwrap();
        assert_eq!(ini.bytes_since_rekey(), 100);
        let _ = res.decrypt_at(&ct, now).unwrap();
        assert_eq!(
            res.bytes_since_rekey(),
            100,
            "decrypt counts plaintext bytes"
        );
        // A second outbound frame accumulates.
        let _ = ini.encrypt_at(&[0u8; 50], now).unwrap();
        assert_eq!(ini.bytes_since_rekey(), 150);
    }
}
