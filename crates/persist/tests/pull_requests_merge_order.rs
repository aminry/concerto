//! Tests for the Task 319 PR-set semantics (migration 0014): the three new
//! `pull_requests` columns (`merge_order`, `external_id`,
//! `repository_full_name`), the insertion-order default, `merge_order`
//! preservation across re-upserts, `(merge_order, pr_number)` ordering,
//! `set_merge_order` reorder, and the FROZEN
//! `UNIQUE(workarea_id, repository_id)` invariant.

use concerto_persist::{
    pull_requests, NewProject, NewPullRequest, NewRepository, NewWorkarea, NewWorkspace,
    Persistence, PersistenceConfig, ProjectId, PullRequestId, RepositoryId, WorkareaId,
    WorkspaceId,
};

async fn fresh_db() -> (tempfile::TempDir, Persistence) {
    let dir = tempfile::tempdir().expect("tempdir");
    let persist = Persistence::open(PersistenceConfig {
        db_path: dir.path().join("test.db"),
        max_readers: 2,
    })
    .await
    .expect("open");
    (dir, persist)
}

/// Seed a project + N repos + one workspace + one workarea. Returns the
/// workarea id and the seeded repo ids (in declaration order).
async fn seed(persist: &Persistence, repo_ids: &[&str]) -> (WorkareaId, Vec<RepositoryId>) {
    let project_id = ProjectId("proj-1".to_string());
    let workspace_id = WorkspaceId("ws-1".to_string());
    let workarea_id = WorkareaId("wa-1".to_string());
    let repos: Vec<RepositoryId> = repo_ids
        .iter()
        .map(|r| RepositoryId(r.to_string()))
        .collect();

    let mut w = persist.writer().await;
    concerto_persist::projects::insert(
        &mut w,
        NewProject {
            id: project_id.clone(),
            name: "Test".to_string(),
            icon: None,
            created_at: 1,
        },
    )
    .await
    .expect("insert project");

    for r in &repos {
        concerto_persist::repositories::insert(
            &mut w,
            NewRepository {
                id: r.clone(),
                project_id: project_id.0.clone(),
                name: r.0.clone(),
                url: format!("https://github.com/acme/{}", r.0),
                local_path: format!("/tmp/{}", r.0),
                clone_strategy: "full".to_string(),
                default_branch: "main".to_string(),
            },
        )
        .await
        .expect("insert repo");
    }

    concerto_persist::workspaces::insert(
        &mut w,
        NewWorkspace {
            id: workspace_id.clone(),
            project_id: project_id.0.clone(),
            name: "WS".to_string(),
            slug: "ws".to_string(),
            description: None,
            permission_mode: None,
            created_at: 1,
        },
    )
    .await
    .expect("insert workspace");

    concerto_persist::workareas::insert(
        &mut w,
        NewWorkarea {
            id: workarea_id.clone(),
            workspace_id: workspace_id.0.clone(),
            composer_name: "bach".to_string(),
            branch_name: "concerto/bach".to_string(),
            worktree_root: "/tmp/wa".to_string(),
            status: "active".to_string(),
            permission_mode: None,
            created_at: 1,
        },
    )
    .await
    .expect("insert workarea");

    (workarea_id, repos)
}

/// Build a `NewPullRequest` for a (workarea, repo) with a given pr_number,
/// merge_order, external_id, and repository_full_name.
#[allow(clippy::too_many_arguments)]
fn new_pr(
    workarea_id: &WorkareaId,
    repository_id: &RepositoryId,
    pr_number: i64,
    merge_order: i64,
    external_id: &str,
    repository_full_name: &str,
) -> NewPullRequest {
    NewPullRequest {
        // Deterministic per (workarea, repo) — the upsert keys on
        // `(workarea_id, repository_id)` so a stable id is fine and avoids a
        // `uuid` dev-dependency.
        id: PullRequestId(format!("pr-{}-{}", workarea_id.0, repository_id.0)),
        workarea_id: workarea_id.clone(),
        repository_id: repository_id.clone(),
        provider: "github".to_string(),
        pr_number,
        base_ref: "main".to_string(),
        head_ref: "feature".to_string(),
        state: "open".to_string(),
        title: "T".to_string(),
        body: String::new(),
        url: String::new(),
        head_sha: "deadbeef".to_string(),
        merge_order,
        external_id: external_id.to_string(),
        repository_full_name: repository_full_name.to_string(),
        created_at: 1,
        updated_at: 1,
    }
}

#[tokio::test]
async fn columns_round_trip() {
    let (_dir, persist) = fresh_db().await;
    let (wa, repos) = seed(&persist, &["repo-a"]).await;

    let id = {
        let mut w = persist.writer().await;
        pull_requests::upsert(
            &mut w,
            new_pr(&wa, &repos[0], 7, 3, "PR_node_xyz", "acme/repo-a"),
        )
        .await
        .expect("upsert")
    };

    let got = pull_requests::get(persist.readers(), &id)
        .await
        .expect("get")
        .expect("present");
    assert_eq!(got.merge_order, 3);
    assert_eq!(got.external_id, "PR_node_xyz");
    assert_eq!(got.repository_full_name, "acme/repo-a");
    assert_eq!(got.pr_number, 7);
}

#[tokio::test]
async fn insertion_order_default_assigns_0_then_1() {
    let (_dir, persist) = fresh_db().await;
    let (wa, repos) = seed(&persist, &["repo-a", "repo-b"]).await;

    let mut w = persist.writer().await;

    // First PR: next_merge_order == 0.
    let o0 = pull_requests::next_merge_order(&mut w, &wa).await.unwrap();
    assert_eq!(o0, 0);
    pull_requests::upsert(&mut w, new_pr(&wa, &repos[0], 1, o0, "", ""))
        .await
        .unwrap();

    // Second PR (distinct repo): next_merge_order == 1.
    let o1 = pull_requests::next_merge_order(&mut w, &wa).await.unwrap();
    assert_eq!(o1, 1);
    pull_requests::upsert(&mut w, new_pr(&wa, &repos[1], 2, o1, "", ""))
        .await
        .unwrap();
    drop(w);

    let set = pull_requests::list_by_workarea(persist.readers(), &wa)
        .await
        .unwrap();
    let orders: Vec<i64> = set.iter().map(|p| p.merge_order).collect();
    assert_eq!(orders, vec![0, 1]);
}

#[tokio::test]
async fn reupsert_preserves_merge_order_refreshes_graphql_fields() {
    let (_dir, persist) = fresh_db().await;
    let (wa, repos) = seed(&persist, &["repo-a"]).await;

    // Insert with merge_order 5 (user-ordered) + initial GraphQL fields.
    let id = {
        let mut w = persist.writer().await;
        pull_requests::upsert(
            &mut w,
            new_pr(&wa, &repos[0], 7, 5, "node_old", "acme/repo-a"),
        )
        .await
        .unwrap()
    };

    // A re-sync upserts the SAME (workarea, repo) with a different
    // merge_order and refreshed GraphQL fields. merge_order must be
    // preserved (5); external_id/repository_full_name must refresh.
    {
        let mut w = persist.writer().await;
        let row = new_pr(&wa, &repos[0], 7, 999, "node_new", "acme/repo-a-renamed");
        let same_id = pull_requests::upsert(&mut w, row).await.unwrap();
        assert_eq!(same_id, id, "upsert keeps the original primary key");
    }

    let got = pull_requests::get(persist.readers(), &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        got.merge_order, 5,
        "re-sync must not reset the user's reorder"
    );
    assert_eq!(got.external_id, "node_new");
    assert_eq!(got.repository_full_name, "acme/repo-a-renamed");
}

#[tokio::test]
async fn list_orders_by_merge_order_then_pr_number() {
    let (_dir, persist) = fresh_db().await;
    let (wa, repos) = seed(&persist, &["repo-a", "repo-b", "repo-c"]).await;

    let mut w = persist.writer().await;
    // repo-a: merge_order 2, pr 10
    pull_requests::upsert(&mut w, new_pr(&wa, &repos[0], 10, 2, "", ""))
        .await
        .unwrap();
    // repo-b: merge_order 0, pr 30
    pull_requests::upsert(&mut w, new_pr(&wa, &repos[1], 30, 0, "", ""))
        .await
        .unwrap();
    // repo-c: merge_order 0, pr 20  (ties repo-b on merge_order; lower pr first)
    pull_requests::upsert(&mut w, new_pr(&wa, &repos[2], 20, 0, "", ""))
        .await
        .unwrap();
    drop(w);

    let set = pull_requests::list_by_workarea(persist.readers(), &wa)
        .await
        .unwrap();
    let prs: Vec<i64> = set.iter().map(|p| p.pr_number).collect();
    // (0,20) (0,30) (2,10)
    assert_eq!(prs, vec![20, 30, 10]);
}

#[tokio::test]
async fn set_merge_order_reorders() {
    let (_dir, persist) = fresh_db().await;
    let (wa, repos) = seed(&persist, &["repo-a", "repo-b"]).await;

    let mut w = persist.writer().await;
    pull_requests::upsert(&mut w, new_pr(&wa, &repos[0], 1, 0, "", ""))
        .await
        .unwrap();
    pull_requests::upsert(&mut w, new_pr(&wa, &repos[1], 2, 1, "", ""))
        .await
        .unwrap();
    drop(w);

    // Move repo-b to the front (merge_order -1).
    let pr_b = pull_requests::id_by_workarea_repo(persist.readers(), &wa, &repos[1])
        .await
        .unwrap()
        .expect("repo-b PR present");
    {
        let mut w = persist.writer().await;
        pull_requests::set_merge_order(&mut w, &pr_b, -1)
            .await
            .unwrap();
    }

    let set = pull_requests::list_by_workarea(persist.readers(), &wa)
        .await
        .unwrap();
    let prs: Vec<i64> = set.iter().map(|p| p.pr_number).collect();
    assert_eq!(prs, vec![2, 1], "repo-b's PR now sorts first");

    // Setting an unknown id is NotFound.
    let err = {
        let mut w = persist.writer().await;
        pull_requests::set_merge_order(&mut w, &PullRequestId("nope".into()), 0).await
    };
    assert!(err.is_err());
}

#[tokio::test]
async fn unique_workarea_repo_still_enforced() {
    let (_dir, persist) = fresh_db().await;
    let (wa, repos) = seed(&persist, &["repo-a"]).await;

    let mut w = persist.writer().await;
    let id1 = pull_requests::upsert(&mut w, new_pr(&wa, &repos[0], 1, 0, "", ""))
        .await
        .unwrap();
    // A second PR for the SAME (workarea, repo) upserts onto the same row
    // (the UNIQUE constraint folds it) rather than creating a duplicate.
    let id2 = pull_requests::upsert(&mut w, new_pr(&wa, &repos[0], 2, 7, "", ""))
        .await
        .unwrap();
    assert_eq!(id1, id2);
    drop(w);

    let set = pull_requests::list_by_workarea(persist.readers(), &wa)
        .await
        .unwrap();
    assert_eq!(set.len(), 1, "one PR per repo per workarea");
    assert_eq!(set[0].pr_number, 2, "the upsert refreshed pr_number");
    assert_eq!(set[0].merge_order, 0, "but preserved the first merge_order");
}
