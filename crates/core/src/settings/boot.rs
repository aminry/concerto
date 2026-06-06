//! Boot-time settings resolution + audit (Task 310, `design/03 §3.13`).
//!
//! At Core start — after the audit writer is live and persistence is up — the
//! Core builds a [`ProjectSettingsResolver`] per project and emits one
//! [`crate::audit::AuditKind::ProjectSettingsResolved`]
//! `{project_id, field, value_source}` per resolved field. This mirrors how
//! `load_managed_policy_audited` is called exactly once at boot, and provides
//! the provenance trail `design/03 §3.13` calls for ("why does this work on my
//! machine but not yours").
//!
//! ## Where the checked-in files live at boot
//!
//! A project has no filesystem root column in the V0.1 schema; the checked-in
//! `project_settings.json` conceptually lives at the project's *reference
//! repo* root (`design/03 §3.10`: the first-listed repo's worktree). At boot
//! we use the project's repositories ordered as `repositories::list_by_project`
//! returns them (by name — deterministic) and read
//! `<first_repo.local_path>/.concerto/project_settings.json` as the project's
//! checked-in layer, plus `<repo.local_path>/.concerto/action_prefs.toml` per
//! repo. Every read is best-effort: a missing repo/dir/file simply yields an
//! empty layer (resolution falls through to local DB / default). The boot step
//! never gates startup.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use concerto_error::Result;
use concerto_persist::{Persistence, ProjectId};

use crate::audit::AuditWriter;
use crate::security::managed::ManagedPolicy;
use crate::settings::project_file::{
    load_action_prefs_file, load_project_settings_file, OptOutConfig,
};
use crate::settings::resolver::ProjectSettingsResolver;

/// The `.concerto/` subdir name a repo worktree carries the checked-in
/// settings files in.
const CONCERTO_DIR_NAME: &str = ".concerto";

/// Build a [`ProjectSettingsResolver`] for one project from all four layers.
///
/// - **Managed:** the supplied [`ManagedPolicy`] (loaded once by the caller).
/// - **Checked-in:** `<reference_repo>/.concerto/project_settings.json` +
///   per-repo `.concerto/action_prefs.toml`.
/// - **Local DB:** `projects.settings_json` + each repo's
///   `repositories.action_prefs_json`.
/// - **Opt-out:** the per-machine `~/.concerto/concerto.json` field list for
///   this project.
///
/// Best-effort throughout: a DB read error propagates (the caller decides
/// whether to skip the project), but missing files/dirs are empty layers.
pub async fn build_resolver_for_project(
    persistence: &Persistence,
    project_id: &ProjectId,
    managed: ManagedPolicy,
    opt_out: &OptOutConfig,
) -> Result<ProjectSettingsResolver> {
    let pool = persistence.readers();

    let local_db_settings_json = concerto_persist::projects::get_settings_json(pool, project_id)
        .await?
        .unwrap_or_else(|| "{}".to_string());

    let repos = concerto_persist::repositories::list_by_project(pool, project_id.as_str()).await?;

    // Checked-in project file: the first-listed repo's `.concerto/` is the
    // project root (`design/03 §3.10` reference-repo rule). No repos → empty.
    let checked_in = match repos.first() {
        Some(repo) => {
            let dir = PathBuf::from(&repo.local_path).join(CONCERTO_DIR_NAME);
            load_project_settings_file(&dir).settings
        }
        None => Default::default(),
    };

    // Per-repo action-prefs layers.
    let mut repo_checked_in = BTreeMap::new();
    let mut repo_local_db = BTreeMap::new();
    for repo in &repos {
        let dir = PathBuf::from(&repo.local_path).join(CONCERTO_DIR_NAME);
        let load = load_action_prefs_file(&dir);
        if !load.prefs.prefs.is_empty() {
            repo_checked_in.insert(repo.id.to_string(), load.prefs);
        }
        repo_local_db.insert(repo.id.to_string(), repo.action_prefs_json.clone());
    }

    let opted_out = opt_out
        .per_project
        .get(project_id.as_str())
        .cloned()
        .unwrap_or_default();

    Ok(ProjectSettingsResolver::new(
        project_id.to_string(),
        managed,
        checked_in,
        &local_db_settings_json,
        repo_checked_in,
        repo_local_db,
        opted_out,
    ))
}

/// Resolve + audit every project's settings at Core boot. Loads the managed
/// policy + the per-machine opt-out config once, then builds a resolver per
/// project and calls
/// [`ProjectSettingsResolver::audit_resolved_at_boot`].
///
/// Returns the total number of `ProjectSettingsResolved` events emitted across
/// all projects (for the boot log + tests). A per-project build error is
/// logged + skipped — one broken project must not block the rest of boot.
pub async fn resolve_and_audit_all_projects(
    persistence: &Persistence,
    config_dir: &Path,
    user_home_concerto_dir: &Path,
    audit: &AuditWriter,
) -> Result<usize> {
    let managed = crate::security::managed::load_managed_policy(config_dir).unwrap_or_default();
    let opt_out = OptOutConfig::load(user_home_concerto_dir);

    let projects = concerto_persist::projects::list_all(persistence.readers()).await?;
    let mut total = 0usize;
    for project in projects {
        match build_resolver_for_project(persistence, &project.id, managed.clone(), &opt_out).await
        {
            Ok(resolver) => {
                total += resolver.audit_resolved_at_boot(audit);
            }
            Err(e) => {
                tracing::warn!(
                    project_id = %project.id,
                    error = %e,
                    "project-settings boot resolution failed; skipping this project"
                );
            }
        }
    }
    tracing::info!(
        events = total,
        "project settings resolved + audited at boot"
    );
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditEvent, AuditKind};
    use std::sync::{Arc, Mutex};

    // A tiny in-memory audit subscriber so the boot-audit test can count the
    // ProjectSettingsResolved events without touching disk.
    struct CapturingSubscriber {
        events: Arc<Mutex<Vec<AuditEvent>>>,
    }

    #[async_trait::async_trait]
    impl crate::audit::AuditLogSubscriber for CapturingSubscriber {
        fn id(&self) -> &str {
            "capturing-test"
        }
        async fn on_event(&self, event: &AuditEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
        async fn flush(&self) {}
    }

    async fn seed_project_with_repo(
        persist: &Persistence,
        project_id: &str,
        repo_root: &Path,
        action_prefs_json: &str,
    ) {
        use concerto_persist::{NewProject, NewRepository, ProjectId, RepositoryId};
        let mut w = persist.writer().await;
        concerto_persist::projects::insert(
            &mut w,
            NewProject {
                id: ProjectId(project_id.to_string()),
                name: format!("proj-{project_id}"),
                icon: None,
                created_at: 0,
            },
        )
        .await
        .unwrap();
        let repo_id = RepositoryId(format!("repo-{project_id}"));
        concerto_persist::repositories::insert(
            &mut w,
            NewRepository {
                id: repo_id.clone(),
                project_id: project_id.to_string(),
                name: "r".to_string(),
                url: "https://example/r".to_string(),
                local_path: repo_root.to_string_lossy().into_owned(),
                clone_strategy: "full".to_string(),
                default_branch: "main".to_string(),
            },
        )
        .await
        .unwrap();
        if action_prefs_json != "{}" {
            sqlx::query("UPDATE repositories SET action_prefs_json = ? WHERE id = ?")
                .bind(action_prefs_json)
                .bind(&repo_id.0)
                .execute(&mut *w)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn boot_emits_one_event_per_field_with_source() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("db.sqlite");
        let persist = Persistence::open(concerto_persist::PersistenceConfig {
            db_path: db_path.clone(),
            max_readers: 2,
        })
        .await
        .unwrap();

        // A repo worktree with a checked-in project_settings.json + action_prefs.
        let repo_root = tmp.path().join("repo");
        let concerto = repo_root.join(".concerto");
        std::fs::create_dir_all(&concerto).unwrap();
        std::fs::write(
            concerto.join("project_settings.json"),
            r#"{ "run_script_mode": "sequential" }"#,
        )
        .unwrap();
        std::fs::write(
            concerto.join("action_prefs.toml"),
            "code_review = \"quote contributing\"\n",
        )
        .unwrap();

        seed_project_with_repo(&persist, "p1", &repo_root, r#"{"pr_create": "db pref"}"#).await;

        // Capture audit events.
        let captured = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Arc::new(CapturingSubscriber {
            events: Arc::clone(&captured),
        });
        let token = tokio_util::sync::CancellationToken::new();
        let (writer, _drained, join) =
            crate::audit::AuditWriterTask::spawn(vec![subscriber], token.clone());

        let config_dir = tmp.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        let home_concerto = tmp.path().join("home_concerto");
        std::fs::create_dir_all(&home_concerto).unwrap();

        let total = resolve_and_audit_all_projects(&persist, &config_dir, &home_concerto, &writer)
            .await
            .unwrap();

        // Flush the writer so every appended event reaches the subscriber:
        // drop our writer handle so the channel closes, cancel the token, then
        // await the drain task's join handle (it processes queued events on
        // shutdown before exiting).
        drop(writer);
        token.cancel();
        join.await.unwrap();

        let events = captured.lock().unwrap();
        let resolved: Vec<_> = events
            .iter()
            .filter(|e| e.kind == AuditKind::ProjectSettingsResolved)
            .collect();
        assert_eq!(resolved.len(), total, "one event per resolved field");
        assert!(total >= 7, "at least the project field set");

        // The checked-in run_script_mode shows the right source; the DB-only
        // action pref shows local_db.
        let run_mode = resolved
            .iter()
            .find(|e| e.details_json["field"] == "run_script_mode")
            .expect("run_script_mode event present");
        assert_eq!(run_mode.details_json["value_source"], "checked_in");

        let pr_create = resolved
            .iter()
            .find(|e| e.details_json["field"] == "action_prefs.pr_create")
            .expect("pr_create event present");
        assert_eq!(pr_create.details_json["value_source"], "local_db");

        let code_review = resolved
            .iter()
            .find(|e| e.details_json["field"] == "action_prefs.code_review")
            .expect("code_review event present");
        assert_eq!(code_review.details_json["value_source"], "checked_in");
    }
}
