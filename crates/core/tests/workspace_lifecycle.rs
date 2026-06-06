//! Integration test for the Task 19 `Workspaces` gRPC service.
//!
//! Exercises the full path:
//! - spawn a real Core subprocess via the Task 17 harness
//! - seed a `projects` row directly (no `Projects` service in V0.1)
//! - register a repository via `Repositories.AddRepository`
//! - create a workspace via `Workspaces.CreateWorkspace`
//! - re-fetch the workspace (`GetWorkspace`)
//! - list (`ListWorkspaces`) for the project
//! - archive (`ArchiveWorkspace`) and re-fetch to verify `archived_at`
//! - verify slug-collision auto-suffix (second workspace with the same
//!   name gets `-2` slug)
//! - verify multi-repo create (Task 306): a 3-repo workspace persists
//!   three `workspace_repos` rows with positions 0/1/2 in declaration
//!   order
//! - verify empty-repo / duplicate-repo / foreign-repo creates are
//!   rejected (`workspace.no_repos` / `workspace.duplicate_repo` /
//!   `NOT_FOUND`).

#![cfg(unix)]

use std::path::Path;

use concerto_proto::v1::{
    AddRepoRequest, CreateWorkspaceRequest, ListWorkspacesRequest, WorkspaceId,
};
use concerto_test_harness::CoreUnderTest;
use tonic::Code;

/// Insert a `projects` row directly into the Core's SQLite file. The
/// `Projects` gRPC service doesn't exist in V0.1 (per Task 19 spec —
/// the project-management surface lands later).
async fn insert_project(db_path: &Path, project_id: &str) {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    let opts = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(false);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("open db write pool");
    sqlx::query("INSERT INTO projects (id, name, created_at) VALUES (?, 'test', 0)")
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("insert project");
    pool.close().await;
}

async fn register_repo(core: &CoreUnderTest, project_id: &str, name: &str) -> String {
    let mut client = core.repositories_client().await.expect("repos client");
    let repo = client
        .add_repository(AddRepoRequest {
            project_id: project_id.to_string(),
            name: name.to_string(),
            // The workspace flow doesn't trigger a clone — we just need
            // a valid `repositories` row whose project_id matches.
            url: format!("file:///tmp/{name}"),
            default_branch: "main".to_string(),
            // Task 301 added clone_strategy/with_sparse; empty → Full.
            ..Default::default()
        })
        .await
        .expect("AddRepository")
        .into_inner();
    repo.id
}

#[tokio::test(flavor = "multi_thread")]
async fn create_get_list_archive_workspace() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let project_id = "ws-test-project".to_string();
    insert_project(&core.db_path, &project_id).await;
    let repo_id = register_repo(&core, &project_id, "fixture-repo").await;

    let mut wsc = core.workspaces_client().await.expect("workspaces client");

    // Create.
    let ws = wsc
        .create_workspace(CreateWorkspaceRequest {
            project_id: project_id.clone(),
            name: "Login Bug Fix".to_string(),
            repository_ids: vec![repo_id.clone()],
            permission_mode: None,
            description: Some("smoke".to_string()),
        })
        .await
        .expect("CreateWorkspace")
        .into_inner();
    assert!(!ws.id.is_empty(), "workspace id should be assigned");
    assert_eq!(ws.project_id, project_id);
    assert_eq!(ws.name, "Login Bug Fix");
    assert_eq!(ws.slug, "login-bug-fix");
    assert_eq!(ws.description.as_deref(), Some("smoke"));
    assert!(ws.created_at.is_some(), "created_at should be set");
    assert!(ws.archived_at.is_none(), "fresh workspace is not archived");

    // Get.
    let got = wsc
        .get_workspace(WorkspaceId {
            value: ws.id.clone(),
        })
        .await
        .expect("GetWorkspace")
        .into_inner();
    assert_eq!(got.id, ws.id);
    assert_eq!(got.slug, "login-bug-fix");

    // List.
    let listed = wsc
        .list_workspaces(ListWorkspacesRequest {
            project_id: project_id.clone(),
        })
        .await
        .expect("ListWorkspaces")
        .into_inner();
    assert_eq!(listed.workspaces.len(), 1);
    assert_eq!(listed.workspaces[0].id, ws.id);

    // Workspace_repos row exists on disk.
    let pool = core.db().await.expect("db");
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM workspace_repos WHERE workspace_id = ?")
            .bind(&ws.id)
            .fetch_one(&pool)
            .await
            .expect("count workspace_repos");
    assert_eq!(count, 1, "exactly one workspace_repos row should exist");
    let (repo_row,): (String,) =
        sqlx::query_as("SELECT repository_id FROM workspace_repos WHERE workspace_id = ?")
            .bind(&ws.id)
            .fetch_one(&pool)
            .await
            .expect("repo row");
    assert_eq!(repo_row, repo_id);

    // Archive.
    wsc.archive_workspace(WorkspaceId {
        value: ws.id.clone(),
    })
    .await
    .expect("ArchiveWorkspace");

    // Get-after-archive: `archived_at` populated.
    let after = wsc
        .get_workspace(WorkspaceId {
            value: ws.id.clone(),
        })
        .await
        .expect("GetWorkspace after archive")
        .into_inner();
    assert!(
        after.archived_at.is_some(),
        "archived_at should be populated after ArchiveWorkspace"
    );

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn slug_collision_auto_suffix() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let project_id = "slug-project".to_string();
    insert_project(&core.db_path, &project_id).await;
    let repo_id = register_repo(&core, &project_id, "slug-repo").await;

    let mut wsc = core.workspaces_client().await.expect("workspaces client");

    let first = wsc
        .create_workspace(CreateWorkspaceRequest {
            project_id: project_id.clone(),
            name: "Same Name".to_string(),
            repository_ids: vec![repo_id.clone()],
            permission_mode: None,
            description: None,
        })
        .await
        .expect("first create")
        .into_inner();
    assert_eq!(first.slug, "same-name");

    let second = wsc
        .create_workspace(CreateWorkspaceRequest {
            project_id: project_id.clone(),
            name: "Same Name".to_string(),
            repository_ids: vec![repo_id.clone()],
            permission_mode: None,
            description: None,
        })
        .await
        .expect("second create")
        .into_inner();
    assert_eq!(
        second.slug, "same-name-2",
        "second slug should auto-suffix `-2`"
    );

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn multi_repo_create_persists_positions() {
    // Task 306: a 3-repo workspace persists three `workspace_repos` rows
    // with `position` 0/1/2 in the caller's declaration order.
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let project_id = "multi-repo-project".to_string();
    insert_project(&core.db_path, &project_id).await;
    // Register out of alphabetical order so a `position`-driven read is
    // distinguishable from the old `ORDER BY repository_id`.
    let repo_api = register_repo(&core, &project_id, "api").await;
    let repo_android = register_repo(&core, &project_id, "android").await;
    let repo_ios = register_repo(&core, &project_id, "ios").await;
    // Declaration order: api, android, ios (NOT id-sorted).
    let declared = vec![repo_api.clone(), repo_android.clone(), repo_ios.clone()];

    let mut wsc = core.workspaces_client().await.expect("workspaces client");
    let ws = wsc
        .create_workspace(CreateWorkspaceRequest {
            project_id: project_id.clone(),
            name: "Cross Platform".to_string(),
            repository_ids: declared.clone(),
            permission_mode: None,
            description: None,
        })
        .await
        .expect("multi-repo create should succeed")
        .into_inner();
    assert_eq!(ws.slug, "cross-platform");

    let pool = core.db().await.expect("db");
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT repository_id, position FROM workspace_repos \
         WHERE workspace_id = ? ORDER BY position",
    )
    .bind(&ws.id)
    .fetch_all(&pool)
    .await
    .expect("workspace_repos rows");
    assert_eq!(rows.len(), 3, "three workspace_repos rows");
    assert_eq!(rows[0], (repo_api, 0));
    assert_eq!(rows[1], (repo_android, 1));
    assert_eq!(rows[2], (repo_ios, 2));

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_repo_set_rejected() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let project_id = "empty-repo-project".to_string();
    insert_project(&core.db_path, &project_id).await;

    let mut wsc = core.workspaces_client().await.expect("workspaces client");
    let err = wsc
        .create_workspace(CreateWorkspaceRequest {
            project_id: project_id.clone(),
            name: "No Repos".to_string(),
            repository_ids: vec![],
            permission_mode: None,
            description: None,
        })
        .await
        .expect_err("empty-repo create must fail");
    assert_eq!(err.code(), Code::InvalidArgument);
    assert!(
        err.message().contains("workspace.no_repos"),
        "expected workspace.no_repos wire code; got {:?}",
        err.message()
    );

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicate_repo_rejected() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let project_id = "dup-repo-project".to_string();
    insert_project(&core.db_path, &project_id).await;
    let repo = register_repo(&core, &project_id, "dup-repo").await;

    let mut wsc = core.workspaces_client().await.expect("workspaces client");
    let err = wsc
        .create_workspace(CreateWorkspaceRequest {
            project_id: project_id.clone(),
            name: "Dup".to_string(),
            repository_ids: vec![repo.clone(), repo.clone()],
            permission_mode: None,
            description: None,
        })
        .await
        .expect_err("duplicate-repo create must fail");
    assert_eq!(err.code(), Code::InvalidArgument);
    assert!(
        err.message().contains("workspace.duplicate_repo"),
        "expected workspace.duplicate_repo wire code; got {:?}",
        err.message()
    );

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn foreign_repo_rejected() {
    // A repo id that does not belong to the project is NOT_FOUND.
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let project_id = "foreign-repo-project".to_string();
    let other_project = "other-project".to_string();
    insert_project(&core.db_path, &project_id).await;
    insert_project(&core.db_path, &other_project).await;
    let foreign = register_repo(&core, &other_project, "foreign-repo").await;

    let mut wsc = core.workspaces_client().await.expect("workspaces client");
    let err = wsc
        .create_workspace(CreateWorkspaceRequest {
            project_id: project_id.clone(),
            name: "Foreign".to_string(),
            repository_ids: vec![foreign],
            permission_mode: None,
            description: None,
        })
        .await
        .expect_err("foreign-repo create must fail");
    assert_eq!(
        err.code(),
        Code::NotFound,
        "a repo from another project should be NOT_FOUND; got {:?}",
        err.code()
    );

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_project_returns_not_found() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");

    let mut wsc = core.workspaces_client().await.expect("workspaces client");
    let err = wsc
        .create_workspace(CreateWorkspaceRequest {
            project_id: "does-not-exist".to_string(),
            name: "Anywhere".to_string(),
            repository_ids: vec!["some-repo-id".to_string()],
            permission_mode: None,
            description: None,
        })
        .await
        .expect_err("unknown project must fail");
    assert_eq!(err.code(), Code::NotFound, "expected NotFound");

    core.shutdown().await.expect("shutdown");
}
