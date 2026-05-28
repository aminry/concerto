//! Archive + restore lifecycle integration tests (Task 31).
//!
//! Covers the four scenarios called out by the task spec:
//! 1. Archive a workarea with no live session, default opts; verify
//!    `archived_at` set + worktree kept.
//! 2. Archive a workarea with `remove_worktree=true`; verify the
//!    worktree directory is gone.
//! 3. Restore a workarea whose worktree was removed; verify the
//!    worktree is re-created and `permission_mode` reset to NULL.
//! 4. Archive a workspace with multiple workareas; verify the cascade
//!    archives every workarea, then restore the workspace and confirm
//!    the workspace timestamp clears but workareas stay archived.
//!
//! Permission-mode reset on restore is verified inside scenario 3.

#![cfg(unix)]

use std::path::Path;

use concerto_proto::v1::{
    ArchiveWorkareaRequest, CreateWorkareaRequest, ListWorkareasRequest, PermissionMode,
    WorkareaId, WorkspaceId,
};
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
        "git {:?} failed: stderr={}",
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

    // Pre-clone the bare repo so workarea create reuses it.
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
        repo_name,
        _bare: bare,
        _work: work,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn archive_workarea_default_keeps_worktree_on_disk() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let s = seed(&core, "keep").await;

    let mut wac = core.workareas_client().await.expect("workareas client");
    let wa = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: s.workspace_id.clone(),
            permission_mode: None,
        })
        .await
        .expect("CreateWorkarea")
        .into_inner();

    let worktree_root = core
        .data_dir
        .join("workspaces")
        .join(&s.workspace_slug)
        .join(&wa.composer_name);
    assert!(worktree_root.is_dir());

    // Archive with default opts (remove_worktree = false).
    wac.archive_workarea_with_opts(ArchiveWorkareaRequest {
        workarea_id: wa.id.clone(),
        remove_worktree: false,
    })
    .await
    .expect("ArchiveWorkareaWithOpts");

    let got = wac
        .get_workarea(WorkareaId {
            value: wa.id.clone(),
        })
        .await
        .expect("get")
        .into_inner();
    assert_eq!(got.status, "archived");
    assert!(got.archived_at.is_some());
    assert!(
        worktree_root.is_dir(),
        "default archive must keep worktree on disk; got missing: {}",
        worktree_root.display()
    );

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn archive_workarea_remove_worktree_blows_disk_away() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let s = seed(&core, "wipe").await;

    let mut wac = core.workareas_client().await.expect("workareas client");
    let wa = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: s.workspace_id.clone(),
            permission_mode: None,
        })
        .await
        .expect("CreateWorkarea")
        .into_inner();

    let worktree_root = core
        .data_dir
        .join("workspaces")
        .join(&s.workspace_slug)
        .join(&wa.composer_name);
    let repo_worktree = worktree_root.join(&s.repo_name);
    assert!(worktree_root.is_dir());
    assert!(repo_worktree.is_dir());

    wac.archive_workarea_with_opts(ArchiveWorkareaRequest {
        workarea_id: wa.id.clone(),
        remove_worktree: true,
    })
    .await
    .expect("ArchiveWorkareaWithOpts(remove)");

    assert!(
        !worktree_root.exists(),
        "remove_worktree=true must delete worktree_root; still exists: {}",
        worktree_root.display()
    );

    let got = wac
        .get_workarea(WorkareaId {
            value: wa.id.clone(),
        })
        .await
        .expect("get")
        .into_inner();
    assert_eq!(got.status, "archived");

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_workarea_recreates_worktree_and_resets_permission_mode() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let s = seed(&core, "restore").await;

    let mut wac = core.workareas_client().await.expect("workareas client");
    // Create with a non-default permission_mode so we can confirm the
    // reset to NULL on restore.
    let wa = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: s.workspace_id.clone(),
            permission_mode: Some(PermissionMode::Yolo as i32),
        })
        .await
        .expect("CreateWorkarea(yolo)")
        .into_inner();
    assert_eq!(
        wa.permission_mode,
        Some(PermissionMode::Yolo as i32),
        "freshly-created workarea should carry the requested permission mode"
    );

    let worktree_root = core
        .data_dir
        .join("workspaces")
        .join(&s.workspace_slug)
        .join(&wa.composer_name);

    // Archive with remove_worktree so restore must re-run worktree_add.
    wac.archive_workarea_with_opts(ArchiveWorkareaRequest {
        workarea_id: wa.id.clone(),
        remove_worktree: true,
    })
    .await
    .expect("archive");
    assert!(
        !worktree_root.exists(),
        "remove_worktree should delete root"
    );

    // Restore.
    let restored = wac
        .restore_workarea(WorkareaId {
            value: wa.id.clone(),
        })
        .await
        .expect("RestoreWorkarea")
        .into_inner();

    assert_eq!(restored.status, "active");
    assert!(restored.archived_at.is_none());
    // Permission mode must be reset to "inherit from workspace" per §3.7.
    assert!(
        restored.permission_mode.is_none(),
        "restored workarea must reset permission_mode to None; got {:?}",
        restored.permission_mode
    );
    // Worktree must be back on disk.
    assert!(
        worktree_root.is_dir(),
        "restore should re-create worktree_root: {}",
        worktree_root.display()
    );
    let repo_worktree = worktree_root.join(&s.repo_name);
    assert!(
        repo_worktree.join(".git").exists(),
        "restored repo worktree should have a .git pointer"
    );

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn archive_workspace_cascades_to_all_workareas() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let s = seed(&core, "cascade").await;

    let mut wac = core.workareas_client().await.expect("workareas client");
    // Create three workareas.
    let mut ids = Vec::new();
    for _ in 0..3 {
        let wa = wac
            .create_workarea(CreateWorkareaRequest {
                workspace_id: s.workspace_id.clone(),
                permission_mode: None,
            })
            .await
            .expect("CreateWorkarea")
            .into_inner();
        ids.push(wa.id);
    }

    // Archive the workspace; should cascade.
    let mut wsc = core.workspaces_client().await.expect("workspaces client");
    wsc.archive_workspace(WorkspaceId {
        value: s.workspace_id.clone(),
    })
    .await
    .expect("ArchiveWorkspace");

    // All three workareas must be archived.
    for id in &ids {
        let got = wac
            .get_workarea(WorkareaId { value: id.clone() })
            .await
            .expect("get workarea")
            .into_inner();
        assert_eq!(
            got.status, "archived",
            "workarea {id} should be cascaded to archived"
        );
        assert!(got.archived_at.is_some());
    }

    // Workspace itself archived.
    let ws = wsc
        .get_workspace(WorkspaceId {
            value: s.workspace_id.clone(),
        })
        .await
        .expect("GetWorkspace")
        .into_inner();
    assert!(ws.archived_at.is_some());

    // List with include_archived=false should be empty.
    let listed = wac
        .list_workareas(ListWorkareasRequest {
            workspace_id: s.workspace_id.clone(),
            include_archived: false,
        })
        .await
        .expect("list hidden")
        .into_inner();
    assert!(listed.workareas.is_empty());

    // Now restore the workspace; per §3.7 only the workspace timestamp
    // clears, workareas stay archived.
    let restored_ws = wsc
        .restore_workspace(WorkspaceId {
            value: s.workspace_id.clone(),
        })
        .await
        .expect("RestoreWorkspace")
        .into_inner();
    assert!(restored_ws.archived_at.is_none());
    for id in &ids {
        let got = wac
            .get_workarea(WorkareaId { value: id.clone() })
            .await
            .expect("get workarea post-restore-ws")
            .into_inner();
        assert_eq!(
            got.status, "archived",
            "workarea {id} should still be archived after workspace-only restore"
        );
    }

    core.shutdown().await.expect("shutdown");
}
