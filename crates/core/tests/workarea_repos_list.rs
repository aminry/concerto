//! Integration test for `Workareas.ListWorkareaRepos`.
//!
//! Regression guard for the Desktop Diff-panel bug: after the
//! Project→Workspace collapse, `Repositories.ListRepositories` became a
//! GLOBAL/unscoped list (every repo across all workspaces). The Diff panel
//! must instead list ONLY the repos a workarea actually materialized (the
//! `workarea_repos` junction) — every one of which `GetWorkareaRepoDiff`
//! accepts. This test seeds TWO workspaces (each with its own repo), creates
//! a workarea in the first, and asserts:
//! - `ListWorkareaRepos(workarea)` returns ONLY repo A (never repo B from the
//!   OTHER workspace, even though both are in the global registry),
//! - the listed repo id is diff-able via `GetWorkareaRepoDiff`,
//! - a non-attached repo (B) still errors "not attached to workarea".

#![cfg(unix)]

use std::path::Path;

use concerto_proto::v1::{CreateWorkareaRequest, GetDiffRequest, ListWorkareaReposRequest};
use concerto_test_harness::CoreUnderTest;
use tempfile::TempDir;
use tokio::process::Command;

async fn git(args: &[&str], cwd: &Path) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .await
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

async fn make_bare_with_commit() -> (String, TempDir, TempDir) {
    let bare = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    git(&["init", "--bare", "-b", "main", "."], bare.path()).await;
    git(&["init", "-b", "main", "."], work.path()).await;
    tokio::fs::write(work.path().join("README.md"), "hello\n")
        .await
        .unwrap();
    git(&["add", "README.md"], work.path()).await;
    git(&["commit", "-m", "initial"], work.path()).await;
    let url = format!("file://{}", bare.path().display());
    git(&["remote", "add", "origin", url.as_str()], work.path()).await;
    git(&["push", "-u", "origin", "main"], work.path()).await;
    (url, bare, work)
}

struct Seeded {
    workspace_id: String,
    repo_id: String,
    _bare: TempDir,
    _work: TempDir,
}

/// Seed one workspace with exactly one repo (its own bare origin, pre-cloned
/// into the Core's repo pool). Mirrors `diff_grpc.rs::seed`.
async fn seed(core: &CoreUnderTest, slug: &str) -> Seeded {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    let (bare_url, bare, work) = make_bare_with_commit().await;

    let workspace_id = format!("ws-{slug}");
    let repo_id = format!("repo-{slug}");
    let repo_name = format!("name-{slug}");
    let local_path = core.data_dir.join("repos").join(&repo_id);

    let opts = SqliteConnectOptions::new()
        .filename(&core.db_path)
        .create_if_missing(false);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("open db write pool");
    sqlx::query(
        "INSERT INTO repositories (id, name, url, local_path, clone_strategy, default_branch)
         VALUES (?, ?, ?, ?, 'full', 'main')",
    )
    .bind(&repo_id)
    .bind(&repo_name)
    .bind(&bare_url)
    .bind(local_path.to_string_lossy().to_string())
    .execute(&pool)
    .await
    .expect("insert repository");
    sqlx::query("INSERT INTO workspaces (id, name, slug, created_at) VALUES (?, 'test', ?, 0)")
        .bind(&workspace_id)
        .bind(slug)
        .execute(&pool)
        .await
        .expect("insert workspace");
    sqlx::query("INSERT INTO workspace_repos (workspace_id, repository_id) VALUES (?, ?)")
        .bind(&workspace_id)
        .bind(&repo_id)
        .execute(&pool)
        .await
        .expect("insert workspace_repos");
    pool.close().await;

    // Pre-clone so the workarea-creation path picks up the existing clone.
    tokio::fs::create_dir_all(local_path.parent().unwrap())
        .await
        .unwrap();
    let out = Command::new("git")
        .args(["clone", bare_url.as_str(), &local_path.to_string_lossy()])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .await
        .expect("git clone");
    assert!(
        out.status.success(),
        "seed clone failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    Seeded {
        workspace_id,
        repo_id,
        _bare: bare,
        _work: work,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn list_workarea_repos_is_scoped_to_the_workarea() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");

    // Two independent workspaces, each with its own repo. Both repos live in
    // the SAME global `repositories` registry — so the unscoped
    // `Repositories.ListRepositories` would return both.
    let a = seed(&core, "alpha").await;
    let b = seed(&core, "beta").await;

    // Create a workarea in workspace A only.
    let mut wac = core.workareas_client().await.expect("workareas client");
    let wa = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: a.workspace_id.clone(),
            permission_mode: None,
        })
        .await
        .expect("CreateWorkarea")
        .into_inner();

    // ListWorkareaRepos returns ONLY repo A — the repo materialized in this
    // workarea — never repo B from the other workspace.
    let listed = wac
        .list_workarea_repos(ListWorkareaReposRequest {
            workarea_id: wa.id.clone(),
        })
        .await
        .expect("ListWorkareaRepos")
        .into_inner();
    let listed_ids: Vec<String> = listed.repositories.iter().map(|r| r.id.clone()).collect();
    assert_eq!(
        listed_ids,
        vec![a.repo_id.clone()],
        "ListWorkareaRepos must be workarea-scoped: only repo A, not repo B"
    );
    assert!(
        !listed_ids.contains(&b.repo_id),
        "repo B (a different workspace's repo) must NOT appear"
    );

    // The listed repo id is exactly what GetWorkareaRepoDiff accepts.
    wac.get_workarea_repo_diff(GetDiffRequest {
        workarea_id: wa.id.clone(),
        repository_id: a.repo_id.clone(),
    })
    .await
    .expect("GetWorkareaRepoDiff accepts the listed repo");

    // The non-attached repo B still errors "not attached to workarea" — the
    // exact failure the Diff panel hit when it sourced the global registry.
    let err = wac
        .get_workarea_repo_diff(GetDiffRequest {
            workarea_id: wa.id.clone(),
            repository_id: b.repo_id.clone(),
        })
        .await
        .expect_err("a non-attached repo must be rejected");
    assert!(
        err.message().contains("not attached to workarea"),
        "expected a 'not attached to workarea' error, got: {}",
        err.message()
    );

    core.shutdown().await.expect("shutdown");
}
