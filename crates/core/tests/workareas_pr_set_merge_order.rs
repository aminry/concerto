//! Task 319: `WorkareaManager` PR-set ordering + `set_merge_order` — the
//! logic behind `Workareas.GetWorkareaPrSet` (now `(merge_order, pr_number)`
//! ordered) and `Workareas.SetMergeOrder` (write + return the re-ordered set).
//!
//! Exercised in-process against a real `Persistence` (no subprocess): seed a
//! multi-repo workarea, assert `list_pr_set` returns merge-order-sorted rows,
//! then `set_merge_order` reorders and returns the new order.

use std::sync::Arc;

use concerto_core::repo_manager::RepoManager;
use concerto_core::workspace_manager::WorkareaManager;
use concerto_persist::{
    pull_requests, NewProject, NewPullRequest, NewRepository, NewWorkarea, NewWorkspace,
    Persistence, PersistenceConfig, ProjectId, PullRequestId, RepositoryId, WorkareaId,
    WorkspaceId,
};

struct Ctx {
    _dir: tempfile::TempDir,
    persist: Arc<Persistence>,
    manager: WorkareaManager,
    workarea_id: WorkareaId,
    repos: Vec<RepositoryId>,
}

async fn setup(repo_ids: &[&str]) -> Ctx {
    let dir = tempfile::tempdir().expect("tempdir");
    let persist = Arc::new(
        Persistence::open(PersistenceConfig {
            db_path: dir.path().join("test.db"),
            max_readers: 2,
        })
        .await
        .expect("open"),
    );

    let project_id = ProjectId("proj-1".to_string());
    let workspace_id = WorkspaceId("ws-1".to_string());
    let workarea_id = WorkareaId("wa-1".to_string());
    let repos: Vec<RepositoryId> = repo_ids
        .iter()
        .map(|r| RepositoryId(r.to_string()))
        .collect();

    {
        let mut w = persist.writer().await;
        concerto_persist::projects::insert(
            &mut w,
            NewProject {
                id: project_id.clone(),
                name: "Test".into(),
                icon: None,
                created_at: 1,
            },
        )
        .await
        .unwrap();
        for r in &repos {
            concerto_persist::repositories::insert(
                &mut w,
                NewRepository {
                    id: r.clone(),
                    project_id: project_id.0.clone(),
                    name: r.0.clone(),
                    url: format!("https://github.com/acme/{}", r.0),
                    local_path: format!("/tmp/{}", r.0),
                    clone_strategy: "full".into(),
                    default_branch: "main".into(),
                },
            )
            .await
            .unwrap();
        }
        concerto_persist::workspaces::insert(
            &mut w,
            NewWorkspace {
                id: workspace_id.clone(),
                project_id: project_id.0.clone(),
                name: "WS".into(),
                slug: "ws".into(),
                description: None,
                permission_mode: None,
                created_at: 1,
            },
        )
        .await
        .unwrap();
        concerto_persist::workareas::insert(
            &mut w,
            NewWorkarea {
                id: workarea_id.clone(),
                workspace_id: workspace_id.0.clone(),
                composer_name: "bach".into(),
                branch_name: "concerto/bach".into(),
                worktree_root: "/tmp/wa".into(),
                status: "active".into(),
                permission_mode: None,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    }

    let repo_manager = RepoManager::new(Arc::clone(&persist), dir.path().join("repos"));
    let manager = WorkareaManager::new(
        Arc::clone(&persist),
        repo_manager,
        Arc::new(dir.path().join("data")),
        Arc::new(dir.path().join("config")),
    );

    Ctx {
        _dir: dir,
        persist,
        manager,
        workarea_id,
        repos,
    }
}

fn new_pr(
    workarea_id: &WorkareaId,
    repository_id: &RepositoryId,
    pr_number: i64,
    merge_order: i64,
) -> NewPullRequest {
    NewPullRequest {
        id: PullRequestId(uuid::Uuid::now_v7().to_string()),
        workarea_id: workarea_id.clone(),
        repository_id: repository_id.clone(),
        provider: "github".into(),
        pr_number,
        base_ref: "main".into(),
        head_ref: "feature".into(),
        state: "open".into(),
        title: "T".into(),
        body: String::new(),
        url: String::new(),
        head_sha: "deadbeef".into(),
        merge_order,
        external_id: String::new(),
        repository_full_name: format!("acme/{}", repository_id.0),
        created_at: 1,
        updated_at: 1,
    }
}

#[tokio::test]
async fn get_pr_set_is_merge_order_sorted_multi_repo() {
    let ctx = setup(&["repo-a", "repo-b", "repo-c"]).await;
    // Insert out of order: merge_order 2, 0, 1.
    {
        let mut w = ctx.persist.writer().await;
        pull_requests::upsert(&mut w, new_pr(&ctx.workarea_id, &ctx.repos[0], 10, 2))
            .await
            .unwrap();
        pull_requests::upsert(&mut w, new_pr(&ctx.workarea_id, &ctx.repos[1], 20, 0))
            .await
            .unwrap();
        pull_requests::upsert(&mut w, new_pr(&ctx.workarea_id, &ctx.repos[2], 30, 1))
            .await
            .unwrap();
    }

    let set = ctx.manager.list_pr_set(&ctx.workarea_id).await.unwrap();
    let prs: Vec<i64> = set.iter().map(|p| p.pr_number).collect();
    assert_eq!(prs, vec![20, 30, 10], "ordered by merge_order 0,1,2");
    assert_eq!(set.len(), 3, "a multi-repo workarea yields multiple rows");
}

#[tokio::test]
async fn set_merge_order_writes_and_returns_reordered_set() {
    let ctx = setup(&["repo-a", "repo-b"]).await;
    {
        let mut w = ctx.persist.writer().await;
        pull_requests::upsert(&mut w, new_pr(&ctx.workarea_id, &ctx.repos[0], 1, 0))
            .await
            .unwrap();
        pull_requests::upsert(&mut w, new_pr(&ctx.workarea_id, &ctx.repos[1], 2, 1))
            .await
            .unwrap();
    }

    // Move repo-b's PR to the front.
    let reordered = ctx
        .manager
        .set_merge_order(&ctx.workarea_id, &ctx.repos[1], -5)
        .await
        .unwrap();
    let prs: Vec<i64> = reordered.iter().map(|p| p.pr_number).collect();
    assert_eq!(prs, vec![2, 1], "SetMergeOrder returns the new order");

    // A re-read sees the same order (it was persisted).
    let set = ctx.manager.list_pr_set(&ctx.workarea_id).await.unwrap();
    let prs: Vec<i64> = set.iter().map(|p| p.pr_number).collect();
    assert_eq!(prs, vec![2, 1]);
}

#[tokio::test]
async fn set_merge_order_unknown_repo_is_not_found() {
    let ctx = setup(&["repo-a"]).await;
    let err = ctx
        .manager
        .set_merge_order(&ctx.workarea_id, &RepositoryId("ghost".into()), 0)
        .await;
    assert!(err.is_err(), "no PR for that repo ⇒ NotFound");
}

#[tokio::test]
async fn list_pr_set_unknown_workarea_is_not_found() {
    let ctx = setup(&["repo-a"]).await;
    let err = ctx.manager.list_pr_set(&WorkareaId("nope".into())).await;
    assert!(err.is_err());
}
