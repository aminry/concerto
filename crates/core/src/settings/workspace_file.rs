//! Checked-in settings file readers + the live-reload watcher (Task 310).
//!
//! Two checked-in artifacts sit on the same precedence stack as the local-DB
//! layer (`design/03 §3.13` / `design/04 §3.13`):
//!
//! - `<project_root>/.concerto/workspace_settings.json` — **jsonc** (allows
//!   `//` line comments). The superset of the local-DB project row.
//! - `<repo_root>/.concerto/action_prefs.toml` — per-repo `action_prefs`
//!   overrides (the seven action keys).
//!
//! Both parse with **per-field validate-and-revert** discipline mirroring
//! `crate::security::managed::ManagedPolicyLoad`: a single malformed field
//! reverts to the *lower* layer (recording a violation) while sibling fields
//! resolve normally. The reader NEVER refuses to resolve because one field is
//! bad — a broken team artifact must not lock a developer out.
//!
//! [`WorkspaceSettingsSource`] is the `notify`-rs hot-reload watcher; it lifts
//! the [`crate::security::managed::ManagedPolicySource`] shape verbatim (watch
//! the parent `.concerto/` dir non-recursively, `std::sync::mpsc` →
//! `spawn_blocking` recv → 500 ms debounce → re-read → `watch::Sender::send`;
//! a failed mid-write re-parse leaves the previous value in place, warn-only).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use notify::{EventKind, RecursiveMode, Watcher};
use serde::Deserialize;
use tokio::sync::watch;

use crate::security::managed::HOT_RELOAD_DEBOUNCE;

/// Locked filename of the checked-in project settings file (jsonc) inside
/// `<project_root>/.concerto/`.
pub const WORKSPACE_SETTINGS_FILE_NAME: &str = "workspace_settings.json";

/// Locked filename of the per-repo checked-in action-prefs file inside
/// `<repo_root>/.concerto/`.
pub const ACTION_PREFS_FILE_NAME: &str = "action_prefs.toml";

/// Locked filename of the per-machine personal-override config inside the
/// user's `~/.concerto/`.
pub const PER_MACHINE_CONFIG_FILE_NAME: &str = "concerto.json";

/// The parsed checked-in `workspace_settings.json` payload (the fields the
/// resolver consults as its checked-in layer). All fields are optional — an
/// absent field falls through to the lower layer. The `scripts` map keeps the
/// raw `{ setup, setup_workarea, run, archive }` sub-keys.
///
/// Stored as the already-validated effective shape; the raw-vs-validated
/// distinction is carried in [`WorkspaceSettingsLoad::violations`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckedInWorkspaceSettings {
    /// `scripts.{setup,setup_workarea,run,archive}` — the sub-keys that were
    /// present and valid (a string value). Missing sub-keys are simply absent.
    pub scripts: BTreeMap<String, String>,
    /// `run_script_mode` (e.g. `"concurrent"` / `"sequential"`).
    pub run_script_mode: Option<String>,
    /// `enterprise_data_privacy` gate.
    pub enterprise_data_privacy: Option<bool>,
    /// `default_permission_mode` (validated string form; the resolver parses
    /// it into [`crate::security::PermissionMode`] when reporting provenance).
    pub default_permission_mode: Option<String>,
    /// `default_deliberation_mode`.
    pub default_deliberation_mode: Option<String>,
    /// `default_reasoning_level`.
    pub default_reasoning_level: Option<String>,
    /// `files_to_copy_rules` — the validated `{ pattern, mode }` list
    /// (`design/03 §3.10`). A malformed entry reverts the *whole* field.
    pub files_to_copy_rules: Option<Vec<super::resolver::FilesToCopyRule>>,
    /// `writable_paths_outside_worktree` — opaque path strings.
    pub writable_paths_outside_worktree: Option<Vec<String>>,
}

/// The result of parsing a checked-in `workspace_settings.json`: the effective
/// [`CheckedInWorkspaceSettings`] plus the per-field validation violations
/// collected while reverting bad fields to the lower layer. Mirrors
/// [`crate::security::managed::ManagedPolicyLoad`].
#[derive(Debug, Clone, Default)]
pub struct WorkspaceSettingsLoad {
    /// Effective settings after invalid fields reverted (dropped).
    pub settings: CheckedInWorkspaceSettings,
    /// Human-readable per-field violation messages (empty on a clean load).
    pub violations: Vec<String>,
}

/// The parsed per-repo `action_prefs.toml` (the checked-in action-prefs
/// layer). Keys are the seven action names; unknown keys are dropped with a
/// violation. Values must be strings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActionPrefsFile {
    /// Validated `action -> pref` entries (only recognised action keys).
    pub prefs: BTreeMap<String, String>,
}

/// The result of parsing an `action_prefs.toml`: the effective
/// [`ActionPrefsFile`] + the per-field violations.
#[derive(Debug, Clone, Default)]
pub struct ActionPrefsLoad {
    pub prefs: ActionPrefsFile,
    pub violations: Vec<String>,
}

/// The per-machine personal-override config
/// (`~/.concerto/concerto.json`). Only the
/// `[project_id].opt_out_of_checked_in_fields` array is consumed by Task 310;
/// the rest of the file (if any) is ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptOutConfig {
    /// `project_id -> [field names to skip the checked-in layer for]`.
    pub per_project: BTreeMap<String, Vec<String>>,
}

impl OptOutConfig {
    /// Whether `field` (its wire name, see
    /// [`super::resolver::WorkspaceSettingsField::wire_name`]) is opted out of
    /// the checked-in layer for `project_id` on this machine.
    pub fn is_opted_out(&self, project_id: &str, field: &str) -> bool {
        self.per_project
            .get(project_id)
            .is_some_and(|fields| fields.iter().any(|f| f == field))
    }

    /// Read `~/.concerto/concerto.json` if present. A missing file → an empty
    /// config (no opt-outs). A malformed file → an empty config + a
    /// `tracing::warn!` (the escape hatch is best-effort; a broken personal
    /// config must not break resolution).
    pub fn load(config_dir: &Path) -> Self {
        let path = config_dir.join(PER_MACHINE_CONFIG_FILE_NAME);
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "concerto.json read failed; no per-machine opt-outs applied"
                );
                return Self::default();
            }
        };
        Self::parse(&raw, &path)
    }

    /// Parse the per-machine config from a raw string (testable seam).
    pub fn parse(raw: &str, path: &Path) -> Self {
        #[derive(Deserialize)]
        struct PerProjectEntry {
            #[serde(default)]
            opt_out_of_checked_in_fields: Vec<String>,
        }
        // The top level is `{ "<project_id>": { opt_out_of_checked_in_fields:
        // [...] }, ... }`. Other keys per project are tolerated + ignored.
        let parsed: BTreeMap<String, PerProjectEntry> = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "concerto.json is not valid JSON; no per-machine opt-outs applied"
                );
                return Self::default();
            }
        };
        let per_project = parsed
            .into_iter()
            .filter(|(_, e)| !e.opt_out_of_checked_in_fields.is_empty())
            .map(|(k, e)| (k, e.opt_out_of_checked_in_fields))
            .collect();
        Self { per_project }
    }
}

/// Strip `//` line comments from a jsonc string, guarding string literals so
/// a `//` inside a quoted value (e.g. a URL) is preserved. Block comments are
/// not stripped (the schema does not use them); a `/* */` would survive into
/// the JSON parse and surface as a parse violation, which is the safe
/// fail-to-lower-layer behaviour.
///
/// Minimal hand-rolled scan — no new dependency (Verification 6 keeps
/// `cargo deny` green). Handles escaped quotes (`\"`) inside strings.
pub fn strip_jsonc_line_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                // Consume to end of line (drop the comment; keep the newline).
                for nc in chars.by_ref() {
                    if nc == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Parse a checked-in `workspace_settings.json` (jsonc) from a raw string,
/// collecting per-field violations. Field-by-field validate-and-revert: a bad
/// field is dropped (so the resolver falls through to the lower layer) +
/// records a violation; siblings still parse. A whole-file parse failure
/// returns empty settings + one violation (never an `Err`).
pub fn parse_workspace_settings_jsonc(raw: &str, path: &Path) -> WorkspaceSettingsLoad {
    let stripped = strip_jsonc_line_comments(raw);
    let value: serde_json::Value = match serde_json::from_str(&stripped) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "workspace_settings.json is not valid jsonc; reverting whole file to lower layer"
            );
            return WorkspaceSettingsLoad {
                settings: CheckedInWorkspaceSettings::default(),
                violations: vec![format!("workspace_settings.json is not valid jsonc: {e}")],
            };
        }
    };
    let Some(obj) = value.as_object() else {
        return WorkspaceSettingsLoad {
            settings: CheckedInWorkspaceSettings::default(),
            violations: vec!["workspace_settings.json top level must be an object".to_string()],
        };
    };

    let mut violations = Vec::new();
    let mut settings = CheckedInWorkspaceSettings::default();

    // scripts: an object of string sub-keys (setup / setup_workarea / run /
    // archive). A non-object reverts the whole field; a non-string sub-value
    // drops just that sub-key.
    if let Some(v) = obj.get("scripts") {
        match v.as_object() {
            Some(map) => {
                for (k, sv) in map {
                    match sv.as_str() {
                        Some(s) => {
                            settings.scripts.insert(k.clone(), s.to_string());
                        }
                        None => violations.push(format!(
                            "scripts.{k} must be a string; reverted to lower layer"
                        )),
                    }
                }
            }
            None => {
                violations.push("scripts must be an object; reverted to lower layer".to_string())
            }
        }
    }

    settings.run_script_mode = opt_string_field(obj, "run_script_mode", &mut violations);
    settings.enterprise_data_privacy =
        opt_bool_field(obj, "enterprise_data_privacy", &mut violations);
    settings.default_permission_mode =
        opt_string_field(obj, "default_permission_mode", &mut violations);
    settings.default_deliberation_mode =
        opt_string_field(obj, "default_deliberation_mode", &mut violations);
    settings.default_reasoning_level =
        opt_string_field(obj, "default_reasoning_level", &mut violations);
    settings.writable_paths_outside_worktree =
        opt_string_array_field(obj, "writable_paths_outside_worktree", &mut violations);

    if let Some(v) = obj.get("files_to_copy_rules") {
        settings.files_to_copy_rules =
            super::resolver::parse_files_to_copy_rules(v, &mut violations);
    }

    WorkspaceSettingsLoad {
        settings,
        violations,
    }
}

/// Parse a per-repo `action_prefs.toml`, collecting per-field violations.
/// Only the recognised action keys are kept; an unknown key or a non-string
/// value records a violation and is dropped. A whole-file parse failure
/// returns empty prefs + one violation.
pub fn parse_action_prefs_toml(raw: &str, path: &Path) -> ActionPrefsLoad {
    let value: toml::Value = match toml::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "action_prefs.toml is not valid TOML; reverting whole file to lower layer"
            );
            return ActionPrefsLoad {
                prefs: ActionPrefsFile::default(),
                violations: vec![format!("action_prefs.toml is not valid TOML: {e}")],
            };
        }
    };
    let Some(table) = value.as_table() else {
        return ActionPrefsLoad {
            prefs: ActionPrefsFile::default(),
            violations: vec!["action_prefs.toml top level must be a table".to_string()],
        };
    };

    let mut violations = Vec::new();
    let mut prefs = ActionPrefsFile::default();
    for (k, v) in table {
        if !super::resolver::ACTION_KEYS.contains(&k.as_str()) {
            violations.push(format!(
                "action_prefs.toml: unknown action key '{k}'; ignored"
            ));
            continue;
        }
        match v.as_str() {
            Some(s) => {
                prefs.prefs.insert(k.clone(), s.to_string());
            }
            None => violations.push(format!(
                "action_prefs.toml: '{k}' must be a string; reverted to lower layer"
            )),
        }
    }
    ActionPrefsLoad { prefs, violations }
}

/// Read + parse `<project_root>/.concerto/workspace_settings.json` if present.
/// A missing file → empty settings, no violations.
pub fn load_workspace_settings_file(project_concerto_dir: &Path) -> WorkspaceSettingsLoad {
    let path = project_concerto_dir.join(WORKSPACE_SETTINGS_FILE_NAME);
    match std::fs::read_to_string(&path) {
        Ok(raw) => parse_workspace_settings_jsonc(&raw, &path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => WorkspaceSettingsLoad::default(),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "workspace_settings.json read failed; reverting to lower layer"
            );
            WorkspaceSettingsLoad {
                settings: CheckedInWorkspaceSettings::default(),
                violations: vec![format!("workspace_settings.json read failed: {e}")],
            }
        }
    }
}

/// Read + parse `<repo_root>/.concerto/action_prefs.toml` if present.
/// A missing file → empty prefs, no violations.
pub fn load_action_prefs_file(repo_concerto_dir: &Path) -> ActionPrefsLoad {
    let path = repo_concerto_dir.join(ACTION_PREFS_FILE_NAME);
    match std::fs::read_to_string(&path) {
        Ok(raw) => parse_action_prefs_toml(&raw, &path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ActionPrefsLoad::default(),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "action_prefs.toml read failed; reverting to lower layer"
            );
            ActionPrefsLoad {
                prefs: ActionPrefsFile::default(),
                violations: vec![format!("action_prefs.toml read failed: {e}")],
            }
        }
    }
}

fn opt_string_field(
    obj: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    violations: &mut Vec<String>,
) -> Option<String> {
    match obj.get(field) {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(_) => {
            violations.push(format!("{field} must be a string; reverted to lower layer"));
            None
        }
    }
}

fn opt_bool_field(
    obj: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    violations: &mut Vec<String>,
) -> Option<bool> {
    match obj.get(field) {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Bool(b)) => Some(*b),
        Some(_) => {
            violations.push(format!(
                "{field} must be a boolean; reverted to lower layer"
            ));
            None
        }
    }
}

fn opt_string_array_field(
    obj: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    violations: &mut Vec<String>,
) -> Option<Vec<String>> {
    match obj.get(field) {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(s) => out.push(s.to_string()),
                    None => {
                        violations.push(format!(
                            "{field} must be an array of strings; reverted to lower layer"
                        ));
                        return None;
                    }
                }
            }
            Some(out)
        }
        Some(_) => {
            violations.push(format!(
                "{field} must be an array of strings; reverted to lower layer"
            ));
            None
        }
    }
}

/// Hot-reload broadcaster for one project's checked-in settings, mirroring
/// [`crate::security::managed::ManagedPolicySource`].
///
/// Owns a `watch::Sender<WorkspaceSettingsLoad>` and the background `notify`-rs
/// watcher. It watches the project's `.concerto/` dir (non-recursive, so
/// create/replace of a not-yet-existing `workspace_settings.json` still fires)
/// and republishes the re-parsed [`WorkspaceSettingsLoad`] whenever the file
/// mutates, debounced at [`HOT_RELOAD_DEBOUNCE`].
///
/// Per-repo `action_prefs.toml` watchers follow the identical shape; a
/// resolver that needs live action-prefs reload builds one
/// [`WorkspaceSettingsSource`] per watched `.concerto/` dir.
pub struct WorkspaceSettingsSource {
    sender: watch::Sender<WorkspaceSettingsLoad>,
    concerto_dir: PathBuf,
    settings_path: PathBuf,
    _watcher: Option<notify::RecommendedWatcher>,
    _debounce_task: Option<tokio::task::JoinHandle<()>>,
}

impl std::fmt::Debug for WorkspaceSettingsSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceSettingsSource")
            .field("concerto_dir", &self.concerto_dir)
            .finish()
    }
}

impl WorkspaceSettingsSource {
    /// Build a source rooted at `<project_root>/.concerto/`. Performs an
    /// initial synchronous parse of `workspace_settings.json`, seeds the watch
    /// channel, then spawns the `notify`-rs watcher on the `.concerto/` dir.
    /// Watcher init failures are logged + swallowed (the seed value still
    /// serves; hot reload is simply disabled).
    pub fn new(project_concerto_dir: &Path) -> Self {
        let concerto_dir = project_concerto_dir.to_path_buf();
        let settings_path = concerto_dir.join(WORKSPACE_SETTINGS_FILE_NAME);
        let initial = load_workspace_settings_file(&concerto_dir);
        let (sender, _) = watch::channel(initial);

        if let Err(e) = std::fs::create_dir_all(&concerto_dir) {
            tracing::warn!(
                dir = %concerto_dir.display(),
                error = %e,
                "workspace_settings.json: failed to ensure .concerto dir; hot reload disabled"
            );
            return Self::without_watcher(sender, concerto_dir, settings_path);
        }

        let (tx, rx) = mpsc::channel::<()>();
        let mut watcher = match notify::recommended_watcher(
            move |res: notify::Result<notify::Event>| match res {
                Ok(ev) => {
                    if matches!(
                        ev.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    ) {
                        let _ = tx.send(());
                    }
                }
                Err(e) => tracing::warn!(error = %e, "workspace_settings.json watcher error"),
            },
        ) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(error = %e, "workspace_settings.json: notify init failed; hot reload disabled");
                return Self::without_watcher(sender, concerto_dir, settings_path);
            }
        };
        if let Err(e) = watcher.watch(&concerto_dir, RecursiveMode::NonRecursive) {
            tracing::warn!(
                dir = %concerto_dir.display(),
                error = %e,
                "workspace_settings.json: notify watch() failed; hot reload disabled"
            );
            return Self::without_watcher(sender, concerto_dir, settings_path);
        }

        let task_sender = sender.clone();
        let task_dir = concerto_dir.clone();
        let task = tokio::spawn(async move {
            debounce_loop(rx, task_dir, task_sender).await;
        });

        Self {
            sender,
            concerto_dir,
            settings_path,
            _watcher: Some(watcher),
            _debounce_task: Some(task),
        }
    }

    fn without_watcher(
        sender: watch::Sender<WorkspaceSettingsLoad>,
        concerto_dir: PathBuf,
        settings_path: PathBuf,
    ) -> Self {
        Self {
            sender,
            concerto_dir,
            settings_path,
            _watcher: None,
            _debounce_task: None,
        }
    }

    /// Subscribe to checked-in settings changes. The receiver immediately
    /// yields the current value via `borrow()`; `changed().await` completes
    /// the next time the watcher republishes.
    pub fn subscribe(&self) -> watch::Receiver<WorkspaceSettingsLoad> {
        self.sender.subscribe()
    }

    /// Current parsed checked-in settings (mainly for tests + the resolver
    /// build path).
    pub fn current(&self) -> WorkspaceSettingsLoad {
        self.sender.borrow().clone()
    }

    /// The `workspace_settings.json` path the watcher observes.
    pub fn path(&self) -> &Path {
        &self.settings_path
    }
}

/// Debounce loop, lifted from
/// [`crate::security::managed`]'s `debounce_loop`: block on the `notify`-rs
/// channel in a `spawn_blocking`, sleep [`HOT_RELOAD_DEBOUNCE`], drain
/// further events, then re-parse + republish. A failed re-parse can't happen
/// here (the parser never errors), but a transient read error reverts to the
/// lower-layer empty settings with a violation, which is the safe posture.
async fn debounce_loop(
    mut rx: mpsc::Receiver<()>,
    concerto_dir: PathBuf,
    sender: watch::Sender<WorkspaceSettingsLoad>,
) {
    loop {
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
            return;
        }

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

        let load = load_workspace_settings_file(&concerto_dir);
        let _ = sender.send(load);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_jsonc_drops_line_comments_keeps_string_slashes() {
        let input = r#"{
            // a comment
            "$schema": "https://concerto.build/schemas/workspace_settings.json",
            "run_script_mode": "concurrent" // trailing comment
        }"#;
        let stripped = strip_jsonc_line_comments(input);
        assert!(!stripped.contains("a comment"));
        assert!(!stripped.contains("trailing comment"));
        // The `//` inside the URL string survives.
        assert!(stripped.contains("https://concerto.build"));
        let v: serde_json::Value = serde_json::from_str(&stripped).unwrap();
        assert_eq!(v["run_script_mode"], "concurrent");
    }

    #[test]
    fn workspace_settings_jsonc_parses_full_set() {
        let raw = r#"{
            "$schema": "https://concerto.build/schemas/workspace_settings.json",
            "scripts": { "setup": "make setup", "run": "make run" },
            "run_script_mode": "concurrent",
            "enterprise_data_privacy": true,
            "default_permission_mode": "auto",
            "files_to_copy_rules": [ { "pattern": ".env*", "mode": "copy" } ],
            "writable_paths_outside_worktree": ["/tmp/shared"]
        }"#;
        let load = parse_workspace_settings_jsonc(raw, Path::new("test"));
        assert!(load.violations.is_empty(), "{:?}", load.violations);
        let s = load.settings;
        assert_eq!(
            s.scripts.get("setup").map(String::as_str),
            Some("make setup")
        );
        assert_eq!(s.run_script_mode.as_deref(), Some("concurrent"));
        assert_eq!(s.enterprise_data_privacy, Some(true));
        assert_eq!(s.default_permission_mode.as_deref(), Some("auto"));
        assert_eq!(s.files_to_copy_rules.as_ref().unwrap().len(), 1);
        assert_eq!(
            s.writable_paths_outside_worktree.as_deref(),
            Some(&["/tmp/shared".to_string()][..])
        );
    }

    #[test]
    fn malformed_field_reverts_while_siblings_parse() {
        // `enterprise_data_privacy` is the wrong type → dropped + a violation,
        // but `run_script_mode` still resolves.
        let raw = r#"{
            "enterprise_data_privacy": "yes",
            "run_script_mode": "sequential"
        }"#;
        let load = parse_workspace_settings_jsonc(raw, Path::new("test"));
        assert_eq!(load.settings.enterprise_data_privacy, None);
        assert_eq!(load.settings.run_script_mode.as_deref(), Some("sequential"));
        assert_eq!(load.violations.len(), 1);
        assert!(load.violations[0].contains("enterprise_data_privacy"));
    }

    #[test]
    fn whole_file_garbage_reverts_to_lower_layer() {
        let load = parse_workspace_settings_jsonc("not json", Path::new("test"));
        assert_eq!(load.settings, CheckedInWorkspaceSettings::default());
        assert_eq!(load.violations.len(), 1);
    }

    #[test]
    fn action_prefs_toml_parses_known_keys_only() {
        let raw = r#"
            code_review = "Quote CONTRIBUTING.md."
            pr_create = "Use the PR template."
            bogus_action = "ignored"
        "#;
        let load = parse_action_prefs_toml(raw, Path::new("test"));
        assert_eq!(
            load.prefs.prefs.get("code_review").map(String::as_str),
            Some("Quote CONTRIBUTING.md.")
        );
        assert_eq!(
            load.prefs.prefs.get("pr_create").map(String::as_str),
            Some("Use the PR template.")
        );
        assert!(!load.prefs.prefs.contains_key("bogus_action"));
        assert_eq!(load.violations.len(), 1);
        assert!(load.violations[0].contains("bogus_action"));
    }

    #[test]
    fn opt_out_config_parses_per_project_fields() {
        let raw = r#"{
            "proj-1": { "opt_out_of_checked_in_fields": ["scripts", "run_script_mode"] },
            "proj-2": { "opt_out_of_checked_in_fields": [] }
        }"#;
        let cfg = OptOutConfig::parse(raw, Path::new("test"));
        assert!(cfg.is_opted_out("proj-1", "scripts"));
        assert!(cfg.is_opted_out("proj-1", "run_script_mode"));
        assert!(!cfg.is_opted_out("proj-1", "enterprise_data_privacy"));
        // Empty list → not stored → not opted out.
        assert!(!cfg.is_opted_out("proj-2", "scripts"));
        assert!(!cfg.is_opted_out("unknown-proj", "scripts"));
    }

    #[test]
    fn opt_out_config_malformed_is_empty() {
        let cfg = OptOutConfig::parse("garbage", Path::new("test"));
        assert!(cfg.per_project.is_empty());
    }

    #[tokio::test]
    async fn notify_reresolves_within_debounce_window() {
        // The watcher re-parses + republishes the checked-in settings after a
        // save, within a bounded window around HOT_RELOAD_DEBOUNCE (500 ms),
        // mirroring the managed.rs watcher discipline.
        let tmp = tempfile::TempDir::new().unwrap();
        let concerto = tmp.path().join(".concerto");
        std::fs::create_dir_all(&concerto).unwrap();
        std::fs::write(
            concerto.join(WORKSPACE_SETTINGS_FILE_NAME),
            r#"{ "run_script_mode": "concurrent" }"#,
        )
        .unwrap();

        let source = WorkspaceSettingsSource::new(&concerto);
        let mut rx = source.subscribe();
        assert_eq!(
            source.current().settings.run_script_mode.as_deref(),
            Some("concurrent")
        );

        // Mutate the file; the watcher should publish the new value.
        std::fs::write(
            concerto.join(WORKSPACE_SETTINGS_FILE_NAME),
            r#"{ "run_script_mode": "sequential" }"#,
        )
        .unwrap();

        // Wait for at most a few debounce windows for the republish.
        let changed = tokio::time::timeout(std::time::Duration::from_secs(5), rx.changed()).await;
        assert!(changed.is_ok(), "watcher did not republish within 5s");
        assert!(
            changed.unwrap().is_ok(),
            "watch channel closed unexpectedly"
        );
        assert_eq!(
            rx.borrow().settings.run_script_mode.as_deref(),
            Some("sequential")
        );
    }
}
