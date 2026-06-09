//! Integration test for the Task 19 `Workspaces` gRPC service.
//!
//! Exercises the full path:
//! - spawn a real Core subprocess via the Task 17 harness
//! - register a repository via `Repositories.AddRepository` (global registry)
//! - create a workspace via `Workspaces.CreateWorkspace`
//! - re-fetch the workspace (`GetWorkspace`)
//! - list (`ListWorkspaces`)
//! - archive (`ArchiveWorkspace`) and re-fetch to verify `archived_at`
//! - verify slug-collision auto-suffix (second workspace with the same
//!   name gets `-2` slug)
//! - verify multi-repo create (Task 306): a 3-repo workspace persists
//!   three `workspace_repos` rows with positions 0/1/2 in declaration
//!   order
//! - verify empty-repo / duplicate-repo / unknown-repo creates are
//!   rejected (`workspace.no_repos` / `workspace.duplicate_repo` /
//!   `NOT_FOUND`).

#![cfg(unix)]

use concerto_proto::v1::{
    AddRepoRequest, CreateWorkspaceRequest, ListWorkspaceReposResponse, ListWorkspacesRequest,
    UpdateWorkspaceRequest, WorkspaceId, WorkspaceRepoSpec,
};
use concerto_test_harness::CoreUnderTest;
use tonic::Code;

/// One repo attachment spec with an empty cone (seeded from defaults).
fn spec(repo_id: &str) -> WorkspaceRepoSpec {
    WorkspaceRepoSpec {
        repository_id: repo_id.to_string(),
        sparse_cones: vec![],
    }
}

async fn register_repo(core: &CoreUnderTest, name: &str) -> String {
    let mut client = core.repositories_client().await.expect("repos client");
    let repo = client
        .add_repository(AddRepoRequest {
            name: name.to_string(),
            // The workspace flow doesn't trigger a clone — we just need a
            // valid `repositories` row in the global registry.
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
    let repo_id = register_repo(&core, "fixture-repo").await;

    let mut wsc = core.workspaces_client().await.expect("workspaces client");

    // Create.
    let ws = wsc
        .create_workspace(CreateWorkspaceRequest {
            name: "Login Bug Fix".to_string(),
            repos: vec![spec(&repo_id)],
            permission_mode: None,
            description: Some("smoke".to_string()),
            icon: None,
        })
        .await
        .expect("CreateWorkspace")
        .into_inner();
    assert!(!ws.id.is_empty(), "workspace id should be assigned");
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
            include_archived: false,
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

    // List default (include_archived = false) now hides it; include_archived
    // = true surfaces it again.
    let hidden = wsc
        .list_workspaces(ListWorkspacesRequest {
            include_archived: false,
        })
        .await
        .expect("ListWorkspaces hide archived")
        .into_inner();
    assert!(
        hidden.workspaces.iter().all(|w| w.id != ws.id),
        "archived workspace hidden by default"
    );
    let shown = wsc
        .list_workspaces(ListWorkspacesRequest {
            include_archived: true,
        })
        .await
        .expect("ListWorkspaces include archived")
        .into_inner();
    assert!(
        shown.workspaces.iter().any(|w| w.id == ws.id),
        "archived workspace shown with include_archived"
    );

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn slug_collision_auto_suffix() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let repo_id = register_repo(&core, "slug-repo").await;

    let mut wsc = core.workspaces_client().await.expect("workspaces client");

    let first = wsc
        .create_workspace(CreateWorkspaceRequest {
            name: "Same Name".to_string(),
            repos: vec![spec(&repo_id)],
            permission_mode: None,
            description: None,
            icon: None,
        })
        .await
        .expect("first create")
        .into_inner();
    assert_eq!(first.slug, "same-name");

    let second = wsc
        .create_workspace(CreateWorkspaceRequest {
            name: "Same Name".to_string(),
            repos: vec![spec(&repo_id)],
            permission_mode: None,
            description: None,
            icon: None,
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
    // Register out of alphabetical order so a `position`-driven read is
    // distinguishable from the old `ORDER BY repository_id`.
    let repo_api = register_repo(&core, "api").await;
    let repo_android = register_repo(&core, "android").await;
    let repo_ios = register_repo(&core, "ios").await;
    // Declaration order: api, android, ios (NOT id-sorted).
    let declared = [repo_api.clone(), repo_android.clone(), repo_ios.clone()];

    let mut wsc = core.workspaces_client().await.expect("workspaces client");
    let ws = wsc
        .create_workspace(CreateWorkspaceRequest {
            name: "Cross Platform".to_string(),
            repos: declared.iter().map(|r| spec(r)).collect(),
            permission_mode: None,
            description: None,
            icon: None,
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

    let mut wsc = core.workspaces_client().await.expect("workspaces client");
    let err = wsc
        .create_workspace(CreateWorkspaceRequest {
            name: "No Repos".to_string(),
            repos: vec![],
            permission_mode: None,
            description: None,
            icon: None,
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
    let repo = register_repo(&core, "dup-repo").await;

    let mut wsc = core.workspaces_client().await.expect("workspaces client");
    let err = wsc
        .create_workspace(CreateWorkspaceRequest {
            name: "Dup".to_string(),
            repos: vec![spec(&repo), spec(&repo)],
            permission_mode: None,
            description: None,
            icon: None,
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
async fn unknown_repo_returns_not_found() {
    // A repo id that does not exist in the global registry is NOT_FOUND.
    let core = CoreUnderTest::spawn().await.expect("spawn core");

    let mut wsc = core.workspaces_client().await.expect("workspaces client");
    let err = wsc
        .create_workspace(CreateWorkspaceRequest {
            name: "Unknown".to_string(),
            repos: vec![spec("does-not-exist")],
            permission_mode: None,
            description: None,
            icon: None,
        })
        .await
        .expect_err("unknown-repo create must fail");
    assert_eq!(
        err.code(),
        Code::NotFound,
        "an unknown repo id should be NOT_FOUND; got {:?}",
        err.code()
    );

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn update_workspace_edits_metadata_and_repos() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let repo_a = register_repo(&core, "edit-a").await;
    let repo_b = register_repo(&core, "edit-b").await;
    let mut wsc = core.workspaces_client().await.expect("workspaces client");

    let ws = wsc
        .create_workspace(CreateWorkspaceRequest {
            name: "Before".to_string(),
            repos: vec![spec(&repo_a)],
            permission_mode: None,
            description: None,
            icon: None,
        })
        .await
        .expect("create")
        .into_inner();
    let original_slug = ws.slug.clone();

    // Edit name + icon, add repo_b.
    let updated = wsc
        .update_workspace(UpdateWorkspaceRequest {
            workspace_id: ws.id.clone(),
            name: Some("After".to_string()),
            icon: Some("🚀".to_string()),
            description: None,
            repos: vec![spec(&repo_a), spec(&repo_b)],
        })
        .await
        .expect("update")
        .into_inner();
    assert_eq!(updated.name, "After");
    assert_eq!(updated.icon.as_deref(), Some("🚀"));
    assert_eq!(updated.slug, original_slug, "slug stays fixed on rename");

    // ListWorkspaceRepos returns both, in declaration order.
    let listed: ListWorkspaceReposResponse = wsc
        .list_workspace_repos(WorkspaceId { value: ws.id.clone() })
        .await
        .expect("list repos")
        .into_inner();
    assert_eq!(listed.repos.len(), 2);
    assert_eq!(listed.repos[0].repository_id, repo_a);
    assert_eq!(listed.repos[1].repository_id, repo_b);

    // Metadata-only edit (empty repos = leave unchanged).
    let only_desc = wsc
        .update_workspace(UpdateWorkspaceRequest {
            workspace_id: ws.id.clone(),
            name: None,
            icon: None,
            description: Some("now described".to_string()),
            repos: vec![],
        })
        .await
        .expect("update desc")
        .into_inner();
    assert_eq!(only_desc.description.as_deref(), Some("now described"));
    assert_eq!(only_desc.name, "After"); // name untouched

    let still_two = wsc
        .list_workspace_repos(WorkspaceId { value: ws.id })
        .await
        .expect("list repos again")
        .into_inner();
    assert_eq!(still_two.repos.len(), 2, "empty repos = no change");

    core.shutdown().await.expect("shutdown");
}
