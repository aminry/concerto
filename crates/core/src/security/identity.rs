//! Core Ed25519 identity establishment (`design/12 §3.1`, Task 206).
//!
//! On first Core start there is a keychain *slot*
//! ([`SecretKind::CoreIdentityPrivateKey`]) but no key in it. This module owns
//! the generate-or-load lifecycle: [`load_or_create_core_identity`] reads the
//! slot, generates + persists + mirrors a fresh keypair on first launch, and
//! reloads the same key on every subsequent boot. The keypair it returns is
//! what the boot path injects into [`concerto_identity::LocalCoreIssuer`].
//!
//! # FROZEN: keychain seed encoding
//!
//! The Core's private key is stored as the **lowercase-hex encoding of the
//! 32-byte Ed25519 seed** (the private scalar's seed, *not* a DER/PKCS#8
//! blob). 32 bytes → 64 hex chars, e.g. `"a1b2…"`. This is **FROZEN**: a
//! re-encode would fail to decode an existing Core's stored key and orphan its
//! identity (every paired device's cert is bound to that Core's public key).
//!
//! Rationale for the encoding:
//! - **Minimal + recovery-tool-friendly** (the `design/12 §3.2` / Task 205
//!   ethos: recovery tools decode without a schema). A bare seed is the
//!   smallest possible representation; hex is the most universally decodable
//!   text form.
//! - **No new dependency.** The task body suggested base64, but the Task 206
//!   license constraint forbids new third-party crates beyond `async-trait`.
//!   `hex` is already in the workspace tree, so hex adds nothing to the
//!   dependency graph. The *encoding* is what is frozen, not the alphabet
//!   choice; hex is equally minimal and recovery-friendly. (Flagged in the
//!   Task 206 Handoff.)
//!
//! # Public-key mirror
//!
//! The public key is mirrored to a non-secret file at `~/.concerto/identity.pub`
//! (`design/12 §3.1`) as lowercase hex + a trailing newline. It is public
//! material; sharing is fine. The mirror is best-effort-rewritten on every
//! load so a deleted/edited file self-heals on the next boot.

use std::path::{Path, PathBuf};

use concerto_identity::{KeyPair, PublicKey};
use concerto_keychain::{SecretKind, SecretValue, Secrets};

use crate::audit::{AuditActor, AuditEvent, AuditKind, AuditWriter, EntityKind};
use concerto_error::{Error, Result};

/// Filename of the public-key mirror under `~/.concerto/`.
const IDENTITY_PUB_FILENAME: &str = "identity.pub";

/// Outcome of [`load_or_create_core_identity`]: the Core's keypair, its public
/// key, and whether it was freshly generated on this call (vs. reloaded).
pub struct CoreIdentity {
    /// The Core's signing keypair (private + public).
    pub keypair: KeyPair,
    /// The Core's public key (also embedded in every issued cert as
    /// `core_pubkey`).
    pub public_key: PublicKey,
    /// `true` iff the key was generated on this call (first launch).
    pub created: bool,
}

/// Read the Core's Ed25519 identity from the keychain, generating + persisting
/// + mirroring it on first launch (`design/12 §3.1`).
///
/// - `secrets`: the OS keychain handle (`SecretKind::CoreIdentityPrivateKey`).
/// - `home_dir`: the user's home directory; the public key is mirrored to
///   `<home_dir>/.concerto/identity.pub`.
/// - `audit`: the audit writer; on first generation a [`AuditKind::CoreIdentityCreated`]
///   event is emitted (`design/12 §3.7`). Pass [`AuditWriter::noop`] in tests.
///
/// Returns the keypair the boot path injects into the issuer. A keychain
/// access failure at startup is fatal (`design/12 §8`: block Core startup with
/// a platform-specific message) — surfaced here as [`Error::Secrets`].
pub async fn load_or_create_core_identity(
    secrets: &Secrets,
    home_dir: &Path,
    audit: &AuditWriter,
) -> Result<CoreIdentity> {
    match secrets.get(SecretKind::CoreIdentityPrivateKey).await? {
        Some(stored) => {
            let keypair = decode_seed(stored.expose())?;
            let public_key = keypair.verifying_key();
            // Self-heal the mirror in case the file was deleted/edited.
            mirror_public_key(home_dir, &public_key)?;
            tracing::info!(
                core_pubkey = %hex::encode(public_key.to_bytes()),
                "core identity loaded from keychain"
            );
            Ok(CoreIdentity {
                keypair,
                public_key,
                created: false,
            })
        }
        None => {
            // First launch: generate a seed, encode it for the keychain, then
            // build the live keypair from that exact seed (the seed is the
            // FROZEN persisted form; `KeyPair` never re-exposes it). The seed
            // is then handed to the keychain (as hex) and is also embedded in
            // `keypair`, which zeroizes on drop; the keychain `SecretValue`
            // zeroizes its hex form on drop too.
            let seed = concerto_identity::generate_seed()
                .map_err(|e| Error::Internal(format!("core identity key generation: {e}")))?;
            let keypair = KeyPair::from_seed(&seed);
            let public_key = keypair.verifying_key();
            let seed_hex = hex::encode(seed);
            secrets
                .set(
                    SecretKind::CoreIdentityPrivateKey,
                    SecretValue::new(seed_hex),
                )
                .await?;

            mirror_public_key(home_dir, &public_key)?;

            audit.append(
                AuditEvent::new(AuditKind::CoreIdentityCreated, AuditActor::System)
                    .with_subject(EntityKind::Secret, "core_identity_private_key")
                    .with_details(serde_json::json!({
                        "core_pubkey": hex::encode(public_key.to_bytes()),
                    })),
            );

            tracing::info!(
                core_pubkey = %hex::encode(public_key.to_bytes()),
                "core identity generated and stored (first launch)"
            );
            Ok(CoreIdentity {
                keypair,
                public_key,
                created: true,
            })
        }
    }
}

/// FROZEN decoding: hex → 32-byte seed → [`KeyPair`].
fn decode_seed(seed_hex: &str) -> Result<KeyPair> {
    let bytes = hex::decode(seed_hex.trim())
        .map_err(|e| Error::Internal(format!("core identity seed is not valid hex: {e}")))?;
    let seed: [u8; 32] = bytes.as_slice().try_into().map_err(|_| {
        Error::Internal(format!(
            "core identity seed has wrong length: {} bytes (expected 32)",
            bytes.len()
        ))
    })?;
    Ok(KeyPair::from_seed(&seed))
}

/// Mirror the public key to `<home_dir>/.concerto/identity.pub` as lowercase
/// hex + newline (`design/12 §3.1`). Best-effort directory creation; an I/O
/// failure here is surfaced (the dir is the same one Core uses for config).
fn mirror_public_key(home_dir: &Path, public_key: &PublicKey) -> Result<()> {
    let dir = concerto_dir(home_dir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(IDENTITY_PUB_FILENAME);
    let contents = format!("{}\n", hex::encode(public_key.to_bytes()));
    std::fs::write(&path, contents)?;
    Ok(())
}

/// `<home_dir>/.concerto`.
fn concerto_dir(home_dir: &Path) -> PathBuf {
    home_dir.join(".concerto")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_encoding_is_64_hex_chars_and_roundtrips() {
        let seed = [3u8; 32];
        let kp = KeyPair::from_seed(&seed);
        let encoded = hex::encode(seed);
        assert_eq!(encoded.len(), 64, "32-byte seed → 64 hex chars");
        assert!(encoded.chars().all(|c| c.is_ascii_hexdigit()));
        // Decoding yields the same public key.
        let decoded = decode_seed(&encoded).expect("decode");
        assert_eq!(decoded.verifying_key(), kp.verifying_key());
    }

    #[test]
    fn decode_rejects_bad_hex_and_wrong_length() {
        assert!(decode_seed("nothex!!").is_err());
        assert!(decode_seed("aabb").is_err()); // valid hex, too short
    }

    #[test]
    fn mirror_writes_hex_pubkey_with_newline() {
        let tmp = tempfile::tempdir().expect("tmp");
        let kp = KeyPair::from_seed(&[5u8; 32]);
        let pk = kp.verifying_key();
        mirror_public_key(tmp.path(), &pk).expect("mirror");
        let written =
            std::fs::read_to_string(tmp.path().join(".concerto").join(IDENTITY_PUB_FILENAME))
                .expect("read");
        assert_eq!(written, format!("{}\n", hex::encode(pk.to_bytes())));
    }

    // The keychain round-trip (generate → store → reload-same-key) is
    // macOS-gated: CI's Linux/Windows lanes have no Secret Service backend, so
    // the `keyring` crate cannot persist there (same gating as
    // `crates/keychain/tests/round_trip.rs`). On macOS this is a real
    // end-to-end test against a unique throwaway service.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn load_or_create_generates_then_reloads_same_key() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let service = format!("concerto-test-{}-{}-{}-id", std::process::id(), nanos, seq);
        let secrets = Secrets::with_service_for_test(service);
        let tmp = tempfile::tempdir().expect("tmp");
        let audit = AuditWriter::noop();

        // First call: generates.
        let first = load_or_create_core_identity(&secrets, tmp.path(), &audit)
            .await
            .expect("first load_or_create");
        assert!(first.created, "first call must generate");

        // Second call: reloads the identical key (created == false).
        let second = load_or_create_core_identity(&secrets, tmp.path(), &audit)
            .await
            .expect("second load_or_create");
        assert!(!second.created, "second call must reload, not regenerate");
        assert_eq!(
            first.public_key.to_bytes(),
            second.public_key.to_bytes(),
            "reloaded key must match the generated key"
        );

        // Mirror file exists and matches.
        let mirror =
            std::fs::read_to_string(tmp.path().join(".concerto").join(IDENTITY_PUB_FILENAME))
                .expect("mirror exists");
        assert_eq!(mirror.trim(), hex::encode(first.public_key.to_bytes()));

        // Cleanup the throwaway keychain entry.
        let _ = secrets.delete(SecretKind::CoreIdentityPrivateKey).await;
    }
}
