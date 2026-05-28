//! V0.1 `managed.json` reader (Task 32).
//!
//! `managed.json` is the org-controlled override layer (per `design/12
//! §3.8`). Lives at `<config_dir>/managed.json`. V0.1 reads three fields
//! only — the rest of the schema (deny lists, identity issuer, MCP
//! controls, …) lands in later phase tasks.
//!
//! Missing file → no managed policy (returns
//! [`ManagedPolicy::default`]). Malformed JSON → warn + default; the
//! Core does not refuse to boot when an org artifact is unparseable in
//! V0.1.

use std::path::Path;

use serde::Deserialize;

use crate::security::permission::PermissionMode;

/// Effective managed policy after parsing `<config_dir>/managed.json`.
///
/// V0.1 surface:
///
/// - `max_permission_mode` — caps the resolved effective mode. When
///   `Some`, [`crate::security::resolve_effective_mode`] downgrades a
///   higher resolved mode to this ceiling.
/// - `allow_yolo` — when `false`, the user cannot set `yolo` at any
///   level. (V0.1 enforcement: this is the same as
///   `max_permission_mode = Some(Auto)` for the cap path; surfaced
///   separately so future code can distinguish "yolo grayed out" UI
///   states.)
/// - `allow_bypass_destructive_guard` — when `false`, the user cannot
///   set `workareas.bypass_destructive_guard = true`.
///
/// Default values (no `managed.json`, missing keys, parse failure) leave
/// every field permissive (`None` cap, `true` allows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedPolicy {
    pub max_permission_mode: Option<PermissionMode>,
    pub allow_yolo: bool,
    pub allow_bypass_destructive_guard: bool,
}

impl Default for ManagedPolicy {
    fn default() -> Self {
        Self {
            max_permission_mode: None,
            allow_yolo: true,
            allow_bypass_destructive_guard: true,
        }
    }
}

/// On-disk schema for V0.1. Each field is optional so partial files
/// (e.g. only `max_permission_mode` set) parse cleanly.
#[derive(Debug, Default, Deserialize)]
struct ManagedFile {
    max_permission_mode: Option<String>,
    allow_yolo: Option<bool>,
    allow_bypass_destructive_guard: Option<bool>,
}

/// Locked filename inside `<config_dir>`.
pub const MANAGED_FILE_NAME: &str = "managed.json";

/// Load the managed policy from `<config_dir>/managed.json`.
///
/// Missing file: returns [`ManagedPolicy::default`] silently — most
/// installs (personal users) ship without one.
///
/// Malformed JSON or unknown `max_permission_mode` value: logs a
/// `tracing::warn!` and returns [`ManagedPolicy::default`]. The Core
/// stays running — an org artifact being broken should not lock the
/// user out of their machine.
///
/// Synchronous I/O on purpose: the file is tiny (< 1 KB in practice),
/// loaded at resolver time only, and the resolver itself is async so
/// the blocking read happens off the gRPC handler's hot path only when
/// a permission-mode RPC actually fires.
pub fn load_managed_policy(config_dir: &Path) -> ManagedPolicy {
    let path = config_dir.join(MANAGED_FILE_NAME);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ManagedPolicy::default(),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "managed.json read failed; defaulting to permissive policy"
            );
            return ManagedPolicy::default();
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
            return ManagedPolicy::default();
        }
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
                None
            }
        },
    };

    ManagedPolicy {
        max_permission_mode,
        allow_yolo: parsed.allow_yolo.unwrap_or(true),
        allow_bypass_destructive_guard: parsed.allow_bypass_destructive_guard.unwrap_or(true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn missing_file_is_default() {
        let d = TempDir::new().unwrap();
        let p = load_managed_policy(d.path());
        assert_eq!(p, ManagedPolicy::default());
    }

    #[test]
    fn cap_to_auto_parses() {
        let d = TempDir::new().unwrap();
        std::fs::write(
            d.path().join("managed.json"),
            r#"{"max_permission_mode": "auto"}"#,
        )
        .unwrap();
        let p = load_managed_policy(d.path());
        assert_eq!(p.max_permission_mode, Some(PermissionMode::Auto));
        assert!(p.allow_yolo);
        assert!(p.allow_bypass_destructive_guard);
    }

    #[test]
    fn unknown_mode_warns_and_defaults() {
        let d = TempDir::new().unwrap();
        std::fs::write(
            d.path().join("managed.json"),
            r#"{"max_permission_mode": "nope"}"#,
        )
        .unwrap();
        let p = load_managed_policy(d.path());
        assert_eq!(p.max_permission_mode, None);
    }

    #[test]
    fn malformed_json_warns_and_defaults() {
        let d = TempDir::new().unwrap();
        std::fs::write(d.path().join("managed.json"), "not json").unwrap();
        let p = load_managed_policy(d.path());
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
        let p = load_managed_policy(d.path());
        assert!(!p.allow_yolo);
        assert!(!p.allow_bypass_destructive_guard);
    }
}
