//! Boot-time settings resolution + audit (Task 310, `design/03 §3.13`).
//!
//! At Core start — after the audit writer is live and persistence is up — the
//! Core builds a [`WorkspaceSettingsResolver`] per workspace and emits one
//! [`crate::audit::AuditKind::WorkspaceSettingsResolved`]
//! `{workspace_id, field, value_source}` per resolved field. This mirrors how
//! `load_managed_policy_audited` is called exactly once at boot, and provides
//! the provenance trail `design/03 §3.13` calls for ("why does this work on my
//! machine but not yours").
//!
//! ## Where the checked-in files live at boot
//!
//! A workspace has no filesystem root column; the checked-in
//! `workspace_settings.json` conceptually lives at the workspace's *reference
//! repo* root (`design/03 §3.10`: the first-listed repo's worktree). At boot
//! we use the workspace's repositories ordered as `workspaces::list_repos`
//! returns them (by `workspace_repos.position` — deterministic) and read
//! `<first_repo.local_path>/.concerto/workspace_settings.json` as the
//! workspace's checked-in layer, plus
//! `<repo.local_path>/.concerto/action_prefs.toml` per repo. Every read is
//! best-effort: a missing repo/dir/file simply yields an empty layer
//! (resolution falls through to local DB / default). The boot step never gates
//! startup.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use concerto_error::Result;
use concerto_persist::{Persistence, WorkspaceId};

use crate::audit::AuditWriter;
use crate::security::managed::ManagedPolicy;
use crate::settings::resolver::WorkspaceSettingsResolver;
use crate::settings::workspace_file::{
    load_action_prefs_file, load_workspace_settings_file, OptOutConfig,
};

/// The `.concerto/` subdir name a repo worktree carries the checked-in
/// settings files in.
const CONCERTO_DIR_NAME: &str = ".concerto";

/// Build a [`WorkspaceSettingsResolver`] for one workspace from all four
/// layers.
///
/// - **Managed:** the supplied [`ManagedPolicy`] (loaded once by the caller).
/// - **Checked-in:** `<reference_repo>/.concerto/workspace_settings.json` +
///   per-repo `.concerto/action_prefs.toml`.
/// - **Local DB:** `workspaces.settings_json` + each repo's
///   `repositories.action_prefs_json`.
/// - **Opt-out:** the per-machine `~/.concerto/concerto.json` field list for
///   this workspace.
///
/// Best-effort throughout: a DB read error propagates (the caller decides
/// whether to skip the workspace), but missing files/dirs are empty layers.
pub async fn build_resolver_for_workspace(
    persistence: &Persistence,
    workspace_id: &WorkspaceId,
    managed: ManagedPolicy,
    opt_out: &OptOutConfig,
) -> Result<WorkspaceSettingsResolver> {
    let pool = persistence.readers();

    let local_db_settings_json =
        concerto_persist::workspaces::get_settings_json(pool, workspace_id)
            .await?
            .unwrap_or_else(|| "{}".to_string());

    // The workspace's repos, in `workspace_repos.position` order.
    let repo_ids = concerto_persist::workspaces::list_repos(pool, workspace_id).await?;
    let mut repos = Vec::with_capacity(repo_ids.len());
    for repo_id in &repo_ids {
        if let Some(repo) = concerto_persist::repositories::get(pool, repo_id).await? {
            repos.push(repo);
        }
    }

    // Checked-in workspace file: the first-listed repo's `.concerto/` is the
    // reference root (`design/03 §3.10` reference-repo rule). No repos → empty.
    let checked_in = match repos.first() {
        Some(repo) => {
            let dir = PathBuf::from(&repo.local_path).join(CONCERTO_DIR_NAME);
            load_workspace_settings_file(&dir).settings
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
        .per_workspace
        .get(workspace_id.as_str())
        .cloned()
        .unwrap_or_default();

    Ok(WorkspaceSettingsResolver::new(
        workspace_id.to_string(),
        managed,
        checked_in,
        &local_db_settings_json,
        repo_checked_in,
        repo_local_db,
        opted_out,
    ))
}

/// Resolve + audit every workspace's settings at Core boot. Loads the managed
/// policy + the per-machine opt-out config once, then builds a resolver per
/// workspace and calls
/// [`WorkspaceSettingsResolver::audit_resolved_at_boot`].
///
/// Returns the total number of `WorkspaceSettingsResolved` events emitted
/// across all workspaces (for the boot log + tests). A per-workspace build
/// error is logged + skipped — one broken workspace must not block the rest of
/// boot.
pub async fn resolve_and_audit_all_workspaces(
    persistence: &Persistence,
    config_dir: &Path,
    user_home_concerto_dir: &Path,
    audit: &AuditWriter,
) -> Result<usize> {
    let managed = crate::security::managed::load_managed_policy(config_dir).unwrap_or_default();
    let opt_out = OptOutConfig::load(user_home_concerto_dir);

    let workspaces = concerto_persist::workspaces::list_all(persistence.readers()).await?;
    let mut total = 0usize;
    for workspace in workspaces {
        match build_resolver_for_workspace(persistence, &workspace.id, managed.clone(), &opt_out)
            .await
        {
            Ok(resolver) => {
                total += resolver.audit_resolved_at_boot(audit);
            }
            Err(e) => {
                tracing::warn!(
                    workspace_id = %workspace.id,
                    error = %e,
                    "workspace-settings boot resolution failed; skipping this workspace"
                );
            }
        }
    }
    tracing::info!(
        events = total,
        "workspace settings resolved + audited at boot"
    );
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditEvent, AuditKind};
    use std::sync::{Arc, Mutex};

    // A tiny in-memory audit subscriber so the boot-audit test can count the
    // WorkspaceSettingsResolved events without touching disk.
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

    async fn seed_workspace_with_repo(
        persist: &Persistence,
        workspace_id: &str,
        repo_root: &Path,
        action_prefs_json: &str,
    ) {
        use concerto_persist::{NewRepository, NewWorkspace, RepositoryId, WorkspaceId};
        use sqlx::Connection;
        let mut w = persist.writer().await;
        let mut tx = w.begin().await.unwrap();
        sqlx::query("PRAGMA defer_foreign_keys = ON")
            .execute(&mut *tx)
            .await
            .unwrap();
        concerto_persist::workspaces::insert(
            &mut tx,
            NewWorkspace {
                id: WorkspaceId(workspace_id.to_string()),
                name: format!("ws-{workspace_id}"),
                slug: workspace_id.to_string(),
                icon: None,
                description: None,
                permission_mode: None,
                created_at: 0,
            },
        )
        .await
        .unwrap();
        let repo_id = RepositoryId(format!("repo-{workspace_id}"));
        concerto_persist::repositories::insert(
            &mut tx,
            NewRepository {
                id: repo_id.clone(),
                name: format!("r-{workspace_id}"),
                url: format!("https://example/r-{workspace_id}"),
                local_path: repo_root.to_string_lossy().into_owned(),
                clone_strategy: "full".to_string(),
                default_branch: "main".to_string(),
            },
        )
        .await
        .unwrap();
        concerto_persist::workspaces::update_repos(
            &mut tx,
            &WorkspaceId(workspace_id.to_string()),
            &[concerto_persist::WorkspaceRepoCones::empty_cones(
                repo_id.clone(),
            )],
        )
        .await
        .unwrap();
        if action_prefs_json != "{}" {
            sqlx::query("UPDATE repositories SET action_prefs_json = ? WHERE id = ?")
                .bind(action_prefs_json)
                .bind(&repo_id.0)
                .execute(&mut *tx)
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();
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

        // A repo worktree with a checked-in workspace_settings.json + action_prefs.
        let repo_root = tmp.path().join("repo");
        let concerto = repo_root.join(".concerto");
        std::fs::create_dir_all(&concerto).unwrap();
        std::fs::write(
            concerto.join("workspace_settings.json"),
            r#"{ "run_script_mode": "sequential" }"#,
        )
        .unwrap();
        std::fs::write(
            concerto.join("action_prefs.toml"),
            "code_review = \"quote contributing\"\n",
        )
        .unwrap();

        seed_workspace_with_repo(&persist, "w1", &repo_root, r#"{"pr_create": "db pref"}"#).await;

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

        let total =
            resolve_and_audit_all_workspaces(&persist, &config_dir, &home_concerto, &writer)
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
            .filter(|e| e.kind == AuditKind::WorkspaceSettingsResolved)
            .collect();
        assert_eq!(resolved.len(), total, "one event per resolved field");
        assert!(total >= 7, "at least the workspace field set");

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
