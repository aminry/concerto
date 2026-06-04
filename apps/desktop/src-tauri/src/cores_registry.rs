//! The connected-Core registry (Task 218, `design/15 §3.10.1`).
//!
//! The Desktop can be paired with many Cores and dial them over different
//! transports (UDS co-located, Iroh split-host). This module owns the
//! **registry**: the cleartext `cores.json` metadata file plus the per-Core
//! secrets (device cert + device private key) in the OS keychain keyed by
//! `core_id`.
//!
//! **Storage split (FROZEN, `design/15 §3.10.1`):**
//! - `cores.json` (cleartext) — the [`PairedCore`] metadata rows + the
//!   active-Core pointer + a `version` field. **Secrets are NEVER here.**
//! - OS keychain (via `concerto-keychain`, keyed by `core_id`) — the device
//!   cert and the device private key. The Windows backend (Task 608) swaps
//!   under the same `concerto-keychain` API.
//!
//! `core_id = BLAKE2b(core_pubkey)` (lowercase hex), reusing
//! `concerto-identity`'s `device_id` hash so the derivation matches the rest of
//! the security spine.
//!
//! This task ships the registry **data layer + CRUD** the Desktop dispatch path
//! and (later) the pairing ceremony (Task 219/207/209) write into. The pairing
//! ceremony itself is OUT of scope; the write-side CRUD here is the seam it
//! calls.

use std::path::PathBuf;
use std::sync::Mutex;

use concerto_keychain::{CoreSecretSlot, SecretValue, Secrets};
use serde::{Deserialize, Serialize};

use crate::core_client::CoreClientError;

/// The `cores.json` schema version (`design/15 §3.10.1`). Bumped only on a
/// breaking change; new fields are append-only and don't bump it.
pub const CORES_JSON_VERSION: u32 = 1;

/// Which wire a paired Core is reached over (`design/15 §3.10.1`). The on-disk
/// (and renderer-facing) string form is the lowercase `"uds"` / `"iroh"`; the
/// renderer maps it onto the `ServerCapabilities.transport_kind` enum
/// (Task 201) when branching affordances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportKind {
    /// Co-located: tonic over a Unix domain socket, peer-UID auth.
    Uds,
    /// Split-host: tonic over Iroh, device-cert auth.
    Iroh,
}

/// One paired Core's cleartext metadata (`design/15 §3.10.1`). **FROZEN schema**
/// — the on-disk JSON shape (new fields append-only). Mirrors the design's
/// `PairedCore` struct; the `device_cert` + device private key it references
/// live in the keychain keyed by [`Self::core_id`], **never** in this struct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedCore {
    /// `BLAKE2b(core_pubkey)` lowercase hex — the registry key.
    pub core_id: String,
    /// User-friendly name ("This machine", "Home workstation", "Cloud VM").
    pub display_name: String,
    /// The transport this Core is reached over.
    pub transport: TransportKind,
    /// The UDS socket path — `Some` when `transport == Uds`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub uds_socket_path: Option<PathBuf>,
    /// The Iroh endpoint id to dial — `Some` when `transport == Iroh`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub iroh_endpoint_id: Option<String>,
    /// The Core's Ed25519 identity public key (32 bytes). Derives `core_id` and
    /// anchors device-cert validation (`design/12 §3.2`).
    pub core_pubkey: [u8; 32],
    /// The Core's X25519 **Noise** static public key (32 bytes) — the responder
    /// static the split-host `IrohCoreClient` pre-loads for the Noise IK
    /// handshake (`design/12 §3.4`). Captured at pairing from the QR's
    /// `core_noise_public` (Task 217 companion). `None` for UDS (peer-UID auth,
    /// no Noise). **Append-only** addition to the `design/15 §3.10.1` schema:
    /// the X25519 Noise static is a distinct key from the Ed25519 `core_pubkey`
    /// and the Iroh dial cannot proceed without it (see Handoff).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub core_noise_pubkey: Option<[u8; 32]>,
    /// Last successful connection, unix epoch seconds. `None` until first
    /// connect.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_connected_at: Option<u64>,
}

/// Derive the canonical `core_id` (lowercase hex of `BLAKE2b(core_pubkey)`) from
/// a Core public key, reusing `concerto-identity`'s `device_id` hash so the
/// derivation matches the security spine (`design/15 §3.10.1`).
///
/// Consumed by the pairing ceremony (Task 219/207/209) when it writes a freshly
/// paired Core; exposed now as the frozen derivation those tasks build on.
#[cfg_attr(not(test), allow(dead_code))]
pub fn core_id_for(core_pubkey: &[u8; 32]) -> String {
    let digest = concerto_identity::device_id(core_pubkey);
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push(char::from_digit((b >> 4) as u32, 16).expect("nibble"));
        s.push(char::from_digit((b & 0x0f) as u32, 16).expect("nibble"));
    }
    s
}

/// The on-disk `cores.json` document (`design/15 §3.10.1`). Cleartext metadata
/// only — the active-Core pointer is the `core_id` of the current Core.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoresDocument {
    /// Schema version (`design/15 §3.10.1`). See [`CORES_JSON_VERSION`].
    pub version: u32,
    /// The paired Cores.
    #[serde(default)]
    pub cores: Vec<PairedCore>,
    /// The active Core's `core_id` (the `ActiveCore` pointer), or `None`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub active_core_id: Option<String>,
}

impl Default for CoresDocument {
    fn default() -> Self {
        Self {
            version: CORES_JSON_VERSION,
            cores: Vec::new(),
            active_core_id: None,
        }
    }
}

/// The connected-Core registry: the in-memory [`CoresDocument`] backed by
/// `cores.json` on disk plus the OS keychain for secrets. Held as Tauri managed
/// state; the dispatch path resolves the active Core through it.
///
/// The on-disk path is fixed at construction; the in-memory doc is the source
/// of truth for reads and is persisted on every mutation.
#[derive(Debug)]
pub struct CoresRegistry {
    path: PathBuf,
    doc: Mutex<CoresDocument>,
}

impl CoresRegistry {
    /// Open (or create) the registry at `cores.json` under `config_dir`
    /// (`~/Library/.../concerto-desktop/` on macOS). A missing or unreadable
    /// file yields an empty registry; a malformed file is a hard error so a
    /// corrupt registry is visible rather than silently dropping pairings.
    pub fn open(config_dir: PathBuf) -> Result<Self, CoreClientError> {
        let path = config_dir.join("cores.json");
        let doc = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<CoresDocument>(&bytes)
                .map_err(|e| CoreClientError::Transport(format!("cores.json is malformed: {e}")))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => CoresDocument::default(),
            Err(e) => {
                return Err(CoreClientError::Transport(format!(
                    "reading {}: {e}",
                    path.display()
                )))
            }
        };
        Ok(Self {
            path,
            doc: Mutex::new(doc),
        })
    }

    /// Snapshot of all paired Cores (cleartext metadata only).
    pub fn list(&self) -> Vec<PairedCore> {
        self.doc
            .lock()
            .expect("cores registry poisoned")
            .cores
            .clone()
    }

    /// The active Core's `core_id`, or `None`.
    pub fn active_core_id(&self) -> Option<String> {
        self.doc
            .lock()
            .expect("cores registry poisoned")
            .active_core_id
            .clone()
    }

    /// The active [`PairedCore`], or `None` when no active pointer is set or it
    /// dangles.
    pub fn active(&self) -> Option<PairedCore> {
        let doc = self.doc.lock().expect("cores registry poisoned");
        let id = doc.active_core_id.as_ref()?;
        doc.cores.iter().find(|c| &c.core_id == id).cloned()
    }

    /// Look up a paired Core by id. (Used by tests + the pairing-write seam
    /// Task 219/207/209 call; not yet read by the live command path.)
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn get(&self, core_id: &str) -> Option<PairedCore> {
        self.doc
            .lock()
            .expect("cores registry poisoned")
            .cores
            .iter()
            .find(|c| c.core_id == core_id)
            .cloned()
    }

    /// Set the active Core. Errors if no such Core is paired.
    pub fn set_active(&self, core_id: &str) -> Result<(), CoreClientError> {
        {
            let mut doc = self.doc.lock().expect("cores registry poisoned");
            if !doc.cores.iter().any(|c| c.core_id == core_id) {
                return Err(CoreClientError::Transport(format!(
                    "no paired Core with id {core_id}"
                )));
            }
            doc.active_core_id = Some(core_id.to_string());
        }
        self.persist()
    }

    /// Insert or replace a paired Core (matched by `core_id`). Does not change
    /// the active pointer. This is the registry-write seam the pairing ceremony
    /// (Task 219/207/209) calls; secrets are stored separately via
    /// [`Self::store_secrets`].
    pub fn upsert(&self, core: PairedCore) -> Result<(), CoreClientError> {
        {
            let mut doc = self.doc.lock().expect("cores registry poisoned");
            match doc.cores.iter_mut().find(|c| c.core_id == core.core_id) {
                Some(existing) => *existing = core,
                None => doc.cores.push(core),
            }
        }
        self.persist()
    }

    /// Remove a paired Core and clear the active pointer if it pointed at it.
    /// The keychain secrets are deleted separately by the caller (kept apart so
    /// a metadata-only test never touches the keychain). The "Remove pairing"
    /// UX (Task 601, `design/15 §3.10.4`) drives this write seam.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn remove(&self, core_id: &str) -> Result<(), CoreClientError> {
        {
            let mut doc = self.doc.lock().expect("cores registry poisoned");
            doc.cores.retain(|c| c.core_id != core_id);
            if doc.active_core_id.as_deref() == Some(core_id) {
                doc.active_core_id = None;
            }
        }
        self.persist()
    }

    /// Promote a co-located UDS socket as the implicit "This machine"
    /// [`PairedCore`] and make it active (`design/15 §3.10.2` step 2). Idempotent
    /// by the synthetic local `core_id`. The local UDS path has no Core pubkey
    /// to fingerprint (peer-UID auth, no cert), so it uses the FROZEN
    /// [`LOCAL_MACHINE_CORE_ID`] sentinel rather than a BLAKE2b derivation.
    pub fn promote_local_uds(&self, socket_path: PathBuf) -> Result<(), CoreClientError> {
        let core = PairedCore {
            core_id: LOCAL_MACHINE_CORE_ID.to_string(),
            display_name: LOCAL_MACHINE_DISPLAY_NAME.to_string(),
            transport: TransportKind::Uds,
            uds_socket_path: Some(socket_path),
            iroh_endpoint_id: None,
            core_pubkey: [0u8; 32],
            core_noise_pubkey: None,
            last_connected_at: None,
        };
        self.upsert(core)?;
        self.set_active(LOCAL_MACHINE_CORE_ID)
    }

    /// Read a per-Core secret (device cert or device private key) from the OS
    /// keychain keyed by `core_id` (`design/15 §3.10.1`). The split-host
    /// `IrohCoreClient` reads the [`CoreSecretSlot::DeviceCert`] to present it in
    /// request metadata; the device private key never leaves the keychain.
    /// Returns `Ok(None)` when no entry exists.
    ///
    /// Secrets live in the keychain — never in `cores.json`. The keychain
    /// service is `concerto-keychain`'s default (`"concerto"`) in production;
    /// tests inject an isolated service via `CONCERTO_KEYCHAIN_SERVICE` to avoid
    /// the macOS Keychain prompt (the CI hazard).
    // Unconditional allow: the only caller today is the macOS-gated keychain
    // round-trip test, so this is dead on the Linux/Windows *test* build too
    // (not just production) until Task 219's connect flow calls it.
    #[allow(dead_code)]
    pub async fn get_secret(
        &self,
        core_id: &str,
        slot: CoreSecretSlot,
    ) -> Result<Option<SecretValue>, CoreClientError> {
        Secrets::new()
            .get_core_secret(core_id, slot)
            .await
            .map_err(|e| CoreClientError::Transport(format!("keychain read: {e}")))
    }

    /// Write a per-Core secret to the keychain keyed by `core_id`. The pairing
    /// ceremony (Task 219/207/209) calls this after a successful pairing to
    /// store the issued device cert + this device's private key.
    #[allow(dead_code)]
    pub async fn set_secret(
        &self,
        core_id: &str,
        slot: CoreSecretSlot,
        value: SecretValue,
    ) -> Result<(), CoreClientError> {
        Secrets::new()
            .set_core_secret(core_id, slot, value)
            .await
            .map_err(|e| CoreClientError::Transport(format!("keychain write: {e}")))
    }

    /// Delete a per-Core secret from the keychain keyed by `core_id`. Called by
    /// [`Self::remove`]'s caller when un-pairing (`design/15 §3.10.4`).
    /// Idempotent.
    #[allow(dead_code)]
    pub async fn delete_secret(
        &self,
        core_id: &str,
        slot: CoreSecretSlot,
    ) -> Result<(), CoreClientError> {
        Secrets::new()
            .delete_core_secret(core_id, slot)
            .await
            .map_err(|e| CoreClientError::Transport(format!("keychain delete: {e}")))
    }

    /// Persist the in-memory document to `cores.json` (pretty JSON, atomic via a
    /// temp file + rename). Secrets are never written here.
    fn persist(&self) -> Result<(), CoreClientError> {
        let doc = self.doc.lock().expect("cores registry poisoned").clone();
        let bytes = serde_json::to_vec_pretty(&doc)
            .map_err(|e| CoreClientError::Transport(format!("serializing cores.json: {e}")))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CoreClientError::Transport(format!("creating {}: {e}", parent.display()))
            })?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes)
            .map_err(|e| CoreClientError::Transport(format!("writing {}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            CoreClientError::Transport(format!("renaming into {}: {e}", self.path.display()))
        })?;
        Ok(())
    }
}

/// The synthetic `core_id` for the implicit co-located "This machine" UDS Core
/// (`design/15 §3.10.2` step 2). **FROZEN** — the local UDS has no `core_pubkey`
/// to fingerprint (peer-UID auth), so the registry keys it on this fixed marker
/// instead of a BLAKE2b derivation.
pub const LOCAL_MACHINE_CORE_ID: &str = "local-machine";

/// The display name of the implicit co-located UDS Core (`design/15 §3.10.2`).
pub const LOCAL_MACHINE_DISPLAY_NAME: &str = "This machine";

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_iroh_core(id_byte: u8) -> PairedCore {
        let pubkey = [id_byte; 32];
        PairedCore {
            core_id: core_id_for(&pubkey),
            display_name: format!("Core {id_byte}"),
            transport: TransportKind::Iroh,
            uds_socket_path: None,
            iroh_endpoint_id: Some(format!("endpoint-{id_byte}")),
            core_pubkey: pubkey,
            core_noise_pubkey: Some([id_byte ^ 0xAA; 32]),
            last_connected_at: None,
        }
    }

    #[test]
    fn round_trips_through_cores_json_with_no_secrets() {
        let tmp = TempDir::new().unwrap();
        let reg = CoresRegistry::open(tmp.path().to_path_buf()).unwrap();

        let core = sample_iroh_core(7);
        reg.upsert(core.clone()).unwrap();
        reg.set_active(&core.core_id).unwrap();

        // Re-open from disk and confirm the row + active pointer survived.
        let reg2 = CoresRegistry::open(tmp.path().to_path_buf()).unwrap();
        assert_eq!(reg2.list(), vec![core.clone()]);
        assert_eq!(reg2.active_core_id(), Some(core.core_id.clone()));
        assert_eq!(reg2.active(), Some(core.clone()));

        // The on-disk JSON must carry NO secret material — only the frozen
        // metadata fields. (The device cert + key live in the keychain.)
        let raw = std::fs::read_to_string(tmp.path().join("cores.json")).unwrap();
        assert!(raw.contains("\"version\""), "version field present");
        assert!(raw.contains(&core.core_id));
        assert!(
            !raw.to_lowercase().contains("device_cert")
                && !raw.to_lowercase().contains("private_key")
                && !raw.contains("device_cert"),
            "cores.json must never contain secret material, got: {raw}"
        );
    }

    #[test]
    fn upsert_replaces_in_place_and_remove_clears_active() {
        let tmp = TempDir::new().unwrap();
        let reg = CoresRegistry::open(tmp.path().to_path_buf()).unwrap();

        let mut core = sample_iroh_core(9);
        reg.upsert(core.clone()).unwrap();
        reg.set_active(&core.core_id).unwrap();
        assert_eq!(reg.list().len(), 1);

        // Upsert the same id with a new display name → replace, not append.
        core.display_name = "Renamed".to_string();
        reg.upsert(core.clone()).unwrap();
        assert_eq!(reg.list().len(), 1);
        assert_eq!(reg.get(&core.core_id).unwrap().display_name, "Renamed");

        // Remove clears both the row and the active pointer.
        reg.remove(&core.core_id).unwrap();
        assert!(reg.list().is_empty());
        assert_eq!(reg.active_core_id(), None);
    }

    #[test]
    fn set_active_rejects_unknown_core() {
        let tmp = TempDir::new().unwrap();
        let reg = CoresRegistry::open(tmp.path().to_path_buf()).unwrap();
        let err = reg.set_active("does-not-exist").expect_err("should reject");
        assert!(matches!(err, CoreClientError::Transport(_)));
    }

    #[test]
    fn promote_local_uds_registers_and_activates_this_machine() {
        let tmp = TempDir::new().unwrap();
        let reg = CoresRegistry::open(tmp.path().to_path_buf()).unwrap();
        let sock = PathBuf::from("/tmp/concerto/core.sock");
        reg.promote_local_uds(sock.clone()).unwrap();

        let active = reg.active().expect("local machine active");
        assert_eq!(active.core_id, LOCAL_MACHINE_CORE_ID);
        assert_eq!(active.display_name, LOCAL_MACHINE_DISPLAY_NAME);
        assert_eq!(active.transport, TransportKind::Uds);
        assert_eq!(active.uds_socket_path, Some(sock.clone()));

        // Idempotent: promoting again does not duplicate the row.
        reg.promote_local_uds(sock).unwrap();
        assert_eq!(reg.list().len(), 1);
    }

    #[test]
    fn malformed_cores_json_is_a_hard_error() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("cores.json"), b"{ not json").unwrap();
        let err = CoresRegistry::open(tmp.path().to_path_buf()).expect_err("should fail");
        assert!(matches!(err, CoreClientError::Transport(_)));
    }

    // macOS-gated: the OS keychain backend (`keyring`'s `apple-native`) only
    // persists on macOS. The Linux/Windows CI lanes have no Secret Service, so
    // a real `set`/`get` round-trip errors there — same gate as
    // `crates/keychain/tests/round_trip.rs`. The Windows backend lands with
    // Task 608. (Isolating the service name avoids the macOS Keychain prompt /
    // headless-CI hang, but can't conjure a backend where there is none.)
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn per_core_secrets_round_trip_in_isolated_keychain() {
        // Isolate the keychain service so this never pops a macOS prompt / hangs
        // headless CI (the KEYCHAIN-IN-CI hazard). `Secrets::new()` reads
        // `CONCERTO_KEYCHAIN_SERVICE`.
        std::env::set_var(
            "CONCERTO_KEYCHAIN_SERVICE",
            format!("concerto-test-{}-cores", std::process::id()),
        );
        let tmp = TempDir::new().unwrap();
        let reg = CoresRegistry::open(tmp.path().to_path_buf()).unwrap();
        let core = sample_iroh_core(5);

        // Absent → None.
        assert!(reg
            .get_secret(&core.core_id, CoreSecretSlot::DeviceCert)
            .await
            .unwrap()
            .is_none());

        // Write + read back the device cert and key.
        reg.set_secret(
            &core.core_id,
            CoreSecretSlot::DeviceCert,
            SecretValue::new("cert-bytes-b64".to_string()),
        )
        .await
        .unwrap();
        reg.set_secret(
            &core.core_id,
            CoreSecretSlot::DevicePrivateKey,
            SecretValue::new("key-seed-b64".to_string()),
        )
        .await
        .unwrap();

        let cert = reg
            .get_secret(&core.core_id, CoreSecretSlot::DeviceCert)
            .await
            .unwrap()
            .expect("cert present");
        assert_eq!(cert.expose(), "cert-bytes-b64");

        // Remove deletes both keychain entries.
        reg.delete_secret(&core.core_id, CoreSecretSlot::DeviceCert)
            .await
            .unwrap();
        reg.delete_secret(&core.core_id, CoreSecretSlot::DevicePrivateKey)
            .await
            .unwrap();
        assert!(reg
            .get_secret(&core.core_id, CoreSecretSlot::DeviceCert)
            .await
            .unwrap()
            .is_none());
    }

    #[test]
    fn core_id_is_blake2b_hex_of_pubkey() {
        let pubkey = [42u8; 32];
        let id = core_id_for(&pubkey);
        // 32-byte digest → 64 hex chars, matching identity's device_id.
        assert_eq!(id.len(), 64);
        let expected = concerto_identity::device_id(&pubkey);
        let mut exp_hex = String::new();
        for b in expected {
            exp_hex.push_str(&format!("{b:02x}"));
        }
        assert_eq!(id, exp_hex);
    }
}
