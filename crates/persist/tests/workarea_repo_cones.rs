//! Round-trip test for the Task 302 `workarea_repos.sparse_cones_json`
//! writer (`update_workarea_repo_cones`) + reader (`get_workarea_repo_cones`).
//!
//! V0.1's `insert_workarea_repo` never wrote `sparse_cones_json` (it relied
//! on the SQL `DEFAULT '[]'`); Task 302 adds the column to the INSERT and a
//! dedicated UPDATE writer. This test proves the cone set written by
//! `update_workarea_repo_cones` round-trips back through
//! `get_workarea_repo_cones` in the FROZEN flat `["<cone_path>", …]` JSON
//! shape, and that the default-empty insert reads back as `"[]"`.

use concerto_persist::{
    workareas, NewProject, NewRepository, NewWorkarea, NewWorkareaRepo, NewWorkspace, Persistence,
    PersistenceConfig, ProjectId, RepositoryId, WorkareaId, WorkspaceId,
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

/// Seed project → repo → workspace → workarea → workarea_repos and return the
/// (workarea, repo) ids. The junction row is inserted with the default-empty
/// cone via [`NewWorkareaRepo::empty_cones`].
async fn seed(persist: &Persistence) -> (WorkareaId, RepositoryId) {
    let project_id = ProjectId("proj-1".to_string());
    let repo_id = RepositoryId("repo-1".to_string());
    let ws_id = WorkspaceId("ws-1".to_string());
    let wa_id = WorkareaId("wa-1".to_string());

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

    concerto_persist::repositories::insert(
        &mut w,
        NewRepository {
            id: repo_id.clone(),
            project_id: project_id.0.clone(),
            name: "smoke-repo".to_string(),
            url: "file:///tmp/bare.git".to_string(),
            local_path: "/tmp/repos/repo-1".to_string(),
            clone_strategy: "blobless".to_string(),
            default_branch: "main".to_string(),
        },
    )
    .await
    .expect("insert repo");

    concerto_persist::workspaces::insert(
        &mut w,
        NewWorkspace {
            id: ws_id.clone(),
            project_id: project_id.0.clone(),
            name: "WS".to_string(),
            slug: "ws".to_string(),
            description: None,
            permission_mode: None,
            created_at: 1_700_000_001_000,
        },
    )
    .await
    .expect("insert workspace");

    workareas::insert(
        &mut w,
        NewWorkarea {
            id: wa_id.clone(),
            workspace_id: ws_id.0.clone(),
            composer_name: "bach".to_string(),
            branch_name: "concerto/bach".to_string(),
            worktree_root: "/tmp/wa/bach".to_string(),
            status: "created".to_string(),
            permission_mode: None,
            created_at: 1_700_000_002_000,
        },
    )
    .await
    .expect("insert workarea");

    workareas::insert_workarea_repo(
        &mut w,
        NewWorkareaRepo {
            workarea_id: wa_id.clone(),
            repository_id: repo_id.clone(),
            worktree_path: "/tmp/wa/bach/smoke-repo".to_string(),
            branch_override: None,
            sparse_cones_json: NewWorkareaRepo::empty_cones(),
        },
    )
    .await
    .expect("insert workarea_repo");

    drop(w);
    (wa_id, repo_id)
}

#[tokio::test]
async fn workarea_repo_cones_default_empty_then_round_trip() {
    let (_dir, persist) = fresh_db().await;
    let (wa, repo) = seed(&persist).await;

    // Default insert path wrote the empty cone literal.
    let initial = workareas::get_workarea_repo_cones(persist.readers(), &wa, &repo)
        .await
        .expect("read cones")
        .expect("junction row exists");
    assert_eq!(
        initial, "[]",
        "default cone should be the empty-array literal"
    );

    // Write a non-empty cone set and read it back in the FROZEN flat shape.
    let cones = vec!["a".to_string(), "packages/core".to_string()];
    {
        let mut w = persist.writer().await;
        workareas::update_workarea_repo_cones(&mut w, &wa, &repo, &cones)
            .await
            .expect("update cones");
    }
    let after = workareas::get_workarea_repo_cones(persist.readers(), &wa, &repo)
        .await
        .expect("read cones")
        .expect("junction row exists");
    let parsed: Vec<String> = serde_json::from_str(&after).expect("valid JSON array");
    assert_eq!(parsed, cones, "written cone set must round-trip");

    // Overwriting with an explicit empty set is a legitimate "top-level files
    // only" cone and must persist as `[]`.
    {
        let mut w = persist.writer().await;
        workareas::update_workarea_repo_cones(&mut w, &wa, &repo, &[])
            .await
            .expect("update to empty");
    }
    let empty = workareas::get_workarea_repo_cones(persist.readers(), &wa, &repo)
        .await
        .expect("read cones")
        .expect("junction row exists");
    assert_eq!(empty, "[]", "explicit empty cone must persist as []");
}

#[tokio::test]
async fn get_workarea_repo_cones_absent_pair_is_none() {
    let (_dir, persist) = fresh_db().await;
    let _ = seed(&persist).await;

    let missing = workareas::get_workarea_repo_cones(
        persist.readers(),
        &WorkareaId("nope".to_string()),
        &RepositoryId("nope".to_string()),
    )
    .await
    .expect("read cones");
    assert!(missing.is_none(), "absent junction row → None");
}
