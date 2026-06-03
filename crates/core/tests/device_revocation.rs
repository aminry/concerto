//! Tier-1 tests for Task 209 — the `Devices` management half: `ListDevices`,
//! `RevokeDevice` (the `design/12 §7.3` sequence), and `GetCoreInfo`.
//!
//! **Test double:** an in-process [`SessionCloser`] stub that records the
//! closed `device_id`s and captures the close [`Instant`]. The
//! [`DeviceManager`](concerto_core::security::devices::DeviceManager) is
//! constructed directly in-test from a fresh `Persistence`, the SAME
//! `RevokedSet` handle a [`LocalCoreIssuer`] reads, and the stub — **no
//! `boot::start`, no keychain**, so the KEYCHAIN-IN-CI hazard does not apply.
//!
//! It proves:
//! - `ListDevices` returns inserted rows (active + revoked distinguishable via
//!   the `revoked_at` sentinel), most-recently-paired first;
//! - `RevokeDevice` runs the `§7.3` ordering: persists `revoked_at`, inserts
//!   the device id into the shared revoked set (asserted via 206's `validate`
//!   now rejecting that device), calls the `SessionCloser` stub, and audits +
//!   broadcasts `device.revoked`;
//! - the **revoke→close latency < 1 s** against the stub (captured `Instant`,
//!   no real `sleep`);
//! - unknown / already-revoked id behaviour (NOT_FOUND / idempotent success);
//! - `GetCoreInfo` returns the wired `core_pubkey` + host/version.
//!
//! What this double does **NOT** cover (→ Phase-2 Tier-3 manual checklist line
//! "revoke a device and confirm < 60 s stream teardown"): a **real** open Iroh
//! stream from a real second device being torn down over the wire — that needs
//! Tasks 212/217's live `TransportHandle`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use concerto_core::audit::{AuditEvent, AuditKind, AuditWriter};
use concerto_core::security::devices::{DeviceManager, SessionCloser};
use concerto_identity::{
    device_id, new_revoked_set, DeviceCertIssuer, KeyPair, LocalCoreIssuer, PairingRequest,
    PublicKey, RevokedSet,
};
use concerto_persist::{Persistence, PersistenceConfig};
use tempfile::TempDir;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Doubles + fixtures.
// ---------------------------------------------------------------------------

/// Records each `close_sessions_for_device` call with the `Instant` it fired,
/// so the test can assert the revoke→close latency budget.
#[derive(Default)]
struct RecordingSessionCloser {
    closed: Mutex<Vec<([u8; 32], Instant)>>,
}

impl RecordingSessionCloser {
    fn calls(&self) -> Vec<([u8; 32], Instant)> {
        self.closed.lock().expect("closer lock").clone()
    }
}

impl SessionCloser for RecordingSessionCloser {
    fn close_sessions_for_device(&self, device_id: [u8; 32]) {
        self.closed
            .lock()
            .expect("closer lock")
            .push((device_id, Instant::now()));
    }
}

/// A fresh on-disk `Persistence` (the `devices` table exists from migration
/// 0001) + a fixed Core seed.
async fn fixtures() -> (TempDir, Arc<Persistence>, KeyPair, PublicKey) {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join("concerto.db");
    let cfg = PersistenceConfig {
        db_path,
        max_readers: 2,
    };
    let persistence = Arc::new(Persistence::open(cfg).await.expect("open persistence"));
    let core_seed = [0x11u8; 32];
    let keypair = KeyPair::from_seed(&core_seed);
    let core_pub = keypair.verifying_key();
    (tmp, persistence, keypair, core_pub)
}

/// Insert a `devices` row directly (bypassing pairing) so the management tests
/// have rows to read/revoke. `revoked_at` left NULL (active).
async fn insert_device(
    persistence: &Persistence,
    device_pubkey: &[u8; 32],
    name: &str,
    paired_at: i64,
) -> String {
    let id_hex = hex::encode(device_id(device_pubkey));
    let mut writer = persistence.writer().await;
    sqlx::query("INSERT INTO devices (id, name, public_key, paired_at) VALUES (?, ?, ?, ?)")
        .bind(&id_hex)
        .bind(name)
        .bind(&device_pubkey[..])
        .bind(paired_at)
        .execute(&mut *writer)
        .await
        .expect("insert device");
    id_hex
}

/// An audit writer whose events the test can drain.
fn recording_audit() -> (AuditWriter, mpsc::Receiver<AuditEvent>) {
    let (tx, rx) = mpsc::channel(16);
    (AuditWriter::new(tx), rx)
}

fn make_manager(
    persistence: Arc<Persistence>,
    revoked: RevokedSet,
    core_pub: PublicKey,
    audit: AuditWriter,
    closer: Arc<RecordingSessionCloser>,
) -> DeviceManager {
    DeviceManager::new(persistence, revoked, core_pub, audit, closer)
}

// ---------------------------------------------------------------------------
// ListDevices.
// ---------------------------------------------------------------------------

/// `ListDevices` returns every inserted row, most-recently-paired first, with
/// active vs revoked distinguishable via the `revoked_at` sentinel.
#[tokio::test]
async fn list_devices_returns_active_and_revoked_rows() {
    let (_tmp, persistence, _core_key, core_pub) = fixtures().await;
    let revoked = new_revoked_set();
    let (audit, _rx) = recording_audit();
    let closer = Arc::new(RecordingSessionCloser::default());
    let mgr = make_manager(
        Arc::clone(&persistence),
        revoked,
        core_pub,
        audit,
        Arc::clone(&closer),
    );

    let pk_a = KeyPair::from_seed(&[0xA1u8; 32]).verifying_key().to_bytes();
    let pk_b = KeyPair::from_seed(&[0xB2u8; 32]).verifying_key().to_bytes();
    let id_a = insert_device(&persistence, &pk_a, "Phone A", 1000).await;
    let id_b = insert_device(&persistence, &pk_b, "Laptop B", 2000).await;

    // Revoke B so the list reports a non-zero `revoked_at` for it.
    mgr.revoke_device(&id_b).await.expect("revoke B");

    let list = mgr.list_devices().await.expect("list");
    assert_eq!(list.len(), 2, "both devices listed");

    // Most-recently-paired first: B (paired_at 2000) before A (1000).
    assert_eq!(list[0].device_id, id_b);
    assert_eq!(list[0].name, "Laptop B");
    assert_eq!(list[0].public_key, pk_b.to_vec());
    assert_eq!(list[0].paired_at, 2000);
    assert_eq!(
        list[0].last_seen_at, 0,
        "last_seen_at deferred → 0 sentinel"
    );
    assert!(
        list[0].revoked_at > 0,
        "revoked device has non-zero revoked_at"
    );

    assert_eq!(list[1].device_id, id_a);
    assert_eq!(
        list[1].revoked_at, 0,
        "active device → revoked_at sentinel 0"
    );
}

// ---------------------------------------------------------------------------
// RevokeDevice — the §7.3 sequence.
// ---------------------------------------------------------------------------

/// `RevokeDevice` runs the FROZEN `§7.3` sequence: persists `revoked_at`,
/// inserts the shared revoked set (so 206's `validate` now rejects the device),
/// calls the `SessionCloser` stub, and audits + broadcasts `device.revoked`.
/// The revoke→close latency is asserted < 1 s against the stub's captured
/// `Instant`.
#[tokio::test]
async fn revoke_runs_full_sequence_and_closes_within_budget() {
    let (_tmp, persistence, _core_key, core_pub) = fixtures().await;
    let revoked = new_revoked_set();
    // The issuer shares the SAME revoked-set handle the manager writes — this
    // is the whole point: a revoke is observed by `validate` with no DB hit.
    let issuer = LocalCoreIssuer::new(KeyPair::from_seed(&[0x11u8; 32]), core_pub, revoked.clone());
    let (audit, mut audit_rx) = recording_audit();
    let closer = Arc::new(RecordingSessionCloser::default());
    let mgr = make_manager(
        Arc::clone(&persistence),
        revoked.clone(),
        core_pub,
        audit,
        Arc::clone(&closer),
    );
    let mut revoked_events = mgr.subscribe_revoked();

    // A real device: issue it a cert from the Core key, confirm `validate`
    // ACCEPTS it before revocation.
    let device_key = KeyPair::from_seed(&[0x22u8; 32]);
    let device_pubkey = device_key.verifying_key().to_bytes();
    let raw_id = device_id(&device_pubkey);
    let signed = issuer
        .issue(PairingRequest {
            device_pubkey,
            device_name: "Stolen Phone".to_string(),
        })
        .await
        .expect("issue cert");
    let mut wire_cert = signed.cert_bytes.clone();
    wire_cert.extend_from_slice(&signed.signature);
    issuer
        .validate(&wire_cert)
        .expect("cert validates before revoke");

    let id_hex = insert_device(&persistence, &device_pubkey, "Stolen Phone", 1234).await;

    // Revoke, measuring the entry→close latency.
    let started = Instant::now();
    mgr.revoke_device(&id_hex).await.expect("revoke");

    // Step 1: `revoked_at` persisted (non-NULL).
    let row_revoked_at: Option<i64> =
        sqlx::query_scalar("SELECT revoked_at FROM devices WHERE id = ?")
            .bind(&id_hex)
            .fetch_one(persistence.readers())
            .await
            .expect("fetch revoked_at");
    assert!(
        row_revoked_at.is_some_and(|v| v > 0),
        "revoked_at persisted to a positive unix second"
    );

    // Step 2: the shared revoked set now contains the raw device id, so the
    // SAME issuer's `validate` REJECTS the previously-valid cert.
    assert!(
        revoked.read().expect("read revoked").contains(&raw_id),
        "device id inserted into the shared revoked set"
    );
    let err = issuer
        .validate(&wire_cert)
        .expect_err("cert rejected after revoke");
    assert!(
        err.to_string().to_lowercase().contains("revoked"),
        "validate now reports revoked: {err}"
    );

    // Step 3: the SessionCloser stub was called with the raw device id, and the
    // revoke→close latency is well within the 1 s budget.
    let calls = closer.calls();
    assert_eq!(calls.len(), 1, "exactly one close call");
    assert_eq!(calls[0].0, raw_id, "closed the right device");
    let latency = calls[0].1.duration_since(started);
    assert!(
        latency < Duration::from_secs(1),
        "revoke→close latency {latency:?} must be < 1s"
    );

    // Step 4: `DeviceRevoked` audited + `device.revoked` broadcast.
    let audit_event = audit_rx.try_recv().expect("a DeviceRevoked audit event");
    assert_eq!(audit_event.kind, AuditKind::DeviceRevoked);
    let broadcast = revoked_events
        .try_recv()
        .expect("a device.revoked broadcast");
    assert_eq!(broadcast.device_id, id_hex);
    assert!(broadcast.revoked_at > 0);
}

/// Revoking an unknown device id fails `NOT_FOUND`; no close, no audit.
#[tokio::test]
async fn revoke_unknown_device_is_not_found() {
    let (_tmp, persistence, _core_key, core_pub) = fixtures().await;
    let revoked = new_revoked_set();
    let (audit, mut audit_rx) = recording_audit();
    let closer = Arc::new(RecordingSessionCloser::default());
    let mgr = make_manager(persistence, revoked, core_pub, audit, Arc::clone(&closer));

    let unknown = hex::encode([0xFFu8; 32]);
    let err = mgr
        .revoke_device(&unknown)
        .await
        .expect_err("unknown id rejected");
    assert!(
        matches!(err, concerto_error::Error::NotFound(_)),
        "unknown device → NotFound, got {err}"
    );
    assert!(closer.calls().is_empty(), "no close on unknown id");
    assert!(audit_rx.try_recv().is_err(), "no audit on unknown id");
}

/// A malformed (non-fingerprint) device id is treated as unknown → NOT_FOUND.
#[tokio::test]
async fn revoke_malformed_id_is_not_found() {
    let (_tmp, persistence, _core_key, core_pub) = fixtures().await;
    let revoked = new_revoked_set();
    let (audit, _rx) = recording_audit();
    let closer = Arc::new(RecordingSessionCloser::default());
    let mgr = make_manager(persistence, revoked, core_pub, audit, closer);

    let err = mgr
        .revoke_device("not-a-hex-fingerprint")
        .await
        .expect_err("malformed id rejected");
    assert!(
        matches!(err, concerto_error::Error::NotFound(_)),
        "got {err}"
    );
}

/// Revoking an already-revoked device is an idempotent no-op SUCCESS: the
/// `revoked_at` is unchanged, the revoked set still contains it, and no SECOND
/// audit/broadcast fires.
#[tokio::test]
async fn revoke_already_revoked_is_idempotent_success() {
    let (_tmp, persistence, _core_key, core_pub) = fixtures().await;
    let revoked = new_revoked_set();
    let (audit, mut audit_rx) = recording_audit();
    let closer = Arc::new(RecordingSessionCloser::default());
    let mgr = make_manager(
        Arc::clone(&persistence),
        revoked.clone(),
        core_pub,
        audit,
        Arc::clone(&closer),
    );

    let pk = KeyPair::from_seed(&[0x33u8; 32]).verifying_key().to_bytes();
    let raw_id = device_id(&pk);
    let id_hex = insert_device(&persistence, &pk, "Phone", 5000).await;

    // First revoke at a pinned `now`.
    let t0 = SystemTime::now();
    mgr.revoke_device_at(&id_hex, t0)
        .await
        .expect("first revoke");
    let first_revoked_at: i64 = sqlx::query_scalar("SELECT revoked_at FROM devices WHERE id = ?")
        .bind(&id_hex)
        .fetch_one(persistence.readers())
        .await
        .expect("fetch");
    // Drain the first revoke's audit event.
    audit_rx.try_recv().expect("first audit");

    // Second revoke at a LATER `now` → no-op success; `revoked_at` unchanged.
    let t1 = t0 + Duration::from_secs(100);
    mgr.revoke_device_at(&id_hex, t1)
        .await
        .expect("second revoke is success");
    let second_revoked_at: i64 = sqlx::query_scalar("SELECT revoked_at FROM devices WHERE id = ?")
        .bind(&id_hex)
        .fetch_one(persistence.readers())
        .await
        .expect("fetch");
    assert_eq!(
        first_revoked_at, second_revoked_at,
        "revoked_at unchanged on re-revoke"
    );
    // The revoked set still contains it.
    assert!(revoked.read().expect("read").contains(&raw_id));
    // Only ONE close (the first revoke); the second is a no-op.
    assert_eq!(closer.calls().len(), 1, "no second close");
    // No SECOND audit event.
    assert!(audit_rx.try_recv().is_err(), "no second audit on re-revoke");
}

// ---------------------------------------------------------------------------
// GetCoreInfo.
// ---------------------------------------------------------------------------

/// `GetCoreInfo` returns the wired `core_pubkey` + the host/version fields from
/// the same sources `ServerCapabilities` uses.
#[tokio::test]
async fn core_info_returns_wired_pubkey_and_host_fields() {
    let (_tmp, persistence, _core_key, core_pub) = fixtures().await;
    let revoked = new_revoked_set();
    let (audit, _rx) = recording_audit();
    let closer = Arc::new(RecordingSessionCloser::default());
    let mgr = make_manager(persistence, revoked, core_pub, audit, closer);

    let info = mgr.core_info();
    assert_eq!(
        info.core_pubkey,
        core_pub.to_bytes(),
        "GetCoreInfo echoes the wired core pubkey"
    );
    assert_eq!(info.core_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(info.core_host_os, std::env::consts::OS);
    assert!(!info.core_hostname.is_empty(), "hostname present");
}

/// Sanity: a freshly-paired-style row's `paired_at` survives the round-trip and
/// the `UNIX_EPOCH`-derived clock used by `revoke_device` is monotonic with the
/// pinned-clock variant (guards against a sign/units regression).
#[tokio::test]
async fn revoke_persists_unix_seconds() {
    let (_tmp, persistence, _core_key, core_pub) = fixtures().await;
    let revoked = new_revoked_set();
    let (audit, _rx) = recording_audit();
    let closer = Arc::new(RecordingSessionCloser::default());
    let mgr = make_manager(Arc::clone(&persistence), revoked, core_pub, audit, closer);

    let pk = KeyPair::from_seed(&[0x44u8; 32]).verifying_key().to_bytes();
    let id_hex = insert_device(&persistence, &pk, "Phone", 1).await;
    let pinned = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    mgr.revoke_device_at(&id_hex, pinned).await.expect("revoke");
    let revoked_at: i64 = sqlx::query_scalar("SELECT revoked_at FROM devices WHERE id = ?")
        .bind(&id_hex)
        .fetch_one(persistence.readers())
        .await
        .expect("fetch");
    assert_eq!(revoked_at, 1_700_000_000);
}
