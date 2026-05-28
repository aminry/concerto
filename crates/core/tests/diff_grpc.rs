//! Integration test for the Task 29 `Workareas.GetWorkareaRepoDiff` RPC.
//!
//! Exercises the full path:
//! - spawn a real Core subprocess via the Task 17 harness,
//! - seed `projects` + `repositories` + `workspaces` + `workspace_repos`
//!   the same way `workarea_lifecycle.rs` does,
//! - call `Workareas.CreateWorkarea` to lay down the worktree,
//! - modify a file inside the worktree,
//! - call `Workareas.GetWorkareaRepoDiff` and assert the returned
//!   payload reports the modified file as `DIFF_KIND_MODIFIED`.

#![cfg(unix)]

use std::path::Path;

use concerto_proto::v1::{CreateWorkareaRequest, DiffKind, GetDiffRequest};
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
    workspace_slug: String,
    repo_id: String,
    repo_name: String,
    _bare: TempDir,
    _work: TempDir,
}

async fn seed(core: &CoreUnderTest, slug: &str) -> Seeded {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    let (bare_url, bare, work) = make_bare_with_commit().await;

    let project_id = format!("proj-{slug}");
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
    sqlx::query("INSERT INTO projects (id, name, created_at) VALUES (?, 'test', 0)")
        .bind(&project_id)
        .execute(&pool)
        .await
        .expect("insert project");
    sqlx::query(
        "INSERT INTO repositories (id, project_id, name, url, local_path, clone_strategy, default_branch)
         VALUES (?, ?, ?, ?, ?, 'full', 'main')",
    )
    .bind(&repo_id)
    .bind(&project_id)
    .bind(&repo_name)
    .bind(&bare_url)
    .bind(local_path.to_string_lossy().to_string())
    .execute(&pool)
    .await
    .expect("insert repository");
    sqlx::query(
        "INSERT INTO workspaces (id, project_id, name, slug, created_at) VALUES (?, ?, 'test', ?, 0)",
    )
    .bind(&workspace_id)
    .bind(&project_id)
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
        workspace_slug: slug.to_string(),
        repo_id,
        repo_name,
        _bare: bare,
        _work: work,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn get_workarea_repo_diff_returns_modified_file() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let s = seed(&core, "diff").await;

    let mut wac = core.workareas_client().await.expect("workareas client");
    let wa = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: s.workspace_id.clone(),
            permission_mode: None,
        })
        .await
        .expect("CreateWorkarea")
        .into_inner();

    // Locate the worktree on disk and modify the seeded README.
    let worktree_root = core
        .data_dir
        .join("workspaces")
        .join(&s.workspace_slug)
        .join(&wa.composer_name);
    let repo_worktree = worktree_root.join(&s.repo_name);
    let readme = repo_worktree.join("README.md");
    assert!(
        readme.is_file(),
        "seeded README should exist at {}",
        readme.display()
    );
    tokio::fs::write(&readme, "hello\nplus a second line\n")
        .await
        .expect("modify README");

    let payload = wac
        .get_workarea_repo_diff(GetDiffRequest {
            workarea_id: wa.id.clone(),
            repository_id: s.repo_id.clone(),
        })
        .await
        .expect("GetWorkareaRepoDiff")
        .into_inner();

    assert!(
        !payload.files.is_empty(),
        "diff should not be empty after a modification"
    );
    let readme_entry = payload
        .files
        .iter()
        .find(|f| f.path == "README.md")
        .expect("README should appear in the diff");
    assert_eq!(readme_entry.kind, DiffKind::Modified as i32);

    // Hunks should describe the addition of the second line. The exact
    // content depends on git's diff output; assert structurally rather
    // than on a specific byte string.
    assert!(
        !readme_entry.hunks.is_empty(),
        "modified file should carry at least one hunk; got {readme_entry:?}"
    );

    core.shutdown().await.expect("shutdown");
}
