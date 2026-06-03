//! `managed.json` reader + hot-reload watcher (Task 32 + Task 42).
//!
//! `managed.json` is the org-controlled override layer (per `design/12
//! §3.8`). Lives at `<config_dir>/managed.json`. The full V0.1 surface is
//! the union of Task 32's three fields and Task 42's two additional
//! "parsed but not enforced in V0.1" fields:
//!
//! - `version` (u32) — required when the file exists. Only `1` is
//!   supported. Higher values are rejected with [`Error::Internal`] so
//!   the user notices a forward-compat mismatch; missing/zero defaults
//!   to 1 for forward compatibility with the pre-Task-42 schema.
//! - `max_permission_mode` — caps the resolved effective mode.
//! - `allow_yolo` — when `false`, the user cannot set `yolo` at any
//!   level (RPC handlers translate this into `policy.yolo_blocked`).
//! - `allow_bypass_destructive_guard` — when `false`, the user cannot
//!   set `workareas.bypass_destructive_guard = true`
//!   (`policy.bypass_blocked`).
//! - `preamble_template_path` — parsed but not enforced in V0.1
//!   (org-customised entry-ceremony preamble; surfaced to the desktop
//!   shell in V1.0).
//! - `max_reasoning_level` — parsed but not enforced in V0.1
//!   (deliberation controls land in V1.0).
//!
//! ## V1.0 security/pairing/remote fields (Task 211)
//!
//! Per `design/12 §3.8` (managed-settings schema) + `design/11 §6.4`
//! (LAN-only mode) the parser is extended with the enforcement fields the
//! Phase-2 spine consumes. These are *parsed + exposed as predicates here*;
//! the enforcement *points* live in the consuming tasks (named below):
//!
//! - `disable_remote` (bool, snake_case per `design/11 §6.4`, default
//!   `false`) → [`ManagedPolicy::remote_disabled`]. **Task 212/214** gate
//!   relay registration + remote-accept off this; mDNS keeps publishing
//!   (LAN-only ≠ discovery-off, see `design/11 §6.4`'s three behaviours).
//! - `allowedPairingDevices` (`null`/absent = any may pair; an array of
//!   device-pubkey fingerprints = the whitelist; `[]` = hard lockdown) →
//!   [`ManagedPolicy::is_pairing_allowed`]. **Task 207** checks it before
//!   minting a cert. The fingerprint format is the hex-encoded
//!   `BLAKE2b-256(device_pubkey)` device id (`concerto_identity::device_id`)
//!   — the same string stored in `devices.id` and used as the pairing
//!   audit subject, so whitelist entries are directly comparable.
//! - `maxPairedDevicesPerUser` (`null`/absent = unlimited) →
//!   [`ManagedPolicy::max_paired_devices`]. **Task 207/209** compare it
//!   against the live active (`revoked_at IS NULL`) `devices` count at
//!   issuance.
//! - `relayUrl` → [`ManagedPolicy::relay_url`]. **Task 214** relay config.
//! - `auditForwardEndpoint` → [`ManagedPolicy::audit_forward_endpoint`].
//!   Parsed + exposed here to resolve Task 112's "where does the
//!   audit-forwarder config live?" question. **Registering** the
//!   `SyslogSubscriber`/`HttpsForwarderSubscriber` from it (the `boot.rs`
//!   subscriber-`vec!` extension) is **explicitly deferred** — 211 supplies
//!   the config field, not the subscriber wiring.
//! - `denyFilesystemPaths` (array, default `[]`) →
//!   [`ManagedPolicy::deny_filesystem_paths`]. Strings stay opaque here;
//!   the allow-list policy (`design/12 §3.5`) enforces them later.
//!
//! ### Validation + the `ManagedSettingsViolation` audit
//!
//! Validation runs on load (`design/12 §3.8`): an invalid field is
//! reverted to its default AND flagged with a [`AuditKind::ManagedSettingsViolation`]
//! audit event; a clean load emits [`AuditKind::ManagedSettingsLoaded`].
//! The free `parse_*` functions (no audit handle in reach) *collect*
//! violations into [`ManagedPolicyLoad`]; the boot/reload call site uses
//! [`load_managed_policy_audited`] (which takes an [`AuditWriter`]) to
//! actually emit them. `load_managed_policy` keeps its V0.1 signature +
//! `tracing::warn!`-only behaviour for the existing permission-mode call
//! sites that have no writer.
//!
//! Missing file → no managed policy ([`ManagedPolicy::default`]).
//! Malformed JSON → warn + default (+ a `ManagedSettingsViolation` on the
//! audited path); the Core does not refuse to boot when an org artifact is
//! unparseable. **Unknown `version` field**, by contrast, IS a hard error
//! — that's a deliberate forward-compatibility tripwire so a v2 policy file
//! isn't silently mis-enforced by an older Core binary.
//!
//! ## Hot reload (Task 42)
//!
//! [`ManagedPolicySource`] wraps a `tokio::sync::watch::Sender<ManagedPolicy>`
//! plus a background watcher task that observes
//! `<config_dir>/managed.json` via `notify`-rs and republishes the parsed
//! policy whenever the file mutates. Events are debounced at
//! [`HOT_RELOAD_DEBOUNCE`] (500 ms) so a typical editor save (write +
//! rename) only triggers one re-parse. Subscribers
//! ([`ManagedPolicySource::subscribe`]) get a `watch::Receiver` that
//! always yields the latest parsed value.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use concerto_error::{Error, Result};
use notify::{EventKind, RecursiveMode, Watcher};
use serde::Deserialize;
use tokio::sync::watch;

use crate::audit::{AuditActor, AuditEvent, AuditKind, AuditWriter};
use crate::security::permission::PermissionMode;

/// Debounce window for the hot-reload watcher. A typical editor save
/// (write + rename) fires multiple `notify` events in quick succession;
/// the debounce coalesces them into a single re-parse to keep the watch
/// channel quiet and the on-disk read bounded.
pub const HOT_RELOAD_DEBOUNCE: Duration = Duration::from_millis(500);

/// Currently-supported `managed.json` schema version. Bump this when
/// adding required fields; older versions are accepted by default
/// (missing/zero → 1), newer versions are rejected with
/// [`Error::Internal`].
pub const MANAGED_SCHEMA_VERSION: u32 = 1;

/// Locked filename inside `<config_dir>`.
pub const MANAGED_FILE_NAME: &str = "managed.json";

/// Effective managed policy after parsing `<config_dir>/managed.json`.
///
/// Default values (no `managed.json`, missing keys) leave every field
/// permissive (`None` cap, `true` allows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPolicy {
    /// Schema version of the parsed file. Always equals
    /// [`MANAGED_SCHEMA_VERSION`] when produced by [`load_managed_policy`]
    /// — higher values short-circuit with an error before this struct is
    /// returned, lower/missing values are normalised to the current
    /// supported version.
    pub version: u32,
    /// Ceiling on the resolved effective permission mode.
    /// [`crate::security::resolve_effective_mode`] downgrades a higher
    /// resolved mode to this value.
    pub max_permission_mode: Option<PermissionMode>,
    /// When `false`, RPC handlers reject any attempt to set
    /// `permission_mode = yolo`. Surfaced separately from
    /// `max_permission_mode` so the UI can render a "yolo grayed out by
    /// policy" hint distinct from "policy caps at auto".
    pub allow_yolo: bool,
    /// When `false`, RPC handlers reject
    /// `workareas.bypass_destructive_guard = true`.
    pub allow_bypass_destructive_guard: bool,
    /// Path to an org-supplied preamble template injected into elevated
    /// permission-mode entry ceremonies. Parsed in V0.1 but not yet
    /// surfaced to the desktop shell — V1.0 work.
    pub preamble_template_path: Option<PathBuf>,
    /// Org cap on the deliberation level (e.g. `"high"`, `"medium"`).
    /// Parsed in V0.1 but not yet enforced — the agent supervisor's
    /// deliberation controls land in V1.0.
    pub max_reasoning_level: Option<String>,

    // ---- V1.0 security/pairing/remote fields (Task 211) ----
    /// LAN-only toggle (`design/11 §6.4`). When `true` the Core does not
    /// register with any relay and accepts only LAN connections (mDNS
    /// keeps publishing). Read via [`Self::remote_disabled`]; the gate
    /// lives in Task 212/214's relay path. Default `false`.
    pub disable_remote: bool,
    /// Pairing whitelist (`design/12 §3.8`). `None` (JSON `null` or absent)
    /// = any device may pair; `Some(vec)` = only those device-pubkey
    /// fingerprints may pair; `Some(vec![])` = a hard lockdown (no device
    /// may pair). The fingerprint format is the hex-encoded
    /// `BLAKE2b-256(device_pubkey)` device id. Read via
    /// [`Self::is_pairing_allowed`].
    pub allowed_pairing_devices: Option<Vec<String>>,
    /// Cap on the number of paired devices (`design/12 §3.8`). `None` =
    /// unlimited. Read via [`Self::max_paired_devices`]; Task 207/209
    /// compares it against the live active `devices` count at issuance.
    pub max_paired_devices_per_user: Option<u32>,
    /// Self-hosted relay URL (`design/12 §3.8`). Read via
    /// [`Self::relay_url`]; consumed by Task 214's relay config. `None`
    /// = use the default/configured relay (or none under `disable_remote`).
    pub relay_url: Option<String>,
    /// Opt-in audit-forwarder endpoint (`design/12 §3.8`, e.g.
    /// `syslog://…`). Parsed + exposed via [`Self::audit_forward_endpoint`]
    /// to resolve Task 112's config-home question. Subscriber registration
    /// is deferred (see the module doc). `None` = no forwarding.
    pub audit_forward_endpoint: Option<String>,
    /// Filesystem paths the agent allow-list must deny (`design/12 §3.8`,
    /// §3.5). Opaque strings here — canonicalization + enforcement is the
    /// later allow-list task. Read via [`Self::deny_filesystem_paths`].
    /// Default `[]`.
    pub deny_filesystem_paths: Vec<String>,
}

impl Default for ManagedPolicy {
    fn default() -> Self {
        Self {
            version: MANAGED_SCHEMA_VERSION,
            max_permission_mode: None,
            allow_yolo: true,
            allow_bypass_destructive_guard: true,
            preamble_template_path: None,
            max_reasoning_level: None,
            disable_remote: false,
            allowed_pairing_devices: None,
            max_paired_devices_per_user: None,
            relay_url: None,
            audit_forward_endpoint: None,
            deny_filesystem_paths: Vec::new(),
        }
    }
}

impl ManagedPolicy {
    /// Whether remote access is disabled (LAN-only mode, `design/11 §6.4`).
    ///
    /// **Consumer seam:** Task 212/214 gate relay registration + the
    /// remote-accept path off this. When `true`, the Core (1) does not
    /// register with any relay, (2) **continues to publish mDNS**, (3)
    /// accepts only LAN connections. LAN-only ≠ discovery-off — the
    /// consumer must NOT also suppress mDNS.
    pub fn remote_disabled(&self) -> bool {
        self.disable_remote
    }

    /// Whether a device with `fingerprint` is allowed to pair
    /// (`design/12 §3.8`).
    ///
    /// `fingerprint` is the hex-encoded `BLAKE2b-256(device_pubkey)` device
    /// id (`concerto_identity::device_id`) — the same string stored in
    /// `devices.id`. Returns:
    /// - `true` when no whitelist is configured (`allowed_pairing_devices`
    ///   is `None` — JSON `null` or the key absent): any device may pair.
    /// - membership in the whitelist otherwise. An empty whitelist
    ///   (`Some(vec![])`) therefore denies every device (hard lockdown).
    ///
    /// **Consumer seam:** Task 207's pairing coordinator calls this before
    /// minting a cert and rejects (with a pairing-denied audit) on `false`.
    pub fn is_pairing_allowed(&self, fingerprint: &str) -> bool {
        match &self.allowed_pairing_devices {
            None => true,
            Some(whitelist) => whitelist.iter().any(|f| f == fingerprint),
        }
    }

    /// The cap on the number of paired devices, or `None` when unlimited
    /// (`design/12 §3.8`).
    ///
    /// **Consumer seam:** Task 207/209 compares this against the live count
    /// of active (`revoked_at IS NULL`) `devices` rows at issuance and
    /// rejects when already at the cap.
    pub fn max_paired_devices(&self) -> Option<u32> {
        self.max_paired_devices_per_user
    }

    /// The configured self-hosted relay URL, if any (`design/12 §3.8`).
    ///
    /// **Consumer seam:** Task 214's relay config. `None` under
    /// `disable_remote` is moot (no relay is contacted at all).
    pub fn relay_url(&self) -> Option<&str> {
        self.relay_url.as_deref()
    }

    /// The opt-in audit-forwarder endpoint, if any (`design/12 §3.8`).
    ///
    /// Provided here to resolve Task 112's "where does the forwarder config
    /// live?" question. **Registering** the matching subscriber is deferred
    /// (a `boot.rs` `vec!` extension owned by a later audit-pipeline/ops
    /// task) — 211 supplies the field only.
    pub fn audit_forward_endpoint(&self) -> Option<&str> {
        self.audit_forward_endpoint.as_deref()
    }

    /// The filesystem paths the agent allow-list must deny (`design/12
    /// §3.8`/§3.5). Opaque strings — canonicalization + enforcement is the
    /// later allow-list task.
    pub fn deny_filesystem_paths(&self) -> &[String] {
        &self.deny_filesystem_paths
    }
}

/// On-disk schema for V0.1. Each field is optional so partial files
/// (e.g. only `max_permission_mode` set) parse cleanly. `version` is
/// optional for forward compatibility with the pre-Task-42 schema; an
/// explicit higher value is rejected by [`load_managed_policy`].
///
/// V1.0 security/pairing/remote fields use `#[serde(rename = "…")]` to
/// pin the on-disk key spelling FROZEN per `design/12 §3.8`
/// (camelCase) + `design/11 §6.4` (`disable_remote` is snake_case). The
/// types are deliberately loose (`serde_json::Value` for the array/number
/// fields that need per-field validation) so a single bad field reverts
/// to its default + audits a `ManagedSettingsViolation` instead of
/// failing the whole parse.
#[derive(Debug, Default, Deserialize)]
struct ManagedFile {
    #[serde(default)]
    version: Option<u32>,
    max_permission_mode: Option<String>,
    allow_yolo: Option<bool>,
    allow_bypass_destructive_guard: Option<bool>,
    preamble_template_path: Option<PathBuf>,
    max_reasoning_level: Option<String>,

    // ---- V1.0 security/pairing/remote fields (Task 211) ----
    // `disable_remote` is snake_case (design/11 §6.4); the rest are
    // camelCase (design/12 §3.8). FROZEN spellings.
    #[serde(default)]
    disable_remote: Option<serde_json::Value>,
    #[serde(rename = "allowedPairingDevices", default)]
    allowed_pairing_devices: Option<serde_json::Value>,
    #[serde(rename = "maxPairedDevicesPerUser", default)]
    max_paired_devices_per_user: Option<serde_json::Value>,
    #[serde(rename = "relayUrl", default)]
    relay_url: Option<serde_json::Value>,
    #[serde(rename = "auditForwardEndpoint", default)]
    audit_forward_endpoint: Option<serde_json::Value>,
    #[serde(rename = "denyFilesystemPaths", default)]
    deny_filesystem_paths: Option<serde_json::Value>,
}

/// The result of parsing `managed.json` with per-field validation
/// (Task 211): the effective [`ManagedPolicy`] plus the list of
/// [`ManagedSettingsViolation`] messages collected while reverting
/// invalid fields to their defaults.
///
/// The free `parse_*` paths can't reach an [`AuditWriter`], so they
/// surface violations *structurally* in this type;
/// [`load_managed_policy_audited`] (which does hold a writer) translates
/// each into an [`AuditKind::ManagedSettingsViolation`] event.
#[derive(Debug, Clone)]
pub struct ManagedPolicyLoad {
    /// The effective policy after invalid fields reverted to default.
    pub policy: ManagedPolicy,
    /// Human-readable validation-violation messages (one per bad field,
    /// or one for a whole-file malformed/unreadable artifact). Empty on a
    /// clean load.
    pub violations: Vec<String>,
}

/// Load the managed policy from `<config_dir>/managed.json`.
///
/// Missing file: returns [`ManagedPolicy::default`] silently — most
/// installs (personal users) ship without one.
///
/// Malformed JSON or an unknown/invalid field value: logs a
/// `tracing::warn!` and returns a [`ManagedPolicy`] with that field
/// reverted to its default (the whole file reverts to default when the
/// JSON itself is unparseable). The Core stays running — an org artifact
/// being broken should not lock the user out of their machine.
///
/// **Unknown `version`** (anything other than missing/zero/1) returns
/// [`Error::Internal`] so the operator notices the mismatch. A future
/// `version: 2` Core binary will keep accepting `version: 1` files, but
/// a v1 Core binary must NOT silently mis-enforce a v2 file.
///
/// This V0.1 entry point drops the structured violation list (it has no
/// audit writer); the boot/reload call site should use
/// [`load_managed_policy_audited`] to also emit the
/// [`AuditKind::ManagedSettingsViolation`] / [`AuditKind::ManagedSettingsLoaded`]
/// events. Synchronous I/O on purpose: the file is tiny (< 1 KB in practice).
pub fn load_managed_policy(config_dir: &Path) -> Result<ManagedPolicy> {
    let path = config_dir.join(MANAGED_FILE_NAME);
    parse_managed_policy_at(&path)
}

/// Load the managed policy AND emit the load/violation audit events
/// (Task 211, `design/12 §3.7`/§3.8). This is the boot + hot-reload entry
/// point: it threads an [`AuditWriter`] in so the free parser's collected
/// violations are recorded.
///
/// - A clean parse emits one [`AuditKind::ManagedSettingsLoaded`].
/// - Each invalid field (reverted to default) emits one
///   [`AuditKind::ManagedSettingsViolation`] with a `reason` detail.
/// - A whole-file malformed/unreadable artifact emits a single
///   `ManagedSettingsViolation` and returns the full default policy
///   (never refuses to boot).
/// - An unknown `version` still returns [`Error::Internal`] (forward-compat
///   tripwire); no audit is emitted because the policy isn't applied.
///
/// A missing file is a silent default — no audit (there's nothing org
/// policy to load).
pub fn load_managed_policy_audited(
    config_dir: &Path,
    audit: &AuditWriter,
) -> Result<ManagedPolicy> {
    let path = config_dir.join(MANAGED_FILE_NAME);
    // A missing file is the common personal-install case: no policy, no
    // audit noise.
    if !path.exists() {
        return Ok(ManagedPolicy::default());
    }
    let loaded = parse_managed_policy_load_at(&path)?;
    for reason in &loaded.violations {
        audit.append(
            AuditEvent::new(AuditKind::ManagedSettingsViolation, AuditActor::System).with_details(
                serde_json::json!({
                    "path": path.display().to_string(),
                    "reason": reason,
                }),
            ),
        );
    }
    audit.append(
        AuditEvent::new(AuditKind::ManagedSettingsLoaded, AuditActor::System).with_details(
            serde_json::json!({
                "path": path.display().to_string(),
                "violations": loaded.violations.len(),
            }),
        ),
    );
    Ok(loaded.policy)
}

/// Parse a [`ManagedPolicy`] from a specific file path, dropping the
/// structured violation list. Used by the hot-reload watcher (which has
/// the path in hand) and by [`load_managed_policy`].
fn parse_managed_policy_at(path: &Path) -> Result<ManagedPolicy> {
    Ok(parse_managed_policy_load_at(path)?.policy)
}

/// Parse a [`ManagedPolicyLoad`] from a specific file path, collecting
/// per-field validation violations.
///
/// Returns `Err` only for the forward-compat `version` tripwire; every
/// other failure mode (unreadable file, malformed JSON, invalid field)
/// reverts to default + records a violation so the Core keeps booting.
fn parse_managed_policy_load_at(path: &Path) -> Result<ManagedPolicyLoad> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ManagedPolicyLoad {
                policy: ManagedPolicy::default(),
                violations: Vec::new(),
            });
        }
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "managed.json read failed; defaulting to permissive policy"
            );
            return Ok(ManagedPolicyLoad {
                policy: ManagedPolicy::default(),
                violations: vec![format!("managed.json read failed: {e}")],
            });
        }
    };
    let parsed: ManagedFile = match serde_json::from_str(&raw) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "managed.json parse failed; defaulting to permissive policy"
            );
            return Ok(ManagedPolicyLoad {
                policy: ManagedPolicy::default(),
                violations: vec![format!("managed.json is not valid JSON: {e}")],
            });
        }
    };

    let mut violations = Vec::new();

    // Forward-compat tripwire: an explicit version higher than what this
    // Core binary understands is a hard error. Missing or zero defaults
    // to the current supported version (compatible with the pre-Task-42
    // schema that omitted `version`).
    let version = parsed.version.unwrap_or(0);
    if version > MANAGED_SCHEMA_VERSION {
        return Err(Error::Internal(format!(
            "managed.json: unsupported version {version} (this Core only understands v{MANAGED_SCHEMA_VERSION})"
        )));
    }
    let version = if version == 0 {
        MANAGED_SCHEMA_VERSION
    } else {
        version
    };

    let max_permission_mode = match parsed.max_permission_mode.as_deref() {
        None => None,
        Some(s) => match crate::security::permission::parse_permission_mode(s) {
            Ok(m) => Some(m),
            Err(_) => {
                tracing::warn!(
                    path = %path.display(),
                    value = %s,
                    "managed.json max_permission_mode is not strict|normal|auto|yolo; ignoring"
                );
                violations.push(format!(
                    "max_permission_mode '{s}' is not strict|normal|auto|yolo; reverted to default"
                ));
                None
            }
        },
    };

    // ---- V1.0 security/pairing/remote fields (Task 211) ----
    let disable_remote = validate_bool(
        path,
        "disable_remote",
        parsed.disable_remote,
        false,
        &mut violations,
    );
    let allowed_pairing_devices = validate_string_array_or_null(
        path,
        "allowedPairingDevices",
        parsed.allowed_pairing_devices,
        &mut violations,
    );
    let max_paired_devices_per_user = validate_u32(
        path,
        "maxPairedDevicesPerUser",
        parsed.max_paired_devices_per_user,
        &mut violations,
    );
    let relay_url = validate_opt_string(path, "relayUrl", parsed.relay_url, &mut violations);
    let audit_forward_endpoint = validate_opt_string(
        path,
        "auditForwardEndpoint",
        parsed.audit_forward_endpoint,
        &mut violations,
    );
    let deny_filesystem_paths = validate_string_array_or_null(
        path,
        "denyFilesystemPaths",
        parsed.deny_filesystem_paths,
        &mut violations,
    )
    .unwrap_or_default();

    let policy = ManagedPolicy {
        version,
        max_permission_mode,
        allow_yolo: parsed.allow_yolo.unwrap_or(true),
        allow_bypass_destructive_guard: parsed.allow_bypass_destructive_guard.unwrap_or(true),
        preamble_template_path: parsed.preamble_template_path,
        max_reasoning_level: parsed.max_reasoning_level,
        disable_remote,
        allowed_pairing_devices,
        max_paired_devices_per_user,
        relay_url,
        audit_forward_endpoint,
        deny_filesystem_paths,
    };
    Ok(ManagedPolicyLoad { policy, violations })
}

/// Validate a JSON value expected to be a bool. Absent → `default`; a
/// non-bool → `default` + a violation. (`serde_json::Value::Null` is
/// treated as absent.)
fn validate_bool(
    path: &Path,
    field: &str,
    value: Option<serde_json::Value>,
    default: bool,
    violations: &mut Vec<String>,
) -> bool {
    match value {
        None | Some(serde_json::Value::Null) => default,
        Some(serde_json::Value::Bool(b)) => b,
        Some(other) => {
            tracing::warn!(
                path = %path.display(),
                field,
                "managed.json field is not a boolean; reverting to default"
            );
            violations.push(format!(
                "{field} must be a boolean, got {}; reverted to default",
                json_type_name(&other)
            ));
            default
        }
    }
}

/// Validate a JSON value expected to be a non-negative integer in `u32`
/// range. Absent/`null` → `None` (unlimited); a non-integer / out-of-range
/// → `None` + a violation.
fn validate_u32(
    path: &Path,
    field: &str,
    value: Option<serde_json::Value>,
    violations: &mut Vec<String>,
) -> Option<u32> {
    match value {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Number(n)) => match n.as_u64() {
            Some(v) if v <= u64::from(u32::MAX) => Some(v as u32),
            _ => {
                tracing::warn!(
                    path = %path.display(),
                    field,
                    "managed.json field is not a u32; reverting to default"
                );
                violations.push(format!(
                    "{field} must be a non-negative integer ≤ {}, got {n}; reverted to default",
                    u32::MAX
                ));
                None
            }
        },
        Some(other) => {
            tracing::warn!(
                path = %path.display(),
                field,
                "managed.json field is not a number; reverting to default"
            );
            violations.push(format!(
                "{field} must be a non-negative integer, got {}; reverted to default",
                json_type_name(&other)
            ));
            None
        }
    }
}

/// Validate a JSON value expected to be a string (or `null`/absent →
/// `None`). A non-string → `None` + a violation.
fn validate_opt_string(
    path: &Path,
    field: &str,
    value: Option<serde_json::Value>,
    violations: &mut Vec<String>,
) -> Option<String> {
    match value {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s),
        Some(other) => {
            tracing::warn!(
                path = %path.display(),
                field,
                "managed.json field is not a string; reverting to default"
            );
            violations.push(format!(
                "{field} must be a string, got {}; reverted to default",
                json_type_name(&other)
            ));
            None
        }
    }
}

/// Validate a JSON value expected to be an array of strings (or
/// `null`/absent → `None`). The `null`-vs-`[]` distinction is preserved:
/// `null`/absent → `None` ("any"), `[]` → `Some(vec![])` ("none"). A
/// non-array, or an array with a non-string element → `None` + a
/// violation (the whole field reverts).
fn validate_string_array_or_null(
    path: &Path,
    field: &str,
    value: Option<serde_json::Value>,
    violations: &mut Vec<String>,
) -> Option<Vec<String>> {
    match value {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    serde_json::Value::String(s) => out.push(s),
                    other => {
                        tracing::warn!(
                            path = %path.display(),
                            field,
                            "managed.json array element is not a string; reverting field to default"
                        );
                        violations.push(format!(
                            "{field} must be an array of strings; element {} is not a string; reverted to default",
                            json_type_name(&other)
                        ));
                        return None;
                    }
                }
            }
            Some(out)
        }
        Some(other) => {
            tracing::warn!(
                path = %path.display(),
                field,
                "managed.json field is not an array; reverting to default"
            );
            violations.push(format!(
                "{field} must be null or an array of strings, got {}; reverted to default",
                json_type_name(&other)
            ));
            None
        }
    }
}

/// Short type name for a `serde_json::Value`, used in violation messages.
fn json_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Hot-reload broadcaster for the managed policy.
///
/// Owns a `tokio::sync::watch::Sender<ManagedPolicy>` and the background
/// `notify`-rs watcher task. Subscribers obtain a
/// [`watch::Receiver<ManagedPolicy>`] via [`Self::subscribe`] and either
/// poll the current value with `borrow()` or await mutations with
/// `changed()`. The receiver is `Clone`, so each consumer can hold its
/// own copy without serialising on a shared mutex.
///
/// V0.1 wiring: the gRPC `Server` constructor builds one
/// [`ManagedPolicySource`] per process and passes the receiver into the
/// per-RPC enforcement helpers as needed. The synchronous
/// [`load_managed_policy`] is still the path used inside individual
/// handler methods — the watch channel exists to let long-lived
/// subscribers (e.g. future cached resolvers) observe changes without
/// re-reading the file.
///
/// The watcher task is parked on a `std::sync::mpsc::Receiver` fed by
/// `notify`-rs's event callback (which runs on `notify`'s own thread).
/// On every event the task waits [`HOT_RELOAD_DEBOUNCE`] for further
/// activity, then re-parses the file and publishes the result via
/// `watch::Sender::send`. Failed re-parses (e.g. mid-write reads) log a
/// `tracing::warn!` and leave the previous policy in place — callers
/// see the next successful parse on the next event burst.
pub struct ManagedPolicySource {
    sender: watch::Sender<ManagedPolicy>,
    path: PathBuf,
    // The `notify::RecommendedWatcher` must outlive the task it feeds —
    // stash it here so dropping the source tears the watcher down.
    _watcher: Option<notify::RecommendedWatcher>,
    // The debounce task's join handle. Detached on drop because the
    // `notify`-rs event channel closes and the task exits naturally.
    _debounce_task: Option<tokio::task::JoinHandle<()>>,
}

impl std::fmt::Debug for ManagedPolicySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedPolicySource")
            .field("path", &self.path)
            .field("current", &*self.sender.borrow())
            .finish()
    }
}

impl ManagedPolicySource {
    /// Build a source rooted at `<config_dir>/managed.json`. Performs an
    /// initial synchronous parse, seeds the watch channel, then spawns
    /// the `notify`-rs watcher on the parent directory (so events for a
    /// not-yet-existing `managed.json` still arrive).
    ///
    /// Errors from the initial parse (e.g. `version > supported`) are
    /// returned to the caller; transient I/O failures during a later
    /// reload are logged and swallowed (the previous policy stays in
    /// effect).
    pub fn new(config_dir: &Path) -> Result<Self> {
        let path = config_dir.join(MANAGED_FILE_NAME);
        let initial = parse_managed_policy_at(&path)?;
        let (sender, _) = watch::channel(initial);

        // Spawn the watcher (best-effort: missing config_dir means no
        // watcher, but the caller can still consult the static parser
        // via the watch sender's seed value).
        let watch_dir = config_dir.to_path_buf();
        if let Err(e) = std::fs::create_dir_all(&watch_dir) {
            tracing::warn!(
                dir = %watch_dir.display(),
                error = %e,
                "managed.json: failed to ensure config dir for watcher; hot reload disabled"
            );
            return Ok(Self {
                sender,
                path,
                _watcher: None,
                _debounce_task: None,
            });
        }

        let (tx, rx) = mpsc::channel::<()>();
        let mut watcher = match notify::recommended_watcher(
            move |res: notify::Result<notify::Event>| match res {
                Ok(ev) => {
                    // Only react to mutations that could change the file
                    // contents: create / modify / remove. Access events
                    // are noise.
                    if matches!(
                        ev.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    ) {
                        let _ = tx.send(());
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "managed.json watcher error");
                }
            },
        ) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(error = %e, "managed.json: notify watcher init failed; hot reload disabled");
                return Ok(Self {
                    sender,
                    path,
                    _watcher: None,
                    _debounce_task: None,
                });
            }
        };
        // Watch the parent directory (non-recursive) so events for
        // create/replace of the managed.json file still arrive even when
        // the file doesn't yet exist at startup.
        if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
            tracing::warn!(
                dir = %watch_dir.display(),
                error = %e,
                "managed.json: notify watch() failed; hot reload disabled"
            );
            return Ok(Self {
                sender,
                path,
                _watcher: None,
                _debounce_task: None,
            });
        }

        let task_sender = sender.clone();
        let task_path = path.clone();
        let task = tokio::spawn(async move {
            debounce_loop(rx, task_path, task_sender).await;
        });

        Ok(Self {
            sender,
            path,
            _watcher: Some(watcher),
            _debounce_task: Some(task),
        })
    }

    /// Subscribe to policy changes. The returned receiver immediately
    /// yields the current value via `borrow()`; `changed().await`
    /// completes the next time the watcher publishes a new policy.
    pub fn subscribe(&self) -> watch::Receiver<ManagedPolicy> {
        self.sender.subscribe()
    }

    /// Current parsed policy. Mainly useful for tests; production code
    /// should `subscribe()` so it sees subsequent reloads.
    pub fn current(&self) -> ManagedPolicy {
        self.sender.borrow().clone()
    }

    /// Path the watcher is observing.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Debounce loop running on the tokio runtime. Blocks on the
/// `notify`-rs event channel in a `spawn_blocking` because the channel
/// is `std::sync::mpsc::Receiver` and would otherwise stall a runtime
/// worker thread. After receiving an event the loop drains further
/// pending events, sleeps [`HOT_RELOAD_DEBOUNCE`], then re-parses and
/// republishes the policy.
async fn debounce_loop(
    mut rx: mpsc::Receiver<()>,
    path: PathBuf,
    sender: watch::Sender<ManagedPolicy>,
) {
    loop {
        // Block on the notify channel inside a spawn_blocking so the
        // tokio worker stays free. Receiver is moved into the blocking
        // task and returned back so the loop can re-park on it.
        let (handed_back, ok) = match tokio::task::spawn_blocking(move || match rx.recv() {
            Ok(()) => (rx, true),
            Err(_) => (rx, false),
        })
        .await
        {
            Ok(pair) => pair,
            Err(_) => return,
        };
        rx = handed_back;
        if !ok {
            // Channel closed (watcher dropped) → stop.
            return;
        }

        // Debounce: sleep, then drain any further events that arrived
        // during the sleep window. A typical editor save (write + rename)
        // fires multiple events in quick succession; we want one re-parse.
        tokio::time::sleep(HOT_RELOAD_DEBOUNCE).await;
        rx = match tokio::task::spawn_blocking(move || {
            while rx.try_recv().is_ok() {}
            rx
        })
        .await
        {
            Ok(r) => r,
            Err(_) => return,
        };

        // Re-parse + publish. A failed parse leaves the previous policy
        // in place — `tracing::warn!` lives inside `parse_managed_policy_at`.
        match parse_managed_policy_at(&path) {
            Ok(policy) => {
                // `watch::Sender::send` returns Err iff there are no
                // receivers; that's fine — the seed value is still cached
                // and the next subscriber will see the latest write.
                let _ = sender.send(policy);
            }
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "managed.json: reload failed; previous policy retained"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_file_is_default() {
        let d = TempDir::new().unwrap();
        let p = load_managed_policy(d.path()).unwrap();
        assert_eq!(p, ManagedPolicy::default());
        assert_eq!(p.version, MANAGED_SCHEMA_VERSION);
    }

    #[test]
    fn cap_to_auto_parses() {
        let d = TempDir::new().unwrap();
        std::fs::write(
            d.path().join("managed.json"),
            r#"{"version": 1, "max_permission_mode": "auto"}"#,
        )
        .unwrap();
        let p = load_managed_policy(d.path()).unwrap();
        assert_eq!(p.max_permission_mode, Some(PermissionMode::Auto));
        assert!(p.allow_yolo);
        assert!(p.allow_bypass_destructive_guard);
        assert_eq!(p.version, 1);
    }

    #[test]
    fn unknown_mode_warns_and_defaults() {
        let d = TempDir::new().unwrap();
        std::fs::write(
            d.path().join("managed.json"),
            r#"{"max_permission_mode": "nope"}"#,
        )
        .unwrap();
        let p = load_managed_policy(d.path()).unwrap();
        assert_eq!(p.max_permission_mode, None);
    }

    #[test]
    fn malformed_json_warns_and_defaults() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("managed.json"), "not json").unwrap();
        let p = load_managed_policy(d.path()).unwrap();
        assert_eq!(p, ManagedPolicy::default());
    }

    #[test]
    fn allow_flags_parse() {
        let d = TempDir::new().unwrap();
        std::fs::write(
            d.path().join("managed.json"),
            r#"{"allow_yolo": false, "allow_bypass_destructive_guard": false}"#,
        )
        .unwrap();
        let p = load_managed_policy(d.path()).unwrap();
        assert!(!p.allow_yolo);
        assert!(!p.allow_bypass_destructive_guard);
    }

    #[test]
    fn missing_version_defaults_to_one() {
        let d = TempDir::new().unwrap();
        std::fs::write(
            d.path().join("managed.json"),
            r#"{"max_permission_mode": "auto"}"#,
        )
        .unwrap();
        let p = load_managed_policy(d.path()).unwrap();
        assert_eq!(p.version, 1);
    }

    #[test]
    fn explicit_version_one_parses() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("managed.json"), r#"{"version": 1}"#).unwrap();
        let p = load_managed_policy(d.path()).unwrap();
        assert_eq!(p.version, 1);
    }

    #[test]
    fn future_version_errors() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("managed.json"), r#"{"version": 2}"#).unwrap();
        let err = load_managed_policy(d.path()).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("unsupported version"),
            "expected version error, got: {msg}"
        );
    }

    #[test]
    fn preamble_and_reasoning_fields_parse() {
        let d = TempDir::new().unwrap();
        std::fs::write(
            d.path().join("managed.json"),
            r#"{"preamble_template_path": "/etc/preamble.md", "max_reasoning_level": "medium"}"#,
        )
        .unwrap();
        let p = load_managed_policy(d.path()).unwrap();
        assert_eq!(
            p.preamble_template_path,
            Some(PathBuf::from("/etc/preamble.md"))
        );
        assert_eq!(p.max_reasoning_level.as_deref(), Some("medium"));
    }

    // ---- Task 211: V1.0 security/pairing/remote field parsing ----

    fn write_policy(json: &str) -> (TempDir, ManagedPolicyLoad) {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("managed.json"), json).unwrap();
        let path = d.path().join("managed.json");
        let load = parse_managed_policy_load_at(&path).unwrap();
        (d, load)
    }

    #[test]
    fn v1_security_defaults_when_absent() {
        // No V1.0 keys present → all default, no violations.
        let (_d, load) = write_policy(r#"{"version": 1}"#);
        assert_eq!(load.violations, Vec::<String>::new());
        let p = load.policy;
        assert!(!p.remote_disabled());
        assert!(p.is_pairing_allowed("anything")); // None → any
        assert_eq!(p.max_paired_devices(), None);
        assert_eq!(p.relay_url(), None);
        assert_eq!(p.audit_forward_endpoint(), None);
        assert!(p.deny_filesystem_paths().is_empty());
    }

    #[test]
    fn frozen_keys_parse_full_policy() {
        let (_d, load) = write_policy(
            r#"{
                "version": 1,
                "disable_remote": true,
                "allowedPairingDevices": ["aa", "bb"],
                "maxPairedDevicesPerUser": 4,
                "relayUrl": "https://relay.example/concerto",
                "auditForwardEndpoint": "syslog://splunk.example:514",
                "denyFilesystemPaths": ["~/.aws", "/opt/secrets"]
            }"#,
        );
        assert_eq!(load.violations, Vec::<String>::new());
        let p = load.policy;
        assert!(p.remote_disabled());
        assert!(p.is_pairing_allowed("aa"));
        assert!(p.is_pairing_allowed("bb"));
        assert!(!p.is_pairing_allowed("cc"));
        assert_eq!(p.max_paired_devices(), Some(4));
        assert_eq!(p.relay_url(), Some("https://relay.example/concerto"));
        assert_eq!(
            p.audit_forward_endpoint(),
            Some("syslog://splunk.example:514")
        );
        assert_eq!(p.deny_filesystem_paths(), &["~/.aws", "/opt/secrets"]);
    }

    #[test]
    fn disable_remote_false_and_absent_both_read_false() {
        let (_d, load) = write_policy(r#"{"disable_remote": false}"#);
        assert!(!load.policy.remote_disabled());
        let (_d2, load2) = write_policy(r#"{}"#);
        assert!(!load2.policy.remote_disabled());
    }

    #[test]
    fn pairing_whitelist_allow_deny() {
        let (_d, load) = write_policy(r#"{"allowedPairingDevices": ["fp-allowed"]}"#);
        let p = load.policy;
        assert!(p.is_pairing_allowed("fp-allowed"));
        assert!(!p.is_pairing_allowed("fp-other"));
    }

    #[test]
    fn pairing_null_means_any() {
        let (_d, load) = write_policy(r#"{"allowedPairingDevices": null}"#);
        assert_eq!(load.policy.allowed_pairing_devices, None);
        assert!(load.policy.is_pairing_allowed("whoever"));
    }

    #[test]
    fn pairing_empty_array_means_hard_lockdown() {
        // `[]` is distinct from `null`: Some(empty) → deny everyone.
        let (_d, load) = write_policy(r#"{"allowedPairingDevices": []}"#);
        assert_eq!(load.policy.allowed_pairing_devices, Some(vec![]));
        assert!(!load.policy.is_pairing_allowed("anyone"));
    }

    #[test]
    fn max_paired_devices_cap_and_unset() {
        let (_d, load) = write_policy(r#"{"maxPairedDevicesPerUser": 2}"#);
        assert_eq!(load.policy.max_paired_devices(), Some(2));
        let (_d2, load2) = write_policy(r#"{}"#);
        assert_eq!(load2.policy.max_paired_devices(), None);
    }

    #[test]
    fn invalid_max_paired_devices_reverts_and_violates() {
        // A non-numeric value reverts the field + records exactly one
        // violation; valid sibling fields still parse.
        let (_d, load) =
            write_policy(r#"{"maxPairedDevicesPerUser": "four", "disable_remote": true}"#);
        assert_eq!(load.policy.max_paired_devices(), None);
        assert!(load.policy.remote_disabled(), "sibling field still parsed");
        assert_eq!(load.violations.len(), 1);
        assert!(load.violations[0].contains("maxPairedDevicesPerUser"));
    }

    #[test]
    fn invalid_allowed_pairing_devices_type_reverts_and_violates() {
        // Not an array/null → revert to None (any) + a violation.
        let (_d, load) = write_policy(r#"{"allowedPairingDevices": 42}"#);
        assert_eq!(load.policy.allowed_pairing_devices, None);
        assert_eq!(load.violations.len(), 1);
        assert!(load.violations[0].contains("allowedPairingDevices"));
    }

    #[test]
    fn invalid_pairing_array_element_reverts_whole_field() {
        // An array with a non-string element reverts the whole field.
        let (_d, load) = write_policy(r#"{"allowedPairingDevices": ["ok", 7]}"#);
        assert_eq!(load.policy.allowed_pairing_devices, None);
        assert_eq!(load.violations.len(), 1);
    }

    #[test]
    fn invalid_disable_remote_type_reverts_to_false() {
        let (_d, load) = write_policy(r#"{"disable_remote": "yes"}"#);
        assert!(!load.policy.remote_disabled());
        assert_eq!(load.violations.len(), 1);
        assert!(load.violations[0].contains("disable_remote"));
    }

    #[test]
    fn invalid_relay_url_type_reverts_to_none() {
        let (_d, load) = write_policy(r#"{"relayUrl": 123}"#);
        assert_eq!(load.policy.relay_url(), None);
        assert_eq!(load.violations.len(), 1);
        assert!(load.violations[0].contains("relayUrl"));
    }

    #[test]
    fn malformed_json_full_default_with_violation() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("managed.json"), "{ not json").unwrap();
        let load = parse_managed_policy_load_at(&d.path().join("managed.json")).unwrap();
        assert_eq!(load.policy, ManagedPolicy::default());
        assert_eq!(load.violations.len(), 1);
        assert!(load.violations[0].contains("not valid JSON"));
    }

    #[test]
    fn unknown_version_still_hard_errors_on_collecting_path() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("managed.json"), r#"{"version": 9}"#).unwrap();
        let err = parse_managed_policy_load_at(&d.path().join("managed.json")).unwrap_err();
        assert!(format!("{err}").contains("unsupported version"));
    }

    #[test]
    fn v0_load_managed_policy_drops_violations_but_keeps_policy() {
        // The V0.1 entry point still returns a usable (defaulted) policy
        // for an invalid field, just without the structured violation list.
        let d = TempDir::new().unwrap();
        std::fs::write(
            d.path().join("managed.json"),
            r#"{"maxPairedDevicesPerUser": "bad", "disable_remote": true}"#,
        )
        .unwrap();
        let p = load_managed_policy(d.path()).unwrap();
        assert_eq!(p.max_paired_devices(), None);
        assert!(p.remote_disabled());
    }

    #[test]
    fn fingerprint_format_matches_hex_device_id() {
        // The whitelist entry format is the hex-encoded BLAKE2b-256 device
        // id (`concerto_identity::device_id`) — the same string stored in
        // `devices.id`. Assert a real derived id matches against the list.
        let pubkey = [7u8; 32];
        let fingerprint = hex::encode(concerto_identity::device_id(&pubkey));
        let json = format!(r#"{{"allowedPairingDevices": ["{fingerprint}"]}}"#);
        let (_d, load) = write_policy(&json);
        assert!(load.policy.is_pairing_allowed(&fingerprint));
        assert!(!load.policy.is_pairing_allowed("deadbeef"));
    }
}
