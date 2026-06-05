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
//! - verify V0.1 multi-repo rejection returns
//!   `INVALID_ARGUMENT` + wire code `workspace.v0_single_repo_only`.

#![cfg(unix)]

use std::path::Path;

use concerto_proto::v1::{
    AddRepoRequest, ConcertoError, CreateWorkspaceRequest, ListWorkspacesRequest, WorkspaceId,
};
use concerto_test_harness::CoreUnderTest;
use prost::Message;
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
async fn multi_repo_rejected_with_typed_wire_code() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let project_id = "multi-repo-project".to_string();
    insert_project(&core.db_path, &project_id).await;
    let repo_a = register_repo(&core, &project_id, "repo-a").await;
    let repo_b = register_repo(&core, &project_id, "repo-b").await;

    let mut wsc = core.workspaces_client().await.expect("workspaces client");
    let err = wsc
        .create_workspace(CreateWorkspaceRequest {
            project_id: project_id.clone(),
            name: "Two Repos".to_string(),
            repository_ids: vec![repo_a, repo_b],
            permission_mode: None,
            description: None,
        })
        .await
        .expect_err("multi-repo create must fail");
    assert_eq!(
        err.code(),
        Code::InvalidArgument,
        "expected INVALID_ARGUMENT, got {:?}",
        err.code()
    );
    // `ConcertoError.code` carries the generic `"validation"` wire
    // code; the specific subcode `workspace.v0_single_repo_only` is
    // prefixed onto the message body per the locked surface.
    let details = ConcertoError::decode(err.details()).expect("decode ConcertoError details");
    assert_eq!(details.code, "validation");
    assert!(
        details.message.contains("workspace.v0_single_repo_only"),
        "ConcertoError.message should embed the wire code subcode; got {:?}",
        details.message
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
