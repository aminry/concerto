//! Three-layer project/repository settings precedence resolver (Task 310,
//! `design/03 §3.13` + `design/04 §3.13`).
//!
//! ## The problem this module solves
//!
//! A team wants `scripts.setup` checked into git so every developer runs the
//! same setup; one developer wants to override it locally; the org wants to
//! disable `yolo` regardless of project. Concerto's answer is a **three-layer
//! precedence stack**, resolved **per field**:
//!
//! ```text
//! managed.json  >  checked-in files  >  local DB rows  >  global defaults
//! ```
//!
//! - **Managed** — `~/.concerto/managed.json` (the org override layer, read
//!   through [`crate::security::managed::ManagedPolicy`]; Task 211 + the
//!   Task 310 project-layer fields).
//! - **Checked-in** — `<project_root>/.concerto/project_settings.json`
//!   (jsonc) for project fields, plus per-repo
//!   `<repo_root>/.concerto/action_prefs.toml` for the per-repo
//!   `action_prefs.<action>` fields. Travels with the git history.
//! - **Local DB** — `projects.settings_json` (project fields) and
//!   `repositories.action_prefs_json` (per-repo action prefs, migration
//!   0011). The user's machine only.
//! - **Default** — the global fallback baked into the resolver.
//!
//! **Per-FIELD, not per-file** (`design/03 §3.13`): a project may lock
//! `scripts` checked-in while `files_to_copy_rules` stays local-only. Every
//! field is resolved independently and carries the [`SettingsSource`] it came
//! from, so the Settings → Project UI (Desktop tasks 322+) can render the
//! lock icon + tooltip ("Locked by `.concerto/project_settings.json`" /
//! "Locked by org policy") on exactly the right control.
//!
//! ## What this module owns vs. what it does not
//!
//! - It **owns** storage + resolution of the §3.13 field set and the per-repo
//!   `action_prefs` (`design/04 §3.13`). The published
//!   `schemas/project_settings.json` autocomplete artifact is folded in here.
//! - It does **not** apply files-to-copy (Task 309), compose action prompts
//!   ([`OneShotLlm`], Task 312/321), or enforce `enterprise_data_privacy`
//!   (Task 413). Those consumers read the resolved value (with provenance)
//!   from this resolver.
//! - The live permission *decision* path stays in
//!   [`crate::security::resolve_effective_mode`] (which walks
//!   session→workarea→workspace→project→managed-cap, *below* and around the
//!   project layer). This resolver reports only the **project-default layer**
//!   value + source of `default_permission_mode` for the Settings UI. See the
//!   doc note on [`resolver::ProjectSettingsResolver::default_permission_mode`].
//!
//! ## Live reload + the escape hatch
//!
//! [`project_file::ProjectSettingsSource`] mirrors
//! [`crate::security::managed::ManagedPolicySource`]: a `notify`-rs watcher on
//! the `<project_root>/.concerto/` dir (+ each repo's `.concerto/` dir),
//! debounced at [`crate::security::managed::HOT_RELOAD_DEBOUNCE`] (500 ms),
//! re-resolving on save and publishing via `tokio::sync::watch`. A field
//! listed in the per-machine `~/.concerto/concerto.json[project_id]
//! .opt_out_of_checked_in_fields` skips the checked-in layer for that project
//! on this machine (the personal-script escape hatch).
//!
//! ## Boot audit
//!
//! [`resolver::ProjectSettingsResolver::audit_resolved_at_boot`] emits one
//! [`crate::audit::AuditKind::ProjectSettingsResolved`]
//! `{project_id, field, value_source}` per resolved field at Core start,
//! mirroring how `load_managed_policy_audited` is called once.

pub mod boot;
pub mod project_file;
pub mod resolver;

pub use boot::{build_resolver_for_project, resolve_and_audit_all_projects};

pub use project_file::{
    parse_action_prefs_toml, parse_project_settings_jsonc, strip_jsonc_line_comments,
    ActionPrefsFile, CheckedInProjectSettings, OptOutConfig, ProjectSettingsLoad,
    ProjectSettingsSource,
};
pub use resolver::{
    FilesToCopyMode, FilesToCopyRule, ProjectSettingsField, ProjectSettingsResolver, Resolved,
    SettingsSource, ACTION_KEYS,
};
