//! The per-field [`WorkspaceSettingsResolver`] + its FROZEN public surface
//! (Task 310, `design/03 §3.13` / `design/04 §3.13`).
//!
//! The resolver is a deterministic, synchronous snapshot built from the four
//! layers' already-loaded inputs (managed policy, checked-in project file +
//! per-repo action-prefs files, local-DB JSON blobs, the per-machine opt-out
//! config). Resolution is pure — every test is table-driven and CI-provable.
//! The live-reload path rebuilds the snapshot when
//! [`super::workspace_file::WorkspaceSettingsSource`] republishes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::audit::{AuditEvent, AuditKind, AuditWriter, EntityKind};
use crate::security::managed::ManagedPolicy;
use crate::security::permission::{parse_permission_mode, PermissionMode};

/// The seven per-repo action keys (`design/04 §3.13`). Frozen order +
/// spelling — the checked-in `action_prefs.toml` reader and the
/// `action_prefs.<action>` field set both reference this.
pub const ACTION_KEYS: [&str; 7] = [
    "code_review",
    "pr_create",
    "error_fix",
    "conflict_resolve",
    "branch_rename",
    "commit_message",
    "digest_summary",
];

/// Per-field provenance — which layer supplied the effective value. The UI
/// (Desktop 322+) renders this as the lock icon + tooltip. **FROZEN.**
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsSource {
    /// `~/.concerto/managed.json` (org policy). Tooltip: "Locked by org policy".
    Managed,
    /// A checked-in file (`.concerto/workspace_settings.json` or
    /// `.concerto/action_prefs.toml`). Tooltip: "Locked by
    /// `.concerto/workspace_settings.json`".
    CheckedIn,
    /// A local-DB row (`workspaces.settings_json` /
    /// `repositories.action_prefs_json`). Editable in the UI.
    LocalDb,
    /// The global default baked into the resolver. Editable in the UI.
    Default,
}

impl SettingsSource {
    /// Snake-case wire string (matches the serde rename + the audit
    /// `value_source` detail).
    pub fn as_str(self) -> &'static str {
        match self {
            SettingsSource::Managed => "managed",
            SettingsSource::CheckedIn => "checked_in",
            SettingsSource::LocalDb => "local_db",
            SettingsSource::Default => "default",
        }
    }
}

/// A resolved field value + the layer it came from. **FROZEN** — consumers
/// (309/312/321/413) destructure `{ value, source }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved<T> {
    /// The effective value (the default when no layer set it).
    pub value: T,
    /// Which layer the value came from.
    pub source: SettingsSource,
}

impl<T> Resolved<T> {
    fn new(value: T, source: SettingsSource) -> Self {
        Self { value, source }
    }
}

/// Copy mode for one files-to-copy rule (`design/03 §3.10`).
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilesToCopyMode {
    Copy,
    Symlink,
    Exclude,
}

/// One files-to-copy rule (`design/03 §3.10`). The resolved
/// `files_to_copy_rules` list (Task 309 applies it; this task only resolves
/// it). **FROZEN.**
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesToCopyRule {
    pub pattern: String,
    pub mode: FilesToCopyMode,
}

/// Every resolvable settings field. The §3.13 `workspace_settings.json`
/// superset **plus** the per-repo `action_prefs.<action>` keys
/// (`design/04 §3.13`). **FROZEN** — new fields append-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceSettingsField {
    /// `scripts.<key>` (setup / setup_workarea / run / archive).
    Script(String),
    RunScriptMode,
    EnterpriseDataPrivacy,
    DefaultPermissionMode,
    DefaultDeliberationMode,
    DefaultReasoningLevel,
    FilesToCopyRules,
    WritablePathsOutsideWorktree,
    /// Per-repo `action_prefs.<action>` for a specific repository.
    ActionPref {
        repo_id: String,
        action: String,
    },
}

impl WorkspaceSettingsField {
    /// The stable wire/opt-out name for this field. `scripts` and
    /// `action_prefs.<action>` collapse to their family name for the opt-out
    /// (the escape hatch is per-family, `design/03 §3.13`), while the audit
    /// detail uses [`Self::audit_name`] for the fully-qualified form.
    pub fn opt_out_name(&self) -> String {
        match self {
            WorkspaceSettingsField::Script(_) => "scripts".to_string(),
            WorkspaceSettingsField::RunScriptMode => "run_script_mode".to_string(),
            WorkspaceSettingsField::EnterpriseDataPrivacy => "enterprise_data_privacy".to_string(),
            WorkspaceSettingsField::DefaultPermissionMode => "default_permission_mode".to_string(),
            WorkspaceSettingsField::DefaultDeliberationMode => {
                "default_deliberation_mode".to_string()
            }
            WorkspaceSettingsField::DefaultReasoningLevel => "default_reasoning_level".to_string(),
            WorkspaceSettingsField::FilesToCopyRules => "files_to_copy_rules".to_string(),
            WorkspaceSettingsField::WritablePathsOutsideWorktree => {
                "writable_paths_outside_worktree".to_string()
            }
            WorkspaceSettingsField::ActionPref { action, .. } => format!("action_prefs.{action}"),
        }
    }

    /// The fully-qualified name used in the `WorkspaceSettingsResolved` audit
    /// detail (`scripts.<key>`, `action_prefs.<action>`, or the bare field).
    pub fn audit_name(&self) -> String {
        match self {
            WorkspaceSettingsField::Script(k) => format!("scripts.{k}"),
            WorkspaceSettingsField::ActionPref { action, .. } => format!("action_prefs.{action}"),
            other => other.opt_out_name(),
        }
    }
}

/// The local-DB workspace layer, parsed from `workspaces.settings_json`. Mirrors
/// the checked-in field set; an absent/malformed field falls through to the
/// default. Built once from the raw JSON string.
#[derive(Debug, Clone, Default)]
struct LocalDbWorkspaceSettings {
    scripts: BTreeMap<String, String>,
    run_script_mode: Option<String>,
    enterprise_data_privacy: Option<bool>,
    default_permission_mode: Option<String>,
    default_deliberation_mode: Option<String>,
    default_reasoning_level: Option<String>,
    files_to_copy_rules: Option<Vec<FilesToCopyRule>>,
    writable_paths_outside_worktree: Option<Vec<String>>,
}

impl LocalDbWorkspaceSettings {
    /// Parse `workspaces.settings_json`. Malformed JSON → all-absent (every
    /// field falls through to default), matching
    /// [`crate::security::permission::resolve_effective_mode`]'s forgiving
    /// posture for project settings.
    fn from_json(raw: &str) -> Self {
        let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(raw)
        else {
            return Self::default();
        };
        let mut out = Self::default();
        if let Some(scripts) = obj.get("scripts").and_then(|v| v.as_object()) {
            for (k, v) in scripts {
                if let Some(s) = v.as_str() {
                    out.scripts.insert(k.clone(), s.to_string());
                }
            }
        }
        out.run_script_mode = string_field(&obj, "run_script_mode");
        out.enterprise_data_privacy = obj.get("enterprise_data_privacy").and_then(|v| v.as_bool());
        out.default_permission_mode = string_field(&obj, "default_permission_mode");
        out.default_deliberation_mode = string_field(&obj, "default_deliberation_mode");
        out.default_reasoning_level = string_field(&obj, "default_reasoning_level");
        out.files_to_copy_rules = obj
            .get("files_to_copy_rules")
            .and_then(|v| serde_json::from_value::<Vec<FilesToCopyRule>>(v.clone()).ok());
        out.writable_paths_outside_worktree = obj
            .get("writable_paths_outside_worktree")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            });
        out
    }
}

fn string_field(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| v.as_str()).map(String::from)
}

/// Parse a `files_to_copy_rules` JSON array with validate-and-revert (used by
/// the checked-in jsonc reader). A malformed entry reverts the *whole* field
/// (returns `None`) + records a violation.
pub(crate) fn parse_files_to_copy_rules(
    value: &serde_json::Value,
    violations: &mut Vec<String>,
) -> Option<Vec<FilesToCopyRule>> {
    match serde_json::from_value::<Vec<FilesToCopyRule>>(value.clone()) {
        Ok(rules) => Some(rules),
        Err(e) => {
            violations.push(format!(
                "files_to_copy_rules must be a list of {{pattern, mode}} ({e}); reverted to lower layer"
            ));
            None
        }
    }
}

/// The per-field three-layer settings resolver (`design/03 §3.13`).
///
/// Built from a snapshot of the four layers; resolution is pure +
/// deterministic. Each getter walks **managed > checked-in > local-DB >
/// default** and returns the value AND its [`SettingsSource`]. The checked-in
/// layer is skipped for any field the per-machine opt-out lists for this
/// project.
pub struct WorkspaceSettingsResolver {
    workspace_id: String,
    managed: ManagedPolicy,
    checked_in: super::workspace_file::CheckedInWorkspaceSettings,
    local_db: LocalDbWorkspaceSettings,
    /// Per-repo checked-in `action_prefs.toml` (repo_id → prefs).
    repo_checked_in_action_prefs: BTreeMap<String, super::workspace_file::ActionPrefsFile>,
    /// Per-repo local-DB `repositories.action_prefs_json` (repo_id → prefs).
    repo_local_db_action_prefs: BTreeMap<String, BTreeMap<String, String>>,
    /// Opt-out field family names for this project (the checked-in layer is
    /// skipped for these).
    opted_out_fields: Vec<String>,
}

impl WorkspaceSettingsResolver {
    /// Build a resolver snapshot for `workspace_id` from the four layers'
    /// loaded inputs. The boot path supplies the DB blobs + file loads; tests
    /// build the snapshot directly via [`WorkspaceSettingsResolverBuilder`].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        workspace_id: impl Into<String>,
        managed: ManagedPolicy,
        checked_in: super::workspace_file::CheckedInWorkspaceSettings,
        local_db_settings_json: &str,
        repo_checked_in_action_prefs: BTreeMap<String, super::workspace_file::ActionPrefsFile>,
        repo_local_db_action_prefs_json: BTreeMap<String, String>,
        opted_out_fields: Vec<String>,
    ) -> Self {
        let repo_local_db_action_prefs = repo_local_db_action_prefs_json
            .into_iter()
            .map(|(repo, raw)| (repo, parse_action_prefs_json(&raw)))
            .collect();
        Self {
            workspace_id: workspace_id.into(),
            managed,
            checked_in,
            local_db: LocalDbWorkspaceSettings::from_json(local_db_settings_json),
            repo_checked_in_action_prefs,
            repo_local_db_action_prefs,
            opted_out_fields,
        }
    }

    /// The workspace id this resolver was built for.
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    fn checked_in_allowed(&self, field: &WorkspaceSettingsField) -> bool {
        !self.opted_out_fields.contains(&field.opt_out_name())
    }

    // ---- Typed convenience getters (the hot-consumer surface) ----

    /// The effective `enterprise_data_privacy` (Task 413). Managed `Some(b)`
    /// wins; then checked-in; then local DB; default `false`.
    pub fn enterprise_data_privacy(&self) -> Resolved<bool> {
        if let Some(b) = self.managed.enterprise_data_privacy() {
            return Resolved::new(b, SettingsSource::Managed);
        }
        if self.checked_in_allowed(&WorkspaceSettingsField::EnterpriseDataPrivacy) {
            if let Some(b) = self.checked_in.enterprise_data_privacy {
                return Resolved::new(b, SettingsSource::CheckedIn);
            }
        }
        if let Some(b) = self.local_db.enterprise_data_privacy {
            return Resolved::new(b, SettingsSource::LocalDb);
        }
        Resolved::new(false, SettingsSource::Default)
    }

    /// The effective `files_to_copy_rules` (Task 309). No managed layer for
    /// this field; checked-in > local DB > empty default.
    pub fn files_to_copy_rules(&self) -> Resolved<Vec<FilesToCopyRule>> {
        if self.checked_in_allowed(&WorkspaceSettingsField::FilesToCopyRules) {
            if let Some(rules) = &self.checked_in.files_to_copy_rules {
                return Resolved::new(rules.clone(), SettingsSource::CheckedIn);
            }
        }
        if let Some(rules) = &self.local_db.files_to_copy_rules {
            return Resolved::new(rules.clone(), SettingsSource::LocalDb);
        }
        Resolved::new(Vec::new(), SettingsSource::Default)
    }

    /// The effective `scripts` map (each present key with its source). Managed
    /// has no `scripts` layer; checked-in > local DB. A key present only in
    /// local DB resolves to [`SettingsSource::LocalDb`].
    pub fn scripts(&self) -> BTreeMap<String, Resolved<String>> {
        let mut out = BTreeMap::new();
        // Local DB first (lowest), then overlay checked-in (higher) so the
        // map carries the winning source per key.
        for (k, v) in &self.local_db.scripts {
            out.insert(k.clone(), Resolved::new(v.clone(), SettingsSource::LocalDb));
        }
        if self.checked_in_allowed(&WorkspaceSettingsField::Script(String::new())) {
            for (k, v) in &self.checked_in.scripts {
                out.insert(
                    k.clone(),
                    Resolved::new(v.clone(), SettingsSource::CheckedIn),
                );
            }
        }
        out
    }

    /// The effective `run_script_mode`. No managed layer; checked-in > local
    /// DB > default `"concurrent"`.
    pub fn run_script_mode(&self) -> Resolved<String> {
        if self.checked_in_allowed(&WorkspaceSettingsField::RunScriptMode) {
            if let Some(m) = &self.checked_in.run_script_mode {
                return Resolved::new(m.clone(), SettingsSource::CheckedIn);
            }
        }
        if let Some(m) = &self.local_db.run_script_mode {
            return Resolved::new(m.clone(), SettingsSource::LocalDb);
        }
        Resolved::new("concurrent".to_string(), SettingsSource::Default)
    }

    /// The **project-default-layer** `default_permission_mode` value + source.
    ///
    /// **Boundary (`design/03 §3.13` ⟷ `design/04 §3.10`):** this reports the
    /// project-default layer only, for the Settings UI + provenance consumers.
    /// The LIVE permission decision still flows through
    /// [`crate::security::resolve_effective_mode`], which walks
    /// session→workarea→workspace→project then caps on
    /// `managed.json.max_permission_mode` (a ceiling). Here, the *managed*
    /// layer is `managed.json.defaultPermissionMode` (a default, Task 310),
    /// NOT the cap — keep the two distinct.
    pub fn default_permission_mode(&self) -> Resolved<Option<PermissionMode>> {
        if let Some(m) = self.managed.default_permission_mode() {
            return Resolved::new(Some(m), SettingsSource::Managed);
        }
        if self.checked_in_allowed(&WorkspaceSettingsField::DefaultPermissionMode) {
            if let Some(s) = &self.checked_in.default_permission_mode {
                if let Ok(m) = parse_permission_mode(s) {
                    return Resolved::new(Some(m), SettingsSource::CheckedIn);
                }
            }
        }
        if let Some(s) = &self.local_db.default_permission_mode {
            if let Ok(m) = parse_permission_mode(s) {
                return Resolved::new(Some(m), SettingsSource::LocalDb);
            }
        }
        Resolved::new(None, SettingsSource::Default)
    }

    /// The effective per-repo `action_prefs.<action>` (Task 312/321 consume
    /// this via `OneShotLlm`). **FROZEN output shape:** `Resolved<Option<String>>`
    /// — `Some(pref)` when any layer set it, `None` (default) when empty.
    /// Managed has no per-repo action-prefs layer in V1.0 (the
    /// `action_prefs_pinned` managed key is a future extension); checked-in
    /// `.concerto/action_prefs.toml` > local-DB `repositories.action_prefs_json`
    /// > empty default.
    pub fn action_pref(&self, repo_id: &str, action: &str) -> Resolved<Option<String>> {
        let field = WorkspaceSettingsField::ActionPref {
            repo_id: repo_id.to_string(),
            action: action.to_string(),
        };
        if self.checked_in_allowed(&field) {
            if let Some(file) = self.repo_checked_in_action_prefs.get(repo_id) {
                if let Some(pref) = file.prefs.get(action) {
                    return Resolved::new(Some(pref.clone()), SettingsSource::CheckedIn);
                }
            }
        }
        if let Some(prefs) = self.repo_local_db_action_prefs.get(repo_id) {
            if let Some(pref) = prefs.get(action) {
                return Resolved::new(Some(pref.clone()), SettingsSource::LocalDb);
            }
        }
        Resolved::new(None, SettingsSource::Default)
    }

    /// Resolve an arbitrary field to its `{ value, source }` rendered as a
    /// JSON value (the generic per-field API the UI + audit use). Each field
    /// resolves independently (the per-FIELD invariant). Returns the effective
    /// value as `serde_json::Value` so heterogeneous field types share one
    /// signature.
    pub fn resolve_field(&self, field: &WorkspaceSettingsField) -> Resolved<serde_json::Value> {
        match field {
            WorkspaceSettingsField::EnterpriseDataPrivacy => {
                let r = self.enterprise_data_privacy();
                Resolved::new(serde_json::Value::Bool(r.value), r.source)
            }
            WorkspaceSettingsField::RunScriptMode => {
                let r = self.run_script_mode();
                Resolved::new(serde_json::Value::String(r.value), r.source)
            }
            WorkspaceSettingsField::DefaultPermissionMode => {
                let r = self.default_permission_mode();
                let v = r
                    .value
                    .map(|m| serde_json::Value::String(m.as_str().to_string()))
                    .unwrap_or(serde_json::Value::Null);
                Resolved::new(v, r.source)
            }
            WorkspaceSettingsField::DefaultDeliberationMode => self.resolve_simple_string(
                field,
                |s| s.default_deliberation_mode.clone(),
                |l| l.default_deliberation_mode.clone(),
            ),
            WorkspaceSettingsField::DefaultReasoningLevel => self.resolve_simple_string(
                field,
                |s| s.default_reasoning_level.clone(),
                |l| l.default_reasoning_level.clone(),
            ),
            WorkspaceSettingsField::FilesToCopyRules => {
                let r = self.files_to_copy_rules();
                Resolved::new(
                    serde_json::to_value(&r.value).unwrap_or(serde_json::Value::Null),
                    r.source,
                )
            }
            WorkspaceSettingsField::WritablePathsOutsideWorktree => {
                let resolved = self.resolve_string_list(
                    field,
                    self.checked_in.writable_paths_outside_worktree.as_ref(),
                    self.local_db.writable_paths_outside_worktree.as_ref(),
                );
                Resolved::new(
                    serde_json::to_value(&resolved.value).unwrap_or(serde_json::Value::Null),
                    resolved.source,
                )
            }
            WorkspaceSettingsField::Script(key) => {
                let scripts = self.scripts();
                match scripts.get(key) {
                    Some(r) => Resolved::new(serde_json::Value::String(r.value.clone()), r.source),
                    None => Resolved::new(serde_json::Value::Null, SettingsSource::Default),
                }
            }
            WorkspaceSettingsField::ActionPref { repo_id, action } => {
                let r = self.action_pref(repo_id, action);
                let v = r
                    .value
                    .map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null);
                Resolved::new(v, r.source)
            }
        }
    }

    fn resolve_simple_string(
        &self,
        field: &WorkspaceSettingsField,
        from_checked_in: impl Fn(&super::workspace_file::CheckedInWorkspaceSettings) -> Option<String>,
        from_local: impl Fn(&LocalDbWorkspaceSettings) -> Option<String>,
    ) -> Resolved<serde_json::Value> {
        if self.checked_in_allowed(field) {
            if let Some(s) = from_checked_in(&self.checked_in) {
                return Resolved::new(serde_json::Value::String(s), SettingsSource::CheckedIn);
            }
        }
        if let Some(s) = from_local(&self.local_db) {
            return Resolved::new(serde_json::Value::String(s), SettingsSource::LocalDb);
        }
        Resolved::new(serde_json::Value::Null, SettingsSource::Default)
    }

    fn resolve_string_list(
        &self,
        field: &WorkspaceSettingsField,
        checked_in: Option<&Vec<String>>,
        local: Option<&Vec<String>>,
    ) -> Resolved<Vec<String>> {
        if self.checked_in_allowed(field) {
            if let Some(v) = checked_in {
                return Resolved::new(v.clone(), SettingsSource::CheckedIn);
            }
        }
        if let Some(v) = local {
            return Resolved::new(v.clone(), SettingsSource::LocalDb);
        }
        Resolved::new(Vec::new(), SettingsSource::Default)
    }

    /// The full set of fields this resolver resolves at boot (the §3.13
    /// superset + every present `action_prefs.<action>` across the project's
    /// repos). Used by [`Self::audit_resolved_at_boot`].
    pub fn boot_fields(&self) -> Vec<WorkspaceSettingsField> {
        let mut fields = vec![
            WorkspaceSettingsField::RunScriptMode,
            WorkspaceSettingsField::EnterpriseDataPrivacy,
            WorkspaceSettingsField::DefaultPermissionMode,
            WorkspaceSettingsField::DefaultDeliberationMode,
            WorkspaceSettingsField::DefaultReasoningLevel,
            WorkspaceSettingsField::FilesToCopyRules,
            WorkspaceSettingsField::WritablePathsOutsideWorktree,
        ];
        // Scripts: the union of keys present in either layer.
        let mut script_keys: Vec<String> = self
            .checked_in
            .scripts
            .keys()
            .chain(self.local_db.scripts.keys())
            .cloned()
            .collect();
        script_keys.sort();
        script_keys.dedup();
        for k in script_keys {
            fields.push(WorkspaceSettingsField::Script(k));
        }
        // Action prefs: every (repo, action) present in either per-repo layer.
        let mut repos: Vec<String> = self
            .repo_checked_in_action_prefs
            .keys()
            .chain(self.repo_local_db_action_prefs.keys())
            .cloned()
            .collect();
        repos.sort();
        repos.dedup();
        for repo in repos {
            for action in ACTION_KEYS {
                let in_checked = self
                    .repo_checked_in_action_prefs
                    .get(&repo)
                    .is_some_and(|f| f.prefs.contains_key(action));
                let in_local = self
                    .repo_local_db_action_prefs
                    .get(&repo)
                    .is_some_and(|p| p.contains_key(action));
                if in_checked || in_local {
                    fields.push(WorkspaceSettingsField::ActionPref {
                        repo_id: repo.clone(),
                        action: action.to_string(),
                    });
                }
            }
        }
        fields
    }

    /// Emit one [`AuditKind::WorkspaceSettingsResolved`]
    /// `{workspace_id, field, value_source}` per resolved field at Core boot
    /// (`design/03 §3.13`). Mirrors `load_managed_policy_audited`'s
    /// once-at-boot call. Returns the number of events emitted (for the boot
    /// log + tests).
    pub fn audit_resolved_at_boot(&self, audit: &AuditWriter) -> usize {
        let fields = self.boot_fields();
        for field in &fields {
            let resolved = self.resolve_field(field);
            audit.append(
                AuditEvent::new(
                    AuditKind::WorkspaceSettingsResolved,
                    crate::audit::AuditActor::System,
                )
                .with_subject(EntityKind::Workspace, self.workspace_id.clone())
                .with_details(serde_json::json!({
                    "workspace_id": self.workspace_id,
                    "field": field.audit_name(),
                    "value_source": resolved.source.as_str(),
                })),
            );
        }
        fields.len()
    }
}

/// Parse a `repositories.action_prefs_json` blob into an `action -> pref` map.
/// Malformed JSON → empty. Only string values for recognised action keys are
/// kept (matching the checked-in reader's posture).
fn parse_action_prefs_json(raw: &str) -> BTreeMap<String, String> {
    let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(raw) else {
        return BTreeMap::new();
    };
    obj.into_iter()
        .filter(|(k, _)| ACTION_KEYS.contains(&k.as_str()))
        .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::workspace_file::{ActionPrefsFile, CheckedInWorkspaceSettings};

    fn checked_in_with_run_mode(mode: &str) -> CheckedInWorkspaceSettings {
        CheckedInWorkspaceSettings {
            run_script_mode: Some(mode.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn precedence_managed_wins_for_enterprise_privacy() {
        let managed = ManagedPolicy {
            enterprise_data_privacy: Some(true),
            ..ManagedPolicy::default()
        };
        let checked_in = CheckedInWorkspaceSettings {
            enterprise_data_privacy: Some(false),
            ..Default::default()
        };
        let r = WorkspaceSettingsResolver::new(
            "p1",
            managed,
            checked_in,
            r#"{"enterprise_data_privacy": false}"#,
            BTreeMap::new(),
            BTreeMap::new(),
            Vec::new(),
        );
        let res = r.enterprise_data_privacy();
        assert!(res.value);
        assert_eq!(res.source, SettingsSource::Managed);
    }

    #[test]
    fn precedence_checked_in_wins_over_local_db() {
        let r = WorkspaceSettingsResolver::new(
            "p1",
            ManagedPolicy::default(),
            checked_in_with_run_mode("sequential"),
            r#"{"run_script_mode": "concurrent"}"#,
            BTreeMap::new(),
            BTreeMap::new(),
            Vec::new(),
        );
        let res = r.run_script_mode();
        assert_eq!(res.value, "sequential");
        assert_eq!(res.source, SettingsSource::CheckedIn);
    }

    #[test]
    fn precedence_local_db_wins_when_no_higher_layer() {
        let r = WorkspaceSettingsResolver::new(
            "p1",
            ManagedPolicy::default(),
            CheckedInWorkspaceSettings::default(),
            r#"{"run_script_mode": "concurrent"}"#,
            BTreeMap::new(),
            BTreeMap::new(),
            Vec::new(),
        );
        let res = r.run_script_mode();
        assert_eq!(res.value, "concurrent");
        assert_eq!(res.source, SettingsSource::LocalDb);
    }

    #[test]
    fn precedence_default_when_no_layer_sets_it() {
        let r = WorkspaceSettingsResolver::new(
            "p1",
            ManagedPolicy::default(),
            CheckedInWorkspaceSettings::default(),
            "{}",
            BTreeMap::new(),
            BTreeMap::new(),
            Vec::new(),
        );
        let res = r.run_script_mode();
        assert_eq!(res.value, "concurrent");
        assert_eq!(res.source, SettingsSource::Default);
    }

    #[test]
    fn per_field_independence() {
        // `run_script_mode` locked checked-in, `enterprise_data_privacy` only
        // in local DB → each resolves to its own layer.
        let checked_in = CheckedInWorkspaceSettings {
            run_script_mode: Some("sequential".to_string()),
            ..Default::default()
        };
        let r = WorkspaceSettingsResolver::new(
            "p1",
            ManagedPolicy::default(),
            checked_in,
            r#"{"enterprise_data_privacy": true}"#,
            BTreeMap::new(),
            BTreeMap::new(),
            Vec::new(),
        );
        assert_eq!(r.run_script_mode().source, SettingsSource::CheckedIn);
        let edp = r.enterprise_data_privacy();
        assert!(edp.value);
        assert_eq!(edp.source, SettingsSource::LocalDb);
    }

    #[test]
    fn opt_out_skips_checked_in_layer() {
        // `run_script_mode` is opted out → the checked-in value is ignored,
        // resolution falls through to local DB.
        let r = WorkspaceSettingsResolver::new(
            "p1",
            ManagedPolicy::default(),
            checked_in_with_run_mode("sequential"),
            r#"{"run_script_mode": "concurrent"}"#,
            BTreeMap::new(),
            BTreeMap::new(),
            vec!["run_script_mode".to_string()],
        );
        let res = r.run_script_mode();
        assert_eq!(res.value, "concurrent");
        assert_eq!(res.source, SettingsSource::LocalDb);
    }

    #[test]
    fn action_pref_checked_in_over_local_db() {
        let mut checked = BTreeMap::new();
        checked.insert(
            "repo-a".to_string(),
            ActionPrefsFile {
                prefs: BTreeMap::from([("pr_create".to_string(), "checked-in pref".to_string())]),
            },
        );
        let mut local = BTreeMap::new();
        local.insert(
            "repo-a".to_string(),
            r#"{"pr_create": "db pref", "branch_rename": "db rename"}"#.to_string(),
        );
        let r = WorkspaceSettingsResolver::new(
            "p1",
            ManagedPolicy::default(),
            CheckedInWorkspaceSettings::default(),
            "{}",
            checked,
            local,
            Vec::new(),
        );
        // pr_create: checked-in wins.
        let pr = r.action_pref("repo-a", "pr_create");
        assert_eq!(pr.value.as_deref(), Some("checked-in pref"));
        assert_eq!(pr.source, SettingsSource::CheckedIn);
        // branch_rename: only in DB → local-db source.
        let br = r.action_pref("repo-a", "branch_rename");
        assert_eq!(br.value.as_deref(), Some("db rename"));
        assert_eq!(br.source, SettingsSource::LocalDb);
        // error_fix: neither → default None.
        let ef = r.action_pref("repo-a", "error_fix");
        assert_eq!(ef.value, None);
        assert_eq!(ef.source, SettingsSource::Default);
    }

    #[test]
    fn default_permission_mode_reports_project_layer_not_cap() {
        // Managed `defaultPermissionMode` is the project-default layer, NOT
        // the `max_permission_mode` cap — assert it surfaces from Managed.
        let managed = ManagedPolicy {
            default_permission_mode: Some(PermissionMode::Strict),
            max_permission_mode: Some(PermissionMode::Auto),
            ..ManagedPolicy::default()
        };
        let r = WorkspaceSettingsResolver::new(
            "p1",
            managed,
            CheckedInWorkspaceSettings::default(),
            "{}",
            BTreeMap::new(),
            BTreeMap::new(),
            Vec::new(),
        );
        let res = r.default_permission_mode();
        assert_eq!(res.value, Some(PermissionMode::Strict));
        assert_eq!(res.source, SettingsSource::Managed);
    }

    #[test]
    fn boot_fields_includes_present_scripts_and_action_prefs() {
        let checked_in = CheckedInWorkspaceSettings {
            scripts: BTreeMap::from([("setup".to_string(), "make".to_string())]),
            ..Default::default()
        };
        let mut checked_prefs = BTreeMap::new();
        checked_prefs.insert(
            "repo-a".to_string(),
            ActionPrefsFile {
                prefs: BTreeMap::from([("code_review".to_string(), "x".to_string())]),
            },
        );
        let r = WorkspaceSettingsResolver::new(
            "p1",
            ManagedPolicy::default(),
            checked_in,
            "{}",
            checked_prefs,
            BTreeMap::new(),
            Vec::new(),
        );
        let fields = r.boot_fields();
        assert!(fields.contains(&WorkspaceSettingsField::Script("setup".to_string())));
        assert!(fields.contains(&WorkspaceSettingsField::ActionPref {
            repo_id: "repo-a".to_string(),
            action: "code_review".to_string(),
        }));
        // A non-present action is NOT enumerated.
        assert!(!fields.contains(&WorkspaceSettingsField::ActionPref {
            repo_id: "repo-a".to_string(),
            action: "digest_summary".to_string(),
        }));
    }

    #[test]
    fn resolve_field_carries_source() {
        let r = WorkspaceSettingsResolver::new(
            "p1",
            ManagedPolicy::default(),
            checked_in_with_run_mode("sequential"),
            "{}",
            BTreeMap::new(),
            BTreeMap::new(),
            Vec::new(),
        );
        let res = r.resolve_field(&WorkspaceSettingsField::RunScriptMode);
        assert_eq!(res.value, serde_json::Value::String("sequential".into()));
        assert_eq!(res.source, SettingsSource::CheckedIn);
    }
}
