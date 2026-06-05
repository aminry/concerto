//! Typed audit events emitted by every state-changing path (Task 44).
//!
//! Per `design/09 §3.5` the audit log is the source of truth for "what
//! changed and who changed it". Each event has:
//!
//! - `at` — wall-clock instant the event fired.
//! - `kind` — the categorical event type (frozen enum).
//! - `actor` — who caused it (device, system, or auto-mode policy).
//! - `subject_ids` — typed entity references the event touches.
//! - `details_json` — free-form structured payload. Designed to never
//!   carry raw secrets; the [`crate::log_filter::SecretsFilter`] guards
//!   the `tracing` channel and audit events are written via the JSONL
//!   subscriber straight to disk after `serde_json::to_string`.
//!
//! ## V0.1 scope
//!
//! V0.1 ships the plumbing + a handful of demo emissions
//! (workspace create, permission-mode change, tool approval). Most prior
//! tasks' state-changing paths still emit via `tracing::info!(audit.kind
//! = ...)` only; the structured-emission migration is a deliberate
//! follow-on (see Handoff Notes on `tasks/44`).

use std::time::SystemTime;

use serde::Serialize;
use serde_json::Value;

use crate::security::permission::PermissionMode;

/// One audit event. Serialized as a single JSON object per JSONL line.
///
/// `at` is the wall-clock instant at which the event was constructed.
/// The JSONL subscriber renders it as RFC3339-ish UTC; consumers reading
/// the file should treat the textual `at` field as authoritative.
#[derive(Debug, Clone, Serialize)]
pub struct AuditEvent {
    /// When the event was emitted. Serialized as a UTC string by the
    /// JSONL writer (see [`crate::audit::jsonl::serialize_event_line`]).
    #[serde(skip_serializing)]
    pub at: SystemTime,
    /// Categorical event type. Frozen at V0.1; new kinds are additive.
    pub kind: AuditKind,
    /// Who caused this event.
    pub actor: AuditActor,
    /// Typed references to entities touched by the event. Multiple ids
    /// may be present (e.g. workspace + workarea on a cascade).
    pub subject_ids: Vec<SubjectRef>,
    /// Free-form structured details. Designed to never carry raw
    /// secrets — see the module doc.
    pub details_json: Value,
}

impl AuditEvent {
    /// Build a new event with `at = SystemTime::now()`.
    ///
    /// This is the canonical constructor; tests that need to pin the
    /// timestamp can mutate `at` directly afterwards.
    pub fn new(kind: AuditKind, actor: AuditActor) -> Self {
        Self {
            at: SystemTime::now(),
            kind,
            actor,
            subject_ids: Vec::new(),
            details_json: Value::Null,
        }
    }

    /// Builder: append a subject id.
    pub fn with_subject(mut self, kind: EntityKind, id: impl Into<String>) -> Self {
        self.subject_ids.push(SubjectRef {
            kind,
            id: id.into(),
        });
        self
    }

    /// Builder: set the structured details payload.
    pub fn with_details(mut self, details: Value) -> Self {
        self.details_json = details;
        self
    }
}

/// Categorical event type. Frozen for V0.1 — additions are additive.
///
/// The wire string form (used in the JSONL `kind` field) is the
/// snake-cased variant name and lives behind [`AuditKind::as_str`].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum AuditKind {
    /// The Core's Ed25519 identity was generated for the first time (Task
    /// 206, `design/12 §3.7`). Emitted by `load_or_create_core_identity` on
    /// first launch only.
    CoreIdentityCreated,
    /// A device-pairing ceremony was initiated — a one-shot token minted
    /// (Task 207, `design/12 §3.7`). Emitted by `start_pairing`.
    DevicePairingStarted,
    /// A device completed pairing — token consumed, cert issued, `devices`
    /// row inserted (Task 207). Emitted by `complete_pairing` on success.
    DevicePairingCompleted,
    /// A pairing attempt failed — bad signature, expired/consumed token, or
    /// issuance/insert error (Task 207, `design/12 §3.7`/§8).
    DevicePairingFailed,
    /// A device was revoked — `revoked_at` persisted, the device id inserted
    /// into the shared revoked set, open sessions closed (Task 209,
    /// `design/12 §3.7`/§3.11/§7.3). Emitted by `revoke_device` on success.
    DeviceRevoked,
    WorkspaceCreated,
    WorkspaceArchived,
    WorkspaceRestored,
    WorkareaCreated,
    WorkareaArchived,
    WorkareaRestored,
    SessionStarted,
    SessionEnded,
    ToolApprovalDecided,
    ToolApprovalAutoApproved,
    ToolApprovalDenied,
    PermissionModeChanged,
    EnteredYoloMode,
    BypassDestructiveGuardEnabled,
    SecretAccessed,
    ConfigReloaded,
    RepositoryAdded,
    RepositoryCloned,
    FsmonitorRestarted,
    /// A repository arrived with a non-cone-mode sparse-checkout config
    /// (`core.sparseCheckoutCone=false`) and the Repo Manager force-set it
    /// to cone mode (Task 302, `design/02 §8` — "Non-cone-mode sparse
    /// config (pre-existing repo) → force-set to true on add, document in
    /// audit log"). The non-cone path is a known-buggy path we never
    /// expose. Emitted by `RepoManager::set_workarea_repo_cones`.
    SparseConfigForcedToCone,
    ScheduleFired,
    ScheduleSuppressed,
    DestructiveCommandIntercepted,
    /// The `managed.json` org policy file was parsed cleanly on load or
    /// hot-reload (Task 211, `design/12 §3.7`/§3.8). Emitted once per
    /// successful audited load.
    ManagedSettingsLoaded,
    /// A `managed.json` field (or the whole file) failed validation; the
    /// offending field was reverted to its default and the violation is
    /// recorded here (Task 211, `design/12 §3.7`/§3.8 — "invalid fields
    /// are flagged in the audit log (`ManagedSettingsViolation`) and the
    /// field reverts to the default"). One event per violation.
    ManagedSettingsViolation,
}

impl AuditKind {
    /// Snake-case wire string. Frozen — adding a variant is a one-line
    /// change here plus a new `match` arm.
    pub fn as_str(self) -> &'static str {
        match self {
            AuditKind::CoreIdentityCreated => "core_identity_created",
            AuditKind::DevicePairingStarted => "device_pairing_started",
            AuditKind::DevicePairingCompleted => "device_pairing_completed",
            AuditKind::DevicePairingFailed => "device_pairing_failed",
            AuditKind::DeviceRevoked => "device_revoked",
            AuditKind::WorkspaceCreated => "workspace_created",
            AuditKind::WorkspaceArchived => "workspace_archived",
            AuditKind::WorkspaceRestored => "workspace_restored",
            AuditKind::WorkareaCreated => "workarea_created",
            AuditKind::WorkareaArchived => "workarea_archived",
            AuditKind::WorkareaRestored => "workarea_restored",
            AuditKind::SessionStarted => "session_started",
            AuditKind::SessionEnded => "session_ended",
            AuditKind::ToolApprovalDecided => "tool_approval_decided",
            AuditKind::ToolApprovalAutoApproved => "tool_approval_auto_approved",
            AuditKind::ToolApprovalDenied => "tool_approval_denied",
            AuditKind::PermissionModeChanged => "permission_mode_changed",
            AuditKind::EnteredYoloMode => "entered_yolo_mode",
            AuditKind::BypassDestructiveGuardEnabled => "bypass_destructive_guard_enabled",
            AuditKind::SecretAccessed => "secret_accessed",
            AuditKind::ConfigReloaded => "config_reloaded",
            AuditKind::RepositoryAdded => "repository_added",
            AuditKind::RepositoryCloned => "repository_cloned",
            AuditKind::FsmonitorRestarted => "fsmonitor_restarted",
            AuditKind::SparseConfigForcedToCone => "sparse_config_forced_to_cone",
            AuditKind::ScheduleFired => "schedule_fired",
            AuditKind::ScheduleSuppressed => "schedule_suppressed",
            AuditKind::DestructiveCommandIntercepted => "destructive_command_intercepted",
            AuditKind::ManagedSettingsLoaded => "managed_settings_loaded",
            AuditKind::ManagedSettingsViolation => "managed_settings_violation",
        }
    }
}

impl Serialize for AuditKind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// Who caused the event.
///
/// `Device(String)` carries a device id (V0.1: free-form; the
/// device-pairing subsystem in V1.0 promotes this to a typed
/// `DeviceId`). `System` is for boot-time / shutdown-time actions
/// (e.g. crash adoption, fsmonitor restart). `AutoMode(PermissionMode)`
/// flags an event taken by the resolver without a human decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditActor {
    Device(String),
    System,
    AutoMode(PermissionMode),
}

impl Serialize for AuditActor {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(2))?;
        match self {
            AuditActor::Device(id) => {
                map.serialize_entry("kind", "device")?;
                map.serialize_entry("device_id", id)?;
            }
            AuditActor::System => {
                map.serialize_entry("kind", "system")?;
                map.serialize_entry("device_id", &Value::Null)?;
            }
            AuditActor::AutoMode(mode) => {
                map.serialize_entry("kind", "auto_mode")?;
                map.serialize_entry("permission_mode", mode.as_str())?;
            }
        }
        map.end()
    }
}

/// Entity-kind tag for [`SubjectRef`]. Frozen.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum EntityKind {
    Project,
    Repository,
    Workspace,
    Workarea,
    Session,
    ToolApproval,
    Schedule,
    Skill,
    Device,
    Secret,
}

impl EntityKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntityKind::Project => "project",
            EntityKind::Repository => "repository",
            EntityKind::Workspace => "workspace",
            EntityKind::Workarea => "workarea",
            EntityKind::Session => "session",
            EntityKind::ToolApproval => "tool_approval",
            EntityKind::Schedule => "schedule",
            EntityKind::Skill => "skill",
            EntityKind::Device => "device",
            EntityKind::Secret => "secret",
        }
    }
}

impl Serialize for EntityKind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// One typed entity reference in [`AuditEvent::subject_ids`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubjectRef {
    pub kind: EntityKind,
    pub id: String,
}
