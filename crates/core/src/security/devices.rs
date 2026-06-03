//! Device management: list / revoke / core-info (`design/12 §3.11`, §5.2,
//! §7.3, Task 209).
//!
//! Task 207's [`PairingCoordinator`](crate::security::pairing::PairingCoordinator)
//! owns the *pairing* half of the `Devices` service (mint a token, run Noise XX,
//! issue a cert, INSERT the `devices` row). This module owns the *management*
//! half: reading the `devices` table back out, revoking a device, and reporting
//! the Core's identity + host/version.
//!
//! # Why a sibling [`DeviceManager`] (not an extension of the coordinator)
//!
//! The pairing coordinator's `new(..)` is a FROZEN Task-207 surface its
//! loopback test constructs directly; widening it would force editing a merged
//! task's test. Instead this task adds a sibling [`DeviceManager`] that the
//! `Devices` handler holds alongside the coordinator. The two share nothing but
//! the `Persistence` handle and the audit writer — both cheaply cloned at the
//! boot construction site.
//!
//! # The revoke sequence (`design/12 §7.3`, FROZEN ordering)
//!
//! [`DeviceManager::revoke_device`] runs, in this exact order:
//!
//! 1. **persist** — `UPDATE devices SET revoked_at = <now> WHERE id = ?`;
//! 2. **revoked-set insert** — `revoked_set.write().insert(device_id)` into the
//!    SAME `Arc<RwLock<HashSet<[u8;32]>>>` the Task-206 `LocalCoreIssuer::validate`
//!    reads, so the next connect from that device fails auth with no DB hit;
//! 3. **close sessions** — [`SessionCloser::close_sessions_for_device`] severs
//!    any open streams (a stolen device is cut off mid-stream);
//! 4. **audit + broadcast** — emit [`AuditKind::DeviceRevoked`] and publish a
//!    [`DeviceRevokedEvent`] on the manager's broadcast channel.
//!
//! Steps 1–2 must precede any reconnect window: the revoked-set insert is what
//! makes a racing reconnect fail. The `< 1 s` budget (`design/12 §10`) is the
//! *close* latency — `revoke_device` entry → step 3's `close_sessions_for_device`
//! call — proven hermetically in tests against an in-process stub that captures
//! the close [`std::time::Instant`].
//!
//! # The [`SessionCloser`] seam (FROZEN signature for Task 217)
//!
//! `close_sessions_for_device(device_id: [u8; 32])` is the contract Task 217's
//! `TransportHandle` (atop the Task-212 Iroh transport) satisfies. Until that
//! lands, the boot path injects [`NoopSessionCloser`] (a co-located install has
//! no remote streams to close) and tests inject a recording stub. The name +
//! signature are FROZEN so 217 wires the real one without renaming.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use concerto_identity::{device_id as derive_device_id, PublicKey, RevokedSet};
use concerto_persist::Persistence;
use tokio::sync::broadcast;

use crate::audit::{AuditActor, AuditEvent, AuditKind, AuditWriter, EntityKind};
use concerto_error::{Error, Result};

/// Broadcast-channel capacity for `device.revoked` events. Small: revocation is
/// a low-rate operator action, and subscribers (the Desktop "Connected Cores"
/// view in Task 219) only care about the latest few.
const DEVICE_EVENT_CAP: usize = 64;

/// The transport seam Task 209 depends on to sever a revoked device's open
/// streams (`design/12 §3.11`, §7.3). **FROZEN name + signature** — Task 217's
/// `TransportHandle` implements this so the live Iroh stream teardown plugs in
/// without renaming.
///
/// The method is sync + non-blocking: a revoke must not stall on a slow
/// transport. The real impl signals its session registry to tear down any
/// stream whose authenticated `device_id` matches; the in-process stub records
/// the call. `device_id` is the raw 32-byte BLAKE2b fingerprint (the cert form
/// the validator keys on), not its hex string.
pub trait SessionCloser: Send + Sync {
    /// Close every open session/stream belonging to `device_id`. Idempotent:
    /// closing a device with no open sessions is a no-op.
    fn close_sessions_for_device(&self, device_id: [u8; 32]);
}

/// The production-default [`SessionCloser`] for a co-located install with no
/// remote transport yet (Task 212/217 not wired). A purely local UDS Core has
/// no remote device streams to sever, so closing is a no-op. Task 217 replaces
/// this with the real `TransportHandle` at the boot construction site.
pub struct NoopSessionCloser;

impl SessionCloser for NoopSessionCloser {
    fn close_sessions_for_device(&self, _device_id: [u8; 32]) {
        // No remote transport in this build — nothing to close.
    }
}

/// A `device.revoked` broadcast event (`design/12 §5.3`). Published after the
/// revoke is persisted + the sessions are closed, so a subscriber observing it
/// can trust the device is already severed. Task 210/219 bridge this onto the
/// `Streams` surface; until then it is an in-process broadcast the manager owns
/// and tests subscribe to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRevokedEvent {
    /// The hex `devices.id` that was revoked.
    pub device_id: String,
    /// Unix seconds the revocation was persisted.
    pub revoked_at: i64,
}

/// One paired-device row, decoded from the `devices` table. The handler maps
/// this to the proto `DeviceEntry`; the `0`-sentinel for the nullable
/// `last_seen_at` / `revoked_at` columns is applied here (FROZEN — see the
/// proto comment).
#[derive(Debug, Clone)]
pub struct DeviceRecord {
    /// `devices.id` — hex BLAKE2b-256(device_pubkey).
    pub device_id: String,
    /// `devices.name`.
    pub name: String,
    /// `devices.public_key` — raw Ed25519 bytes.
    pub public_key: Vec<u8>,
    /// `devices.paired_at` — unix seconds.
    pub paired_at: i64,
    /// `devices.last_seen_at`, or `0` when NULL (deferred; always `0` in V1.0).
    pub last_seen_at: i64,
    /// `devices.revoked_at`, or `0` when NULL (`0` == active).
    pub revoked_at: i64,
}

/// The Core's identity + host/version (`design/12 §5.2`). The handler maps this
/// to the proto `CoreInfo`.
#[derive(Debug, Clone)]
pub struct CoreInfo {
    /// The Core's Ed25519 identity public key (32 bytes).
    pub core_pubkey: [u8; 32],
    /// The Core binary version (`CARGO_PKG_VERSION`).
    pub core_version: String,
    /// The Core host OS (`std::env::consts::OS`).
    pub core_host_os: String,
    /// The Core hostname (`hostname::get()`).
    pub core_hostname: String,
}

/// Owns the device-management read/revoke paths. Held behind an `Arc` by the
/// gRPC `Devices` handler; cheap to share.
pub struct DeviceManager {
    persistence: Arc<Persistence>,
    /// The SAME handle Task 206's issuer reads — revoke inserts here so the next
    /// `validate` rejects the device.
    revoked_set: RevokedSet,
    /// The Core's identity public key, echoed in `GetCoreInfo` (mirrors the
    /// value the issuer embeds in every cert).
    core_pubkey: PublicKey,
    audit: AuditWriter,
    session_closer: Arc<dyn SessionCloser>,
    revoked_tx: broadcast::Sender<DeviceRevokedEvent>,
}

impl DeviceManager {
    /// Build a manager from a persistence handle, the shared revoked set (the
    /// SAME `Arc` the Task-206 issuer holds), the Core's public key, an audit
    /// writer, and the [`SessionCloser`] seam (Task 217's `TransportHandle` in
    /// production; [`NoopSessionCloser`] until then).
    pub fn new(
        persistence: Arc<Persistence>,
        revoked_set: RevokedSet,
        core_pubkey: PublicKey,
        audit: AuditWriter,
        session_closer: Arc<dyn SessionCloser>,
    ) -> Self {
        let (revoked_tx, _rx) = broadcast::channel(DEVICE_EVENT_CAP);
        Self {
            persistence,
            revoked_set,
            core_pubkey,
            audit,
            session_closer,
            revoked_tx,
        }
    }

    /// Subscribe to `device.revoked` broadcasts (`design/12 §5.3`). Task 219's
    /// "Connected Cores" view and Task 210's auth bridge consume this; tests
    /// subscribe to assert a revoke was broadcast.
    pub fn subscribe_revoked(&self) -> broadcast::Receiver<DeviceRevokedEvent> {
        self.revoked_tx.subscribe()
    }

    /// List every paired device (active + revoked), most-recently-paired first
    /// (`design/12 §5.2`). Read-only.
    pub async fn list_devices(&self) -> Result<Vec<DeviceRecord>> {
        // `COALESCE(col, 0)` applies the FROZEN `0`-sentinel for the nullable
        // `last_seen_at` / `revoked_at` columns inside SQLite so the decode is
        // total (no Option handling at the boundary).
        let rows: Vec<(String, String, Vec<u8>, i64, i64, i64)> = sqlx::query_as(
            "SELECT id, name, public_key, paired_at, \
             COALESCE(last_seen_at, 0), COALESCE(revoked_at, 0) \
             FROM devices ORDER BY paired_at DESC, id ASC",
        )
        .fetch_all(self.persistence.readers())
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;

        Ok(rows
            .into_iter()
            .map(
                |(device_id, name, public_key, paired_at, last_seen_at, revoked_at)| DeviceRecord {
                    device_id,
                    name,
                    public_key,
                    paired_at,
                    last_seen_at,
                    revoked_at,
                },
            )
            .collect())
    }

    /// The Core's identity + host/version (`design/12 §5.2`). Host/version are
    /// derived from exactly the same sources `RuntimeHandler` uses for
    /// `ServerCapabilities` (Task 201) so there is one source of truth.
    pub fn core_info(&self) -> CoreInfo {
        CoreInfo {
            core_pubkey: self.core_pubkey.to_bytes(),
            core_version: env!("CARGO_PKG_VERSION").to_string(),
            core_host_os: std::env::consts::OS.to_string(),
            core_hostname: core_hostname(),
        }
    }

    /// Revoke a device (`design/12 §3.11`, §7.3). Runs the FROZEN sequence:
    /// persist `revoked_at` → insert the shared revoked set → close open
    /// sessions → audit + broadcast.
    ///
    /// Idempotency (DOCUMENTED CHOICE): revoking an already-revoked device is a
    /// no-op **success** — the `revoked_at` and revoked-set entry are left as
    /// they were and no second audit/broadcast fires (the device is already
    /// severed). An **unknown** device id fails [`Error::NotFound`]
    /// (`Code::NotFound`).
    pub async fn revoke_device(&self, device_id_hex: &str) -> Result<()> {
        self.revoke_device_at(device_id_hex, SystemTime::now())
            .await
    }

    /// Clock-injected core of [`Self::revoke_device`] (tests pin `now` so the
    /// persisted `revoked_at` is deterministic).
    pub async fn revoke_device_at(&self, device_id_hex: &str, now: SystemTime) -> Result<()> {
        // The raw 32-byte device id the revoked set + the SessionCloser key on.
        // Reject a malformed id up front (NOT_FOUND — no such device could
        // exist under a non-fingerprint id).
        let raw_id = decode_device_id(device_id_hex)
            .ok_or_else(|| Error::NotFound(format!("device.unknown: {device_id_hex}")))?;

        let revoked_at = now
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Step 1: persist `revoked_at`. The `WHERE revoked_at IS NULL` clause
        // makes the UPDATE affect exactly the *active* row: 0 rows means either
        // the id is unknown OR already revoked — we disambiguate with a follow-up
        // existence probe so idempotent re-revoke is a success and a truly
        // unknown id is NOT_FOUND.
        let affected = {
            let mut writer = self.persistence.writer().await;
            sqlx::query("UPDATE devices SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
                .bind(revoked_at)
                .bind(device_id_hex)
                .execute(&mut *writer)
                .await
                .map_err(|e| Error::Sqlx(Box::new(e)))?
                .rows_affected()
        };

        if affected == 0 {
            // No active row updated: either already-revoked (idempotent success)
            // or unknown (NOT_FOUND).
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM devices WHERE id = ?)")
                    .bind(device_id_hex)
                    .fetch_one(self.persistence.readers())
                    .await
                    .map_err(|e| Error::Sqlx(Box::new(e)))?;
            if exists {
                // Already revoked — ensure the revoked set still contains it
                // (defence in depth across a restart that hasn't re-mirrored the
                // table) but emit no second audit/broadcast.
                self.insert_revoked(raw_id);
                return Ok(());
            }
            return Err(Error::NotFound(format!("device.unknown: {device_id_hex}")));
        }

        // Step 2: insert into the shared revoked set BEFORE closing sessions, so
        // a reconnect that races the close still fails `validate`.
        self.insert_revoked(raw_id);

        // Step 3: actively close any open sessions for this device.
        self.session_closer.close_sessions_for_device(raw_id);

        // Step 4: audit + broadcast (the device is already severed at this
        // point — the audit log is the source of truth, `design/12 §3.11`).
        self.audit.append(
            AuditEvent::new(AuditKind::DeviceRevoked, AuditActor::System)
                .with_subject(EntityKind::Device, device_id_hex.to_string())
                .with_details(serde_json::json!({ "revoked_at": revoked_at })),
        );
        // `send` errors only when there are no live receivers, which is the
        // normal steady state (no Desktop subscribed); ignore it.
        let _ = self.revoked_tx.send(DeviceRevokedEvent {
            device_id: device_id_hex.to_string(),
            revoked_at,
        });

        Ok(())
    }

    /// Insert a raw device id into the shared revoked set (write lock). Sync +
    /// short — no `.await` under the lock.
    fn insert_revoked(&self, raw_id: [u8; 32]) {
        match self.revoked_set.write() {
            Ok(mut set) => {
                set.insert(raw_id);
            }
            Err(poisoned) => {
                // A poisoned lock means a panicked writer; recover the guard and
                // still insert (the revoked set must never silently drop a
                // revocation — that would leave a stolen device able to connect).
                poisoned.into_inner().insert(raw_id);
            }
        }
    }
}

/// Decode a hex `devices.id` into the raw 32-byte fingerprint. Returns `None`
/// for non-hex or wrong-length input (treated as an unknown device).
fn decode_device_id(device_id_hex: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(device_id_hex).ok()?;
    bytes.as_slice().try_into().ok()
}

/// The Core hostname, mirroring `RuntimeHandler::core_hostname` (Task 201) so
/// `GetCoreInfo` and `ServerCapabilities` report the same value.
fn core_hostname() -> String {
    match hostname::get() {
        Ok(h) => h.to_string_lossy().into_owned(),
        Err(e) => {
            tracing::warn!(error = %e, "hostname::get() failed; defaulting to <unknown>");
            "<unknown>".to_string()
        }
    }
}

/// Re-export so callers can derive a device id from a pubkey without reaching
/// into `concerto_identity` directly (keeps the management surface cohesive).
pub fn device_id_for_pubkey(device_pubkey: &[u8; 32]) -> [u8; 32] {
    derive_device_id(device_pubkey)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_device_id_roundtrips_and_rejects_bad_input() {
        let raw = [7u8; 32];
        let hexed = hex::encode(raw);
        assert_eq!(decode_device_id(&hexed), Some(raw));
        assert_eq!(decode_device_id("nothex!!"), None);
        assert_eq!(decode_device_id("aabb"), None); // valid hex, too short
    }

    #[test]
    fn noop_session_closer_is_a_noop() {
        // Purely a smoke check that the production default does not panic.
        NoopSessionCloser.close_sessions_for_device([0u8; 32]);
    }
}
