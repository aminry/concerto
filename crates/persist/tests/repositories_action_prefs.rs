//! Round-trip test for the Task 310 `repositories.action_prefs_json` column
//! (migration 0011, `design/04 §3.13`) — the local-DB layer of the settings
//! precedence chain.
//!
//! Migration 0011 adds `action_prefs_json TEXT NOT NULL DEFAULT '{}'` via a
//! plain `ADD COLUMN` (no CHECK, no table recreate). This proves:
//! - a freshly-inserted repo reads back the SQL default `"{}"` on every SELECT
//!   path (`get` / `list_by_project` / `list_all`),
//! - an explicit `action_prefs_json` value round-trips through the column.

use concerto_persist::{
    repositories, NewProject, NewRepository, Persistence, PersistenceConfig, ProjectId,
    RepositoryId,
};

async fn fresh_db() -> (tempfile::TempDir, Persistence) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let persist = Persistence::open(PersistenceConfig {
        db_path,
        max_readers: 2,
    })
    .await
    .expect("open");
    (dir, persist)
}

async fn seed_repo(persist: &Persistence, repo_id: &str) {
    let project_id = ProjectId("proj-1".to_string());
    let mut w = persist.writer().await;
    concerto_persist::projects::insert(
        &mut w,
        NewProject {
            id: project_id.clone(),
            name: "Test".to_string(),
            icon: None,
            created_at: 1_700_000_000_000,
        },
    )
    .await
    .expect("insert project");
    repositories::insert(
        &mut w,
        NewRepository {
            id: RepositoryId(repo_id.to_string()),
            project_id: project_id.0.clone(),
            name: repo_id.to_string(),
            url: format!("file:///tmp/{repo_id}.git"),
            local_path: format!("/tmp/repos/{repo_id}"),
            clone_strategy: "full".to_string(),
            default_branch: "main".to_string(),
        },
    )
    .await
    .expect("insert repo");
}

#[tokio::test]
async fn action_prefs_json_defaults_to_empty_object_on_every_select() {
    let (_dir, persist) = fresh_db().await;
    seed_repo(&persist, "r1").await;

    let id = RepositoryId("r1".to_string());
    let got = repositories::get(persist.readers(), &id)
        .await
        .expect("get")
        .expect("present");
    assert_eq!(got.action_prefs_json, "{}");

    let by_project = repositories::list_by_project(persist.readers(), "proj-1")
        .await
        .expect("list_by_project");
    assert_eq!(by_project.len(), 1);
    assert_eq!(by_project[0].action_prefs_json, "{}");

    let all = repositories::list_all(persist.readers())
        .await
        .expect("list_all");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].action_prefs_json, "{}");
}

#[tokio::test]
async fn action_prefs_json_round_trips_explicit_value() {
    let (_dir, persist) = fresh_db().await;
    seed_repo(&persist, "r1").await;

    let payload = r#"{"pr_create":"Use the PR template.","branch_rename":"kebab-case."}"#;
    {
        let mut w = persist.writer().await;
        sqlx::query("UPDATE repositories SET action_prefs_json = ? WHERE id = ?")
            .bind(payload)
            .bind("r1")
            .execute(&mut *w)
            .await
            .expect("update action_prefs_json");
    }

    let id = RepositoryId("r1".to_string());
    let got = repositories::get(persist.readers(), &id)
        .await
        .expect("get")
        .expect("present");
    assert_eq!(got.action_prefs_json, payload);
    // The other layers (cone_defaults_json) are untouched by the new column.
    assert_eq!(got.cone_defaults_json, "[]");
}
