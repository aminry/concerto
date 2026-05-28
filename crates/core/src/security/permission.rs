//! Permission-mode inheritance + entry-ceremony enforcement (Task 32).
//!
//! Implements the chain locked in `design/03 §3.8` and `design/04 §3.10`:
//!
//! ```text
//! sessions.permission_mode
//!   → workareas.permission_mode
//!     → workspaces.permission_mode
//!       → projects.settings_json.default_permission_mode
//!         → managed.json.max_permission_mode (cap, not floor)
//!           → global default ("normal")
//! ```
//!
//! [`resolve_effective_mode`] returns the first non-NULL value walking
//! the DB chain, then caps the result against `managed.json` in Rust.
//! Sessions never carry NULL (the schema sets `DEFAULT 'normal'`), so
//! the walk's "first non-NULL" semantics terminate at the session row
//! for live sessions; the chain is meaningful at workarea-create /
//! session-spawn time when no row exists yet.
//!
//! The acknowledgement strings ([`ACK_YOLO`],
//! [`ACK_BYPASS_DESTRUCTIVE_GUARD`]) are frozen wire surface — clients
//! send the literal string verbatim; the server checks `==`. No
//! trimming, no case folding.

use std::path::Path;

use concerto_error::{Error, Result};
use concerto_persist::{Persistence, SessionId};
use sqlx::Row;

use crate::security::managed::{load_managed_policy, ManagedPolicy};

/// Permission mode taxonomy (`design/04 §3.10`).
///
/// Wire serialization uses lowercase strings (`"strict" | "normal" |
/// "auto" | "yolo"`); the proto enum
/// (`concerto_proto::v1::PermissionMode`) is the gRPC view.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PermissionMode {
    Strict,
    Normal,
    Auto,
    Yolo,
}

impl PermissionMode {
    /// Lowercase SQL/wire string form.
    pub fn as_str(self) -> &'static str {
        match self {
            PermissionMode::Strict => "strict",
            PermissionMode::Normal => "normal",
            PermissionMode::Auto => "auto",
            PermissionMode::Yolo => "yolo",
        }
    }

    /// Numeric ordering — `strict < normal < auto < yolo`. Used by
    /// [`resolve_effective_mode`]'s `managed.json` cap step (when a
    /// resolved mode exceeds the cap, downgrade to the cap).
    pub fn rank(self) -> u8 {
        match self {
            PermissionMode::Strict => 0,
            PermissionMode::Normal => 1,
            PermissionMode::Auto => 2,
            PermissionMode::Yolo => 3,
        }
    }
}

/// Parse the wire/SQL string into a [`PermissionMode`]. Rejects unknown
/// values with [`Error::Validation`] carrying the offending string for
/// diagnostics.
pub fn parse_permission_mode(s: &str) -> Result<PermissionMode> {
    match s {
        "strict" => Ok(PermissionMode::Strict),
        "normal" => Ok(PermissionMode::Normal),
        "auto" => Ok(PermissionMode::Auto),
        "yolo" => Ok(PermissionMode::Yolo),
        other => Err(Error::Validation(format!(
            "permission_mode {other:?} must be one of strict|normal|auto|yolo"
        ))),
    }
}

/// Where the resolver picked the effective mode from. Used by audit
/// (Task 44) and by the UI to render "inherited from workspace" vs
/// "set on this session" hints.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ModeSource {
    Session,
    Workarea,
    Workspace,
    Project,
    Managed,
    Default,
}

impl ModeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ModeSource::Session => "session",
            ModeSource::Workarea => "workarea",
            ModeSource::Workspace => "workspace",
            ModeSource::Project => "project",
            ModeSource::Managed => "managed",
            ModeSource::Default => "default",
        }
    }
}

/// Resolver output: the effective mode after walking the chain + cap.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct EffectiveMode {
    pub mode: PermissionMode,
    pub bypass_destructive_guard: bool,
    pub source: ModeSource,
}

/// Frozen acknowledgement literal required to set a workarea or session
/// to [`PermissionMode::Yolo`] (`design/04 §3.10`).
pub const ACK_YOLO: &str = "I understand";

/// Frozen acknowledgement literal required to set
/// `workareas.bypass_destructive_guard = true` (`design/12 §3.8`).
pub const ACK_BYPASS_DESTRUCTIVE_GUARD: &str = "I understand the risks";

/// True iff `ack` is exactly the yolo acknowledgement. No trimming, no
/// case folding — clients must send the literal string.
pub fn ack_for_yolo(ack: &str) -> bool {
    ack == ACK_YOLO
}

/// True iff `ack` is exactly the bypass-destructive-guard
/// acknowledgement.
pub fn ack_for_bypass_destructive_guard(ack: &str) -> bool {
    ack == ACK_BYPASS_DESTRUCTIVE_GUARD
}

/// Walk the inheritance chain for `session_id` and return the
/// effective mode + source.
///
/// Single SQL `SELECT` joins sessions → workareas → workspaces →
/// projects, then walks `COALESCE(session.permission_mode,
/// workarea.permission_mode, workspace.permission_mode, NULL)`. The
/// project default is pulled from `projects.settings_json` (a JSON
/// string) and resolved in Rust because SQLite's JSON1 may or may not
/// be compiled in. After the walk, [`load_managed_policy`] is consulted
/// and the result is capped (and the source switched to `Managed`) when
/// the cap is binding.
pub async fn resolve_effective_mode(
    persistence: &Persistence,
    config_dir: &Path,
    session_id: &SessionId,
) -> Result<EffectiveMode> {
    let pool = persistence.readers();
    let row = sqlx::query(
        "SELECT
            s.permission_mode          AS session_mode,
            s.bypass_destructive_guard AS session_bypass,
            wa.permission_mode         AS workarea_mode,
            wa.bypass_destructive_guard AS workarea_bypass,
            ws.permission_mode         AS workspace_mode,
            ws.bypass_destructive_guard AS workspace_bypass,
            p.settings_json            AS project_settings_json
         FROM sessions s
         JOIN workareas wa  ON wa.id = s.workarea_id
         JOIN workspaces ws ON ws.id = wa.workspace_id
         JOIN projects p    ON p.id  = ws.project_id
         WHERE s.id = ?",
    )
    .bind(&session_id.0)
    .fetch_optional(pool)
    .await
    .map_err(|e| Error::Sqlx(Box::new(e)))?;

    let row = row.ok_or_else(|| Error::NotFound(format!("session {session_id} not found")))?;

    // Walk for `mode`.
    let session_mode: Option<String> = row.get("session_mode");
    let workarea_mode: Option<String> = row.get("workarea_mode");
    let workspace_mode: Option<String> = row.get("workspace_mode");
    let project_settings_json: String = row.get("project_settings_json");

    let (mut mode, mut source) = if let Some(m) = session_mode.as_deref() {
        (parse_permission_mode(m)?, ModeSource::Session)
    } else if let Some(m) = workarea_mode.as_deref() {
        (parse_permission_mode(m)?, ModeSource::Workarea)
    } else if let Some(m) = workspace_mode.as_deref() {
        (parse_permission_mode(m)?, ModeSource::Workspace)
    } else if let Some(m) = project_default_from_settings(&project_settings_json)? {
        (m, ModeSource::Project)
    } else {
        (PermissionMode::Normal, ModeSource::Default)
    };

    // Walk for `bypass_destructive_guard`. Sessions DEFAULT 0 — but
    // explicit row override is still honoured.
    let session_bypass: i64 = row.get("session_bypass");
    let workarea_bypass: Option<i64> = row.get("workarea_bypass");
    let workspace_bypass: Option<i64> = row.get("workspace_bypass");
    let mut bypass = if session_bypass != 0 {
        true
    } else if let Some(b) = workarea_bypass {
        b != 0
    } else if let Some(b) = workspace_bypass {
        b != 0
    } else {
        false
    };

    // Apply managed.json cap.
    let managed = load_managed_policy(config_dir);
    if let Some(cap) = managed.max_permission_mode {
        if mode.rank() > cap.rank() {
            mode = cap;
            source = ModeSource::Managed;
        }
    }
    // `allow_yolo = false` also caps to Auto (UI gray-out path).
    if !managed.allow_yolo && mode == PermissionMode::Yolo {
        mode = PermissionMode::Auto;
        source = ModeSource::Managed;
    }
    // `allow_bypass_destructive_guard = false` forces the flag off
    // regardless of any inherited value.
    if !managed.allow_bypass_destructive_guard && bypass {
        bypass = false;
    }

    Ok(EffectiveMode {
        mode,
        bypass_destructive_guard: bypass,
        source,
    })
}

/// Try to read `default_permission_mode` from a project's
/// `settings_json` blob. Returns `Ok(None)` if the key is absent or the
/// JSON cannot be parsed (treating malformed project settings as
/// "inherit from below" is more forgiving than refusing to spawn).
fn project_default_from_settings(settings_json: &str) -> Result<Option<PermissionMode>> {
    let parsed: serde_json::Value = match serde_json::from_str(settings_json) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let Some(s) = parsed
        .as_object()
        .and_then(|m| m.get("default_permission_mode"))
        .and_then(|v| v.as_str())
    else {
        return Ok(None);
    };
    Ok(Some(parse_permission_mode(s)?))
}

/// Cap a requested mode against the managed policy, returning either
/// the original mode (allowed) or an [`Error::PolicyLocked`] when
/// blocked.
///
/// Used by the RPC handlers to reject `yolo` requests when
/// `managed.json` caps to `auto` (etc.). The error carries the
/// `policy.locked` wire code per `design/12 §3.8`.
pub fn enforce_managed_cap(
    requested: PermissionMode,
    managed: &ManagedPolicy,
) -> Result<PermissionMode> {
    if !managed.allow_yolo && requested == PermissionMode::Yolo {
        return Err(Error::PolicyLocked(
            "policy.locked: managed.json forbids yolo".to_string(),
        ));
    }
    if let Some(cap) = managed.max_permission_mode {
        if requested.rank() > cap.rank() {
            return Err(Error::PolicyLocked(format!(
                "policy.locked: managed.json caps permission_mode to {} (requested {})",
                cap.as_str(),
                requested.as_str()
            )));
        }
    }
    Ok(requested)
}

/// Reject a `bypass_destructive_guard = true` request when managed
/// policy disallows it. Returns `Ok(())` when permitted.
pub fn enforce_managed_bypass(enable: bool, managed: &ManagedPolicy) -> Result<()> {
    if enable && !managed.allow_bypass_destructive_guard {
        return Err(Error::PolicyLocked(
            "policy.locked: managed.json forbids bypass_destructive_guard".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_round_trips() {
        for m in [
            PermissionMode::Strict,
            PermissionMode::Normal,
            PermissionMode::Auto,
            PermissionMode::Yolo,
        ] {
            assert_eq!(parse_permission_mode(m.as_str()).unwrap(), m);
        }
    }

    #[test]
    fn parse_rejects_unknown() {
        assert!(parse_permission_mode("bogus").is_err());
    }

    #[test]
    fn rank_monotonic() {
        assert!(PermissionMode::Strict.rank() < PermissionMode::Normal.rank());
        assert!(PermissionMode::Normal.rank() < PermissionMode::Auto.rank());
        assert!(PermissionMode::Auto.rank() < PermissionMode::Yolo.rank());
    }

    #[test]
    fn ack_strings_are_exact() {
        assert!(ack_for_yolo("I understand"));
        assert!(!ack_for_yolo("i understand"));
        assert!(!ack_for_yolo("I understand "));
        assert!(ack_for_bypass_destructive_guard("I understand the risks"));
        assert!(!ack_for_bypass_destructive_guard("I understand"));
    }

    #[test]
    fn managed_cap_blocks_yolo() {
        let mp = ManagedPolicy {
            max_permission_mode: Some(PermissionMode::Auto),
            ..ManagedPolicy::default()
        };
        let err = enforce_managed_cap(PermissionMode::Yolo, &mp).unwrap_err();
        assert!(matches!(err, Error::PolicyLocked(_)));
        assert_eq!(
            enforce_managed_cap(PermissionMode::Auto, &mp).unwrap(),
            PermissionMode::Auto
        );
    }

    #[test]
    fn managed_allow_yolo_false_blocks_yolo() {
        let mp = ManagedPolicy {
            allow_yolo: false,
            ..ManagedPolicy::default()
        };
        assert!(enforce_managed_cap(PermissionMode::Yolo, &mp).is_err());
        assert!(enforce_managed_cap(PermissionMode::Auto, &mp).is_ok());
    }

    #[test]
    fn project_settings_parses_default_mode() {
        let s = r#"{"default_permission_mode": "auto"}"#;
        assert_eq!(
            project_default_from_settings(s).unwrap(),
            Some(PermissionMode::Auto)
        );
        let s = r#"{}"#;
        assert_eq!(project_default_from_settings(s).unwrap(), None);
        // Malformed → None, not error.
        let s = "not json";
        assert_eq!(project_default_from_settings(s).unwrap(), None);
    }
}
