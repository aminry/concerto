//! Device-pairing coordinator (`design/12 §3.3`, §6.2, Task 207).
//!
//! Owns the in-memory pairing-token store and the two halves of the ceremony:
//!
//! - [`PairingCoordinator::start_pairing`] mints a one-shot token and returns
//!   the QR-payload [`PairingChallenge`].
//! - [`PairingCoordinator::complete_pairing`] verifies the device's signature
//!   over `pairing_token || nonce || device_pubkey`, consumes the token, mints +
//!   signs a `DeviceCert` via Task 206's [`LocalCoreIssuer`], inserts the
//!   `devices` row, and returns the signed cert bytes.
//!
//! # Token rules (`design/12 §6.2`, FROZEN)
//!
//! - 32 random bytes from `getrandom`.
//! - **In-memory only** — never persisted; a Core restart drops all tokens, so
//!   pairing survives a restart only by the operator re-initiating it.
//! - **Hashed at rest**: the store keys on `BLAKE2b-256(token)`
//!   ([`TokenHash`]), never the raw token. The raw 32 bytes go out in the QR /
//!   are the Noise PSK; the Core keeps only the hash to compare + consume.
//! - **60 s TTL** — a photographed-but-unused token still expires.
//! - **One-shot** — a successful consume removes it; a replay finds nothing
//!   (`pairing.consumed`).
//! - **≤ 3 active** — minting a 4th evicts the oldest (by `issued_at`).
//!
//! # Signed-payload framing (FROZEN)
//!
//! The device signs the exact concatenation
//! `pairing_token (32) || nonce (32) || device_pubkey (32)` (96 bytes total).
//! The 32-byte nonce length is fixed so the layout parses unambiguously. The
//! Core reconstructs the same bytes from the raw token it holds + the request's
//! nonce/pubkey and verifies against `device_pubkey` (Task 205's `verify`).
//!
//! # Transport-agnostic (Tier-2 double)
//!
//! The coordinator drives issuance + persistence; the Noise XX handshake itself
//! is a thin wrapper in [`concerto_identity::noise_xx`] that the caller runs
//! over whatever byte duplex it has (a real Iroh stream in production, a
//! `tokio::io::duplex` in the Tier-2 loopback test). The coordinator's
//! `complete_pairing` takes the already-decrypted [`CompletePairingRequest`]
//! material, so it is independent of the transport — exactly the seam the
//! loopback double exercises.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use blake2::digest::consts::U32;
use blake2::{Blake2b, Digest};
use concerto_identity::{DeviceCertIssuer, LocalCoreIssuer, PairingRequest, PublicKey, Signature};
use concerto_persist::Persistence;

use crate::audit::{AuditEvent, AuditKind, AuditWriter, EntityKind};
use concerto_error::{Error, Result};

/// BLAKE2b-256, matching the `device_id` / Files-checksum digest.
type Blake2b256 = Blake2b<U32>;

/// The at-rest key for a pairing token: `BLAKE2b-256(raw_token)`.
type TokenHash = [u8; 32];

/// Token time-to-live: 60 s (`design/12 §6.2`).
pub const PAIRING_TOKEN_TTL: Duration = Duration::from_secs(60);

/// Maximum number of simultaneously-active pairing tokens (`design/12 §6.2`).
pub const MAX_ACTIVE_TOKENS: usize = 3;

/// The FROZEN nonce length in the signed pairing payload.
pub const PAIRING_NONCE_LEN: usize = 32;

/// In-memory state for one outstanding pairing token (`design/12 §4`).
///
/// Keyed in the store by `BLAKE2b-256(token)`; the raw token is never stored.
struct PairingTokenState {
    /// When the token was minted (used both for TTL and oldest-eviction).
    issued_at: SystemTime,
    /// When the token expires (`issued_at + 60 s`).
    expires_at: SystemTime,
}

/// The pairing-token store: a hash-keyed map under a sync mutex.
///
/// All operations are sync + short (no `.await` while the lock is held), so a
/// `std::sync::Mutex` is correct and cheap. The coordinator owns one of these.
struct TokenStore {
    tokens: Mutex<HashMap<TokenHash, PairingTokenState>>,
}

impl TokenStore {
    fn new() -> Self {
        Self {
            tokens: Mutex::new(HashMap::new()),
        }
    }

    /// Mint a fresh raw token, storing only its hash. Sweeps expired entries,
    /// then evicts the oldest if minting would exceed [`MAX_ACTIVE_TOKENS`].
    /// Returns the raw 32-byte token (for the QR) and its expiry instant.
    fn mint(&self, now: SystemTime) -> Result<([u8; 32], SystemTime)> {
        let mut token = [0u8; 32];
        getrandom::getrandom(&mut token)
            .map_err(|e| Error::Internal(format!("pairing token randomness: {e}")))?;
        let hash = hash_token(&token);
        let expires_at = now + PAIRING_TOKEN_TTL;

        let mut guard = self.tokens.lock().expect("pairing token store poisoned");
        // Sweep expired tokens first so they don't count toward the cap.
        guard.retain(|_, st| st.expires_at > now);
        // Enforce ≤ 3 active: evict the oldest by `issued_at` until there is
        // room for the new one.
        while guard.len() >= MAX_ACTIVE_TOKENS {
            if let Some(oldest) = guard
                .iter()
                .min_by_key(|(_, st)| st.issued_at)
                .map(|(k, _)| *k)
            {
                guard.remove(&oldest);
            } else {
                break;
            }
        }
        guard.insert(
            hash,
            PairingTokenState {
                issued_at: now,
                expires_at,
            },
        );
        Ok((token, expires_at))
    }

    /// One-shot consume. Looks the raw token up by hash and removes it.
    ///
    /// - `Ok(())` — token was present and not expired (now removed).
    /// - `Err(pairing.expired)` — token was present but past its TTL.
    /// - `Err(pairing.consumed)` — token was absent (never minted, already
    ///   consumed, or evicted) — surfaced as a replay rejection.
    fn consume(&self, raw_token: &[u8], now: SystemTime) -> Result<()> {
        let hash = hash_token(raw_token);
        let mut guard = self.tokens.lock().expect("pairing token store poisoned");
        match guard.remove(&hash) {
            Some(st) => {
                if st.expires_at <= now {
                    Err(Error::Pairing("pairing.expired".to_string()))
                } else {
                    Ok(())
                }
            }
            None => Err(Error::Pairing("pairing.consumed".to_string())),
        }
    }

    /// Number of currently-stored (not-yet-swept) tokens. Test helper.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.tokens.lock().expect("poisoned").len()
    }
}

/// `BLAKE2b-256(raw_token)` — the at-rest token key.
fn hash_token(raw_token: &[u8]) -> TokenHash {
    let mut hasher = Blake2b256::new();
    hasher.update(raw_token);
    hasher.finalize().into()
}

/// Reconstruct the FROZEN signed payload `pairing_token || nonce ||
/// device_pubkey` the device signed.
fn signed_payload(pairing_token: &[u8], nonce: &[u8], device_pubkey: &[u8; 32]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(pairing_token.len() + nonce.len() + 32);
    payload.extend_from_slice(pairing_token);
    payload.extend_from_slice(nonce);
    payload.extend_from_slice(device_pubkey);
    payload
}

/// The already-decrypted `PairingRequest` material the coordinator verifies
/// (`design/12 §3.3`). In production this is read off the Noise XX transport;
/// the loopback double passes the same fields in-process. Mirrors the proto
/// `CompletePairingRequest`.
#[derive(Debug, Clone)]
pub struct CompletePairingInput {
    /// The pairing device's Ed25519 public key (32 bytes).
    pub device_pubkey: [u8; 32],
    /// The user-supplied device name.
    pub device_name: String,
    /// The 32-byte nonce the device chose (FROZEN length).
    pub nonce: Vec<u8>,
    /// The device's 64-byte Ed25519 signature over the FROZEN payload.
    pub signature: [u8; 64],
    /// The raw 32-byte pairing token the device read from the QR.
    pub pairing_token: Vec<u8>,
}

/// The successful pairing outcome.
#[derive(Debug, Clone)]
pub struct PairingOutcome {
    /// The on-wire signed cert form `cert_bytes || signature` (opaque CBOR
    /// bytes per Decision D1) the device persists + presents on every connect.
    pub signed_device_cert: Vec<u8>,
    /// The Core's Ed25519 identity public key (32 bytes), echoed to the device.
    pub core_pubkey: [u8; 32],
}

/// The QR-payload challenge returned by [`PairingCoordinator::start_pairing`]
/// (`design/12 §3.3`). The handler maps this to the proto `PairingChallenge`.
#[derive(Debug, Clone)]
pub struct PairingChallenge {
    /// The Core's identity public key (32 bytes).
    pub core_pubkey: [u8; 32],
    /// The raw 32-byte one-shot pairing token (secret; QR only).
    pub pairing_token: [u8; 32],
    /// The Core's LAN endpoint when known (empty in the loopback double).
    pub lan_endpoint: String,
    /// The relay hint when configured (empty otherwise).
    pub relay_hint: String,
    /// Token expiry instant (`issued_at + 60 s`).
    pub expires_at: SystemTime,
}

/// The device-pairing coordinator. Holds the token store, the Core's issuer,
/// a persistence handle (for the `devices` INSERT), and an audit writer.
///
/// Cloning is not supported (it owns the token store); the gRPC handler holds
/// it behind an `Arc`.
pub struct PairingCoordinator {
    tokens: TokenStore,
    issuer: LocalCoreIssuer,
    persistence: std::sync::Arc<Persistence>,
    audit: AuditWriter,
    lan_endpoint: String,
    relay_hint: String,
}

impl PairingCoordinator {
    /// Build a coordinator from the Core's issuer, a persistence handle, and an
    /// audit writer. `lan_endpoint` / `relay_hint` are the QR-payload hints the
    /// transport layer (Task 212/213/214) supplies; pass empty strings when
    /// none is known (the Tier-2 loopback double does).
    pub fn new(
        issuer: LocalCoreIssuer,
        persistence: std::sync::Arc<Persistence>,
        audit: AuditWriter,
        lan_endpoint: String,
        relay_hint: String,
    ) -> Self {
        Self {
            tokens: TokenStore::new(),
            issuer,
            persistence,
            audit,
            lan_endpoint,
            relay_hint,
        }
    }

    /// Mint a one-shot pairing token + return the QR challenge (`design/12
    /// §3.3`). Sync — the token store is in-memory. Emits
    /// `DevicePairingStarted`.
    pub fn start_pairing(&self) -> Result<PairingChallenge> {
        self.start_pairing_at(SystemTime::now())
    }

    /// Clock-injected core of [`Self::start_pairing`] (tests pin `now`).
    pub fn start_pairing_at(&self, now: SystemTime) -> Result<PairingChallenge> {
        let (token, expires_at) = self.tokens.mint(now)?;
        let core_pubkey = self.issuer.core_public_key().to_bytes();
        self.audit.append(
            AuditEvent::new(
                AuditKind::DevicePairingStarted,
                crate::audit::AuditActor::System,
            )
            .with_details(serde_json::json!({
                "core_pubkey": hex::encode(core_pubkey),
            })),
        );
        Ok(PairingChallenge {
            core_pubkey,
            pairing_token: token,
            lan_endpoint: self.lan_endpoint.clone(),
            relay_hint: self.relay_hint.clone(),
            expires_at,
        })
    }

    /// Complete pairing (`design/12 §3.3`). Verifies the device's signature,
    /// consumes the token one-shot, mints + signs the cert, inserts the
    /// `devices` row, and returns the signed cert bytes. Emits
    /// `DevicePairingCompleted` on success and `DevicePairingFailed` on any
    /// rejection.
    pub async fn complete_pairing(&self, input: CompletePairingInput) -> Result<PairingOutcome> {
        self.complete_pairing_at(input, SystemTime::now()).await
    }

    /// Clock-injected core of [`Self::complete_pairing`] (tests pin `now` to
    /// exercise the TTL boundary deterministically).
    pub async fn complete_pairing_at(
        &self,
        input: CompletePairingInput,
        now: SystemTime,
    ) -> Result<PairingOutcome> {
        match self.complete_pairing_inner(input, now).await {
            Ok(outcome) => Ok(outcome),
            Err(e) => {
                self.audit.append(
                    AuditEvent::new(
                        AuditKind::DevicePairingFailed,
                        crate::audit::AuditActor::System,
                    )
                    .with_details(serde_json::json!({ "reason": e.to_string() })),
                );
                Err(e)
            }
        }
    }

    async fn complete_pairing_inner(
        &self,
        input: CompletePairingInput,
        now: SystemTime,
    ) -> Result<PairingOutcome> {
        // Validate the nonce length up front (FROZEN 32 bytes) so the signed
        // payload framing is unambiguous.
        if input.nonce.len() != PAIRING_NONCE_LEN {
            return Err(Error::Pairing(format!(
                "pairing.bad_nonce: expected {PAIRING_NONCE_LEN}-byte nonce, got {}",
                input.nonce.len()
            )));
        }

        // Verify the device's signature over `token || nonce || device_pubkey`
        // BEFORE consuming the token, so a bad-signature attempt does not burn
        // a still-valid token (the device can retry). The token must still be
        // present + unexpired for the request to be meaningful, but consuming
        // is the one-shot side effect we defer until the signature proves
        // possession of the device key.
        let device_pub = PublicKey::from_bytes(&input.device_pubkey)
            .map_err(|_| Error::Pairing("pairing.bad_device_pubkey".to_string()))?;
        let payload = signed_payload(&input.pairing_token, &input.nonce, &input.device_pubkey);
        let sig = Signature::from_bytes(&input.signature);
        device_pub
            .verify(&payload, &sig)
            .map_err(|_| Error::Pairing("pairing.bad_signature".to_string()))?;

        // Signature is good — now consume the token one-shot. A replay of an
        // already-consumed token fails here with `pairing.consumed`; an expired
        // token fails with `pairing.expired` (`design/12 §8`).
        self.tokens.consume(&input.pairing_token, now)?;

        // Mint + sign the cert via Task 206's issuer. The issuer derives
        // `device_id`, sets the 365-day expiry + `["admin"]` caps, and signs
        // with the Core's identity. The issuer trusts that the device key was
        // proven above — possession verification is the pairing channel's job.
        let signed = self
            .issuer
            .issue(PairingRequest {
                device_pubkey: input.device_pubkey,
                device_name: input.device_name.clone(),
            })
            .await
            .map_err(|e| Error::Pairing(format!("pairing.issue_failed: {e}")))?;

        // INSERT the `devices` row (`id` = device_id hex fingerprint;
        // `public_key` = raw Ed25519 bytes; `paired_at` = now unix seconds).
        // `revoked_at` / `push_*` are left NULL (Task 209 / Phase 5 own them).
        let device_id_hex = hex::encode(signed.cert.device_id);
        let paired_at = now
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        {
            let mut writer = self.persistence.writer().await;
            sqlx::query(
                "INSERT INTO devices (id, name, public_key, paired_at) VALUES (?, ?, ?, ?)",
            )
            .bind(&device_id_hex)
            .bind(&input.device_name)
            .bind(&input.device_pubkey[..])
            .bind(paired_at)
            .execute(&mut *writer)
            .await
            .map_err(|e| Error::Sqlx(Box::new(e)))?;
        }

        // On-wire signed cert form: `cert_bytes || signature` (D1 opaque CBOR).
        let mut signed_device_cert = signed.cert_bytes.clone();
        signed_device_cert.extend_from_slice(&signed.signature);

        self.audit.append(
            AuditEvent::new(
                AuditKind::DevicePairingCompleted,
                crate::audit::AuditActor::System,
            )
            .with_subject(EntityKind::Device, device_id_hex.clone())
            .with_details(serde_json::json!({
                "device_name": input.device_name,
            })),
        );

        Ok(PairingOutcome {
            signed_device_cert,
            core_pubkey: self.issuer.core_public_key().to_bytes(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_hashes_at_rest_and_consume_is_one_shot() {
        let store = TokenStore::new();
        let now = SystemTime::now();
        let (token, _exp) = store.mint(now).expect("mint");
        assert_eq!(store.len(), 1);
        // First consume succeeds; the same token is gone afterward.
        store.consume(&token, now).expect("first consume");
        assert_eq!(store.len(), 0);
        // Replay → pairing.consumed.
        let err = store.consume(&token, now).expect_err("replay rejected");
        assert!(err.to_string().contains("pairing.consumed"));
    }

    #[test]
    fn expired_token_consume_rejected() {
        let store = TokenStore::new();
        let now = SystemTime::now();
        let (token, _exp) = store.mint(now).expect("mint");
        let later = now + PAIRING_TOKEN_TTL + Duration::from_secs(1);
        let err = store.consume(&token, later).expect_err("expired rejected");
        assert!(err.to_string().contains("pairing.expired"));
    }

    #[test]
    fn at_most_three_active_tokens_oldest_evicted() {
        let store = TokenStore::new();
        let base = SystemTime::now();
        // Mint 3 within TTL with increasing issued_at so eviction order is
        // deterministic.
        let (t0, _) = store.mint(base).expect("mint 0");
        let (_t1, _) = store.mint(base + Duration::from_secs(1)).expect("mint 1");
        let (_t2, _) = store.mint(base + Duration::from_secs(2)).expect("mint 2");
        assert_eq!(store.len(), 3);
        // 4th mint evicts the oldest (t0).
        let (_t3, _) = store.mint(base + Duration::from_secs(3)).expect("mint 3");
        assert_eq!(store.len(), 3, "still capped at 3");
        // t0 was evicted → consuming it is a replay miss.
        let err = store
            .consume(&t0, base + Duration::from_secs(3))
            .expect_err("evicted token gone");
        assert!(err.to_string().contains("pairing.consumed"));
    }

    #[test]
    fn signed_payload_is_token_nonce_pubkey_concatenation() {
        let token = [1u8; 32];
        let nonce = [2u8; 32];
        let pubkey = [3u8; 32];
        let p = signed_payload(&token, &nonce, &pubkey);
        assert_eq!(p.len(), 96);
        assert_eq!(&p[0..32], &token[..]);
        assert_eq!(&p[32..64], &nonce[..]);
        assert_eq!(&p[64..96], &pubkey[..]);
    }
}
