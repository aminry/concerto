//! Round-trip test for the Task 306 position-aware `workspace_repos`
//! writer ([`workspaces::update_repos`]) + reader
//! ([`workspaces::list_repos`]).
//!
//! Migration 0009 adds `workspace_repos.position INTEGER NOT NULL
//! DEFAULT 0`. `update_repos` stamps `position = slice index`;
//! `list_repos` returns rows ordered by `(position, repository_id)`.
//! This proves:
//! - insertion order is preserved as `position` 0/1/2 and read back in
//!   declaration order (NOT id-sorted),
//! - a re-`update_repos` with a reordered slice re-positions the set,
//! - a row written through the legacy two-column INSERT path backfills
//!   to `position = 0` via the `DEFAULT 0`.

use concerto_persist::{
    workspaces, NewProject, NewRepository, NewWorkspace, Persistence, PersistenceConfig, ProjectId,
    RepositoryId, WorkspaceId,
};
use sqlx::Row;

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

/// Seed a project + three repos + one workspace (no junction rows yet).
async fn seed(persist: &Persistence) -> (WorkspaceId, [RepositoryId; 3]) {
    let project_id = ProjectId("proj-1".to_string());
    let ws_id = WorkspaceId("ws-1".to_string());
    // Names chosen so id-sort order (api < android < ios is false:
    // "android" < "api" < "ios") differs from declaration order, making
    // a position-driven read distinguishable from `ORDER BY repository_id`.
    let repos = [
        RepositoryId("api".to_string()),
        RepositoryId("android".to_string()),
        RepositoryId("ios".to_string()),
    ];

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

    for r in &repos {
        concerto_persist::repositories::insert(
            &mut w,
            NewRepository {
                id: r.clone(),
                project_id: project_id.0.clone(),
                name: r.0.clone(),
                url: format!("file:///tmp/{}.git", r.0),
                local_path: format!("/tmp/repos/{}", r.0),
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
    drop(w);

    (ws_id, repos)
}

#[tokio::test]
async fn update_repos_writes_positions_and_list_repos_orders_by_position() {
    let (_dir, persist) = fresh_db().await;
    let (ws, repos) = seed(&persist).await;
    // Declaration order: api(0), android(1), ios(2) — NOT id-sorted.
    let declared = vec![repos[0].clone(), repos[1].clone(), repos[2].clone()];

    {
        let mut w = persist.writer().await;
        workspaces::update_repos(&mut w, &ws, &declared)
            .await
            .expect("update_repos");
    }

    // Positions written = slice index.
    let pool = persist.readers();
    let rows = sqlx::query(
        "SELECT repository_id, position FROM workspace_repos \
         WHERE workspace_id = ? ORDER BY position",
    )
    .bind(&ws.0)
    .fetch_all(pool)
    .await
    .expect("rows");
    let positioned: Vec<(String, i64)> = rows
        .into_iter()
        .map(|r| {
            (
                r.get::<String, _>("repository_id"),
                r.get::<i64, _>("position"),
            )
        })
        .collect();
    assert_eq!(
        positioned,
        vec![
            ("api".to_string(), 0),
            ("android".to_string(), 1),
            ("ios".to_string(), 2),
        ],
        "position must equal the slice index"
    );

    // `list_repos` returns declaration order, not id-sorted order.
    let listed = workspaces::list_repos(pool, &ws).await.expect("list_repos");
    assert_eq!(
        listed, declared,
        "list_repos must return repos in declaration (position) order"
    );
}

#[tokio::test]
async fn update_repos_reorders_on_recall() {
    let (_dir, persist) = fresh_db().await;
    let (ws, repos) = seed(&persist).await;

    {
        let mut w = persist.writer().await;
        workspaces::update_repos(
            &mut w,
            &ws,
            &[repos[0].clone(), repos[1].clone(), repos[2].clone()],
        )
        .await
        .expect("initial update_repos");
    }

    // Re-call with a reordered + reduced slice: ios(0), api(1).
    let reordered = vec![repos[2].clone(), repos[0].clone()];
    {
        let mut w = persist.writer().await;
        workspaces::update_repos(&mut w, &ws, &reordered)
            .await
            .expect("reorder update_repos");
    }

    let listed = workspaces::list_repos(persist.readers(), &ws)
        .await
        .expect("list_repos");
    assert_eq!(
        listed, reordered,
        "re-calling update_repos must re-position the set"
    );
}

#[tokio::test]
async fn legacy_two_column_insert_backfills_position_zero() {
    // Migration 0009's `DEFAULT 0` backfills any row written without an
    // explicit position (the V0.1 / migration-upgrade path).
    let (_dir, persist) = fresh_db().await;
    let (ws, repos) = seed(&persist).await;

    {
        let mut w = persist.writer().await;
        sqlx::query("INSERT INTO workspace_repos (workspace_id, repository_id) VALUES (?, ?)")
            .bind(&ws.0)
            .bind(&repos[0].0)
            .execute(&mut *w)
            .await
            .expect("legacy insert");
    }

    let (position,): (i64,) =
        sqlx::query_as("SELECT position FROM workspace_repos WHERE workspace_id = ?")
            .bind(&ws.0)
            .fetch_one(persist.readers())
            .await
            .expect("position");
    assert_eq!(position, 0, "DEFAULT 0 must backfill the legacy row");
}
