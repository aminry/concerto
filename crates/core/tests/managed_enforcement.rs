//! Task 211 — integration tests for the audited `managed.json` load path.
//!
//! The pure parsing + predicate behaviour (whitelist allow/deny/null/
//! empty-array, max-paired cap, `disable_remote` read, per-field
//! validation) is covered by the unit tests in
//! `crates/core/src/security/managed.rs`. This file exercises the seam the
//! free parser can't reach: [`load_managed_policy_audited`] emitting the
//! `ManagedSettingsLoaded` / `ManagedSettingsViolation` audit events
//! through a real [`AuditWriter`] + capturing subscriber.
//!
//! Tier 1 — no Core boot, no keychain. A bounded in-memory subscriber
//! captures the events; we assert kind + count. It does NOT cover the
//! downstream enforcement points (relay suppression off `disable_remote`,
//! pairing-time whitelist/cap rejection, audit-forwarder registration) —
//! those live in their owning tasks (212/214, 207/209, the deferred
//! Task-112 boot wiring) and are the Phase-2 Tier-3 checklist's business.

use std::sync::Arc;

use async_trait::async_trait;
use concerto_core::audit::{AuditEvent, AuditLogSubscriber, AuditWriterTask};
use concerto_core::security::load_managed_policy_audited;
use tempfile::TempDir;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Captures every event so the test can assert on emitted audit kinds.
struct MemorySubscriber {
    events: Arc<Mutex<Vec<AuditEvent>>>,
}

impl MemorySubscriber {
    fn new() -> (Self, Arc<Mutex<Vec<AuditEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                events: Arc::clone(&events),
            },
            events,
        )
    }
}

#[async_trait]
impl AuditLogSubscriber for MemorySubscriber {
    fn id(&self) -> &str {
        "memory"
    }
    async fn on_event(&self, event: &AuditEvent) {
        self.events.lock().await.push(event.clone());
    }
    async fn flush(&self) {}
}

/// Run `load_managed_policy_audited` against a tempdir holding `contents`
/// (or no file when `None`), drain the writer, and return the captured
/// audit kinds (as wire strings).
async fn audited_load_kinds(contents: Option<&str>) -> Vec<String> {
    let dir = TempDir::new().unwrap();
    if let Some(c) = contents {
        std::fs::write(dir.path().join("managed.json"), c).unwrap();
    }

    let (memory, captured) = MemorySubscriber::new();
    let shutdown = CancellationToken::new();
    let (writer, _drained, join) = AuditWriterTask::spawn(vec![Arc::new(memory)], shutdown.clone());

    let _policy = load_managed_policy_audited(dir.path(), &writer).expect("audited load");

    drop(writer);
    shutdown.cancel();
    let _ = join.await;

    let captured = captured.lock().await;
    captured
        .iter()
        .map(|e| e.kind.as_str().to_string())
        .collect()
}

#[tokio::test]
async fn clean_load_emits_settings_loaded_only() {
    let kinds = audited_load_kinds(Some(
        r#"{"version": 1, "disable_remote": true, "maxPairedDevicesPerUser": 4}"#,
    ))
    .await;
    assert_eq!(kinds, vec!["managed_settings_loaded".to_string()]);
}

#[tokio::test]
async fn invalid_field_emits_violation_then_loaded() {
    // A single bad field → one ManagedSettingsViolation, then the
    // ManagedSettingsLoaded summary.
    let kinds = audited_load_kinds(Some(r#"{"maxPairedDevicesPerUser": "four"}"#)).await;
    assert_eq!(
        kinds,
        vec![
            "managed_settings_violation".to_string(),
            "managed_settings_loaded".to_string(),
        ]
    );
}

#[tokio::test]
async fn malformed_file_still_audits_and_does_not_panic() {
    // Whole-file malformed JSON → one violation + the loaded summary; the
    // returned policy is the full default (the Core never refuses to boot).
    let kinds = audited_load_kinds(Some("{ this is not json")).await;
    assert_eq!(
        kinds,
        vec![
            "managed_settings_violation".to_string(),
            "managed_settings_loaded".to_string(),
        ]
    );
}

#[tokio::test]
async fn missing_file_emits_no_audit() {
    // No managed.json at all (the common personal-install case) → no audit
    // noise.
    let kinds = audited_load_kinds(None).await;
    assert!(kinds.is_empty(), "expected no audit events, got {kinds:?}");
}

#[tokio::test]
async fn unknown_version_hard_errors_with_no_audit() {
    // The forward-compat tripwire returns Err and applies no policy, so no
    // load/violation audit is emitted.
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("managed.json"), r#"{"version": 99}"#).unwrap();

    let (memory, captured) = MemorySubscriber::new();
    let shutdown = CancellationToken::new();
    let (writer, _drained, join) = AuditWriterTask::spawn(vec![Arc::new(memory)], shutdown.clone());

    let err = load_managed_policy_audited(dir.path(), &writer).unwrap_err();
    assert!(format!("{err}").contains("unsupported version"));

    drop(writer);
    shutdown.cancel();
    let _ = join.await;

    assert!(captured.lock().await.is_empty());
}
