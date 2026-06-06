//! Integration test for the Task 20 `Workareas` gRPC service.
//!
//! Exercises the full path:
//! - spawn a real Core subprocess via the Task 17 harness
//! - seed `projects` + `repositories` + `workspaces` + `workspace_repos`
//!   directly (the cloned repo is a tiny local bare repo)
//! - call `Workareas.CreateWorkarea`; verify the on-disk layout
//!   `<data>/workspaces/<slug>/<composer>/{ .context/, <repo_name>/ }`,
//!   the DB row, and the worktree's `.git/info/exclude` carries
//!   `.context/`.
//! - second `CreateWorkarea` on the same workspace must get a different
//!   composer name (lowest-index unused).
//! - `ArchiveWorkarea`: status → `archived`, `archived_at` populated.

#![cfg(unix)]

use std::path::Path;

use concerto_proto::v1::{CreateWorkareaRequest, ListWorkareasRequest, WorkareaId};
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

/// Build a bare repo with one commit on `main` and return its file:// URL.
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

/// Seed `projects` + `repositories` + `workspaces` + `workspace_repos`
/// directly. Then clone the repo into `<data>/repos/<repo_id>/` so the
/// workarea-creation path finds an existing clone.
struct Seeded {
    project_id: String,
    workspace_id: String,
    workspace_slug: String,
    repo_id: String,
    repo_name: String,
    /// kept alive so the bare-repo tempdir isn't deleted under us
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

    // Clone the bare repo to <data>/repos/<repo_id>/ so the workarea
    // path doesn't have to drive RepoManager::clone_repo (which would
    // also work but is exercised by the Task 18 test).
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
        project_id,
        workspace_id,
        workspace_slug: slug.to_string(),
        repo_id,
        repo_name,
        _bare: bare,
        _work: work,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn create_workarea_lays_out_disk_and_db() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let s = seed(&core, "alpha").await;

    let mut wac = core.workareas_client().await.expect("workareas client");
    let wa = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: s.workspace_id.clone(),
            permission_mode: None,
        })
        .await
        .expect("CreateWorkarea")
        .into_inner();

    assert!(!wa.id.is_empty());
    assert_eq!(wa.workspace_id, s.workspace_id);
    assert!(!wa.composer_name.is_empty());
    assert_eq!(wa.branch_name, format!("concerto/{}", wa.composer_name));
    assert_eq!(wa.status, "active");
    assert!(wa.archived_at.is_none());

    // Disk layout.
    let worktree_root = core
        .data_dir
        .join("workspaces")
        .join(&s.workspace_slug)
        .join(&wa.composer_name);
    assert!(
        worktree_root.is_dir(),
        "worktree_root should exist: {}",
        worktree_root.display()
    );
    let context = worktree_root.join(".context");
    assert!(context.is_dir(), ".context/ should exist");
    assert!(context.join("PROMPT.md").is_file(), ".context/PROMPT.md");
    assert!(context.join("todos.md").is_file(), ".context/todos.md");
    assert!(context.join("scratch").is_dir(), ".context/scratch/");

    let repo_worktree = worktree_root.join(&s.repo_name);
    assert!(
        repo_worktree.is_dir(),
        "repo worktree should exist: {}",
        repo_worktree.display()
    );
    // The README.md from the seed commit must be visible in the
    // worktree.
    assert!(
        repo_worktree.join("README.md").is_file(),
        "worktree should have the committed README"
    );

    // `.context/` must be in the worktree's git exclude (a `git status`
    // in the worktree must report no `.context/` entry).
    let status = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&repo_worktree)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .await
        .expect("git status");
    assert!(
        status.status.success(),
        "git status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(
        !stdout.contains(".context"),
        "git status should not list .context/ paths; got:\n{stdout}"
    );

    // DB rows.
    let pool = core.db().await.expect("db");
    let (status_db,): (String,) = sqlx::query_as("SELECT status FROM workareas WHERE id = ?")
        .bind(&wa.id)
        .fetch_one(&pool)
        .await
        .expect("workareas row");
    assert_eq!(status_db, "active");
    let (jcount,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM workarea_repos WHERE workarea_id = ?")
            .bind(&wa.id)
            .fetch_one(&pool)
            .await
            .expect("workarea_repos count");
    assert_eq!(jcount, 1);

    // Get + List.
    let got = wac
        .get_workarea(WorkareaId {
            value: wa.id.clone(),
        })
        .await
        .expect("GetWorkarea")
        .into_inner();
    assert_eq!(got.id, wa.id);

    let listed = wac
        .list_workareas(ListWorkareasRequest {
            workspace_id: s.workspace_id.clone(),
            include_archived: false,
        })
        .await
        .expect("ListWorkareas")
        .into_inner();
    assert_eq!(listed.workareas.len(), 1);
    assert_eq!(listed.workareas[0].id, wa.id);

    core.shutdown().await.expect("shutdown");
}

/// Seed a 2-repo workspace (Task 306): one project, two repos cloned to
/// `<data>/repos/<repo_id>/`, one workspace with both repos attached at
/// `workspace_repos.position` 0 and 1.
struct SeededMulti {
    workspace_id: String,
    workspace_slug: String,
    repo_names: [String; 2],
    _bares: Vec<TempDir>,
    _works: Vec<TempDir>,
}

async fn seed_multi(core: &CoreUnderTest, slug: &str) -> SeededMulti {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    let project_id = format!("proj-{slug}");
    let workspace_id = format!("ws-{slug}");

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
        "INSERT INTO workspaces (id, project_id, name, slug, created_at) VALUES (?, ?, 'test', ?, 0)",
    )
    .bind(&workspace_id)
    .bind(&project_id)
    .bind(slug)
    .execute(&pool)
    .await
    .expect("insert workspace");

    let mut bares = Vec::new();
    let mut works = Vec::new();
    let mut repo_names = Vec::new();
    // Two repos: positions 0 (api) and 1 (web).
    for (position, short) in ["api", "web"].into_iter().enumerate() {
        let (bare_url, bare, work) = make_bare_with_commit().await;
        let repo_id = format!("repo-{slug}-{short}");
        let repo_name = format!("name-{slug}-{short}");
        let local_path = core.data_dir.join("repos").join(&repo_id);

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
            "INSERT INTO workspace_repos (workspace_id, repository_id, position) VALUES (?, ?, ?)",
        )
        .bind(&workspace_id)
        .bind(&repo_id)
        .bind(position as i64)
        .execute(&pool)
        .await
        .expect("insert workspace_repos");

        // Clone the bare repo so the workarea path reuses it.
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

        bares.push(bare);
        works.push(work);
        repo_names.push(repo_name);
    }
    pool.close().await;

    SeededMulti {
        workspace_id,
        workspace_slug: slug.to_string(),
        repo_names: [repo_names[0].clone(), repo_names[1].clone()],
        _bares: bares,
        _works: works,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn multi_repo_workarea_lays_out_one_worktree_per_repo() {
    // Task 306: a workarea on a 2-repo workspace materializes two
    // worktrees on disk and persists two `workarea_repos` rows.
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let s = seed_multi(&core, "multi").await;

    let mut wac = core.workareas_client().await.expect("workareas client");
    let wa = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: s.workspace_id.clone(),
            permission_mode: None,
        })
        .await
        .expect("CreateWorkarea")
        .into_inner();
    assert_eq!(wa.status, "active");

    let worktree_root = core
        .data_dir
        .join("workspaces")
        .join(&s.workspace_slug)
        .join(&wa.composer_name);
    // `.context/` once at the root.
    assert!(
        worktree_root.join(".context").is_dir(),
        ".context/ should exist once at the workarea root"
    );
    // One worktree per repo, each with the committed README.
    for repo_name in &s.repo_names {
        let repo_worktree = worktree_root.join(repo_name);
        assert!(
            repo_worktree.is_dir(),
            "repo worktree should exist: {}",
            repo_worktree.display()
        );
        assert!(
            repo_worktree.join("README.md").is_file(),
            "worktree {repo_name} should have the committed README"
        );
    }

    // Two `workarea_repos` rows.
    let pool = core.db().await.expect("db");
    let (jcount,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM workarea_repos WHERE workarea_id = ?")
            .bind(&wa.id)
            .fetch_one(&pool)
            .await
            .expect("workarea_repos count");
    assert_eq!(jcount, 2, "two workarea_repos rows for a 2-repo workspace");

    core.shutdown().await.expect("shutdown");
}

/// Task 307: seed a 2-repo workspace whose SECOND repo's `local_path`
/// points at a directory that is NOT a git repo, so `git worktree add`
/// fails for it while the first repo succeeds — driving the soft `partial`
/// create path.
async fn seed_multi_one_broken(core: &CoreUnderTest, slug: &str) -> SeededMulti {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    let project_id = format!("proj-{slug}");
    let workspace_id = format!("ws-{slug}");

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
        "INSERT INTO workspaces (id, project_id, name, slug, created_at) VALUES (?, ?, 'test', ?, 0)",
    )
    .bind(&workspace_id)
    .bind(&project_id)
    .bind(slug)
    .execute(&pool)
    .await
    .expect("insert workspace");

    let mut bares = Vec::new();
    let mut works = Vec::new();
    let mut repo_names = Vec::new();
    for (position, short) in ["good", "broken"].into_iter().enumerate() {
        let (bare_url, bare, work) = make_bare_with_commit().await;
        let repo_id = format!("repo-{slug}-{short}");
        let repo_name = format!("name-{slug}-{short}");
        let local_path = core.data_dir.join("repos").join(&repo_id);

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
            "INSERT INTO workspace_repos (workspace_id, repository_id, position) VALUES (?, ?, ?)",
        )
        .bind(&workspace_id)
        .bind(&repo_id)
        .bind(position as i64)
        .execute(&pool)
        .await
        .expect("insert workspace_repos");

        if position == 0 {
            // First repo: a real clone so its worktree-add succeeds.
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
            assert!(out.status.success(), "good clone failed");
        } else {
            // Second repo: a NON-git directory (it has a HEAD file so the
            // create path's `clone_repo` short-circuits, but `git worktree
            // add` against it fails — driving `partial`).
            tokio::fs::create_dir_all(&local_path).await.unwrap();
            tokio::fs::write(local_path.join("HEAD"), b"not a git repo\n")
                .await
                .unwrap();
        }

        bares.push(bare);
        works.push(work);
        repo_names.push(repo_name);
    }
    pool.close().await;

    SeededMulti {
        workspace_id,
        workspace_slug: slug.to_string(),
        repo_names: [repo_names[0].clone(), repo_names[1].clone()],
        _bares: bares,
        _works: works,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn partial_create_when_one_repo_worktree_add_fails() {
    // Task 307: a 2-repo workarea where the second repo's worktree-add
    // fails persists as `partial` (not aborted), keeps the first repo's
    // worktree + junction row, and the second repo gets no junction row.
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let s = seed_multi_one_broken(&core, "partial").await;

    let mut wac = core.workareas_client().await.expect("workareas client");
    let wa = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: s.workspace_id.clone(),
            permission_mode: None,
        })
        .await
        .expect("CreateWorkarea must succeed (soft partial, not abort)")
        .into_inner();

    assert_eq!(
        wa.status, "partial",
        "≥1 failed worktree-add must yield a `partial` workarea, got {:?}",
        wa.status
    );

    // The good repo's worktree exists; the broken one does not.
    let worktree_root = core
        .data_dir
        .join("workspaces")
        .join(&s.workspace_slug)
        .join(&wa.composer_name);
    assert!(
        worktree_root.join(&s.repo_names[0]).is_dir(),
        "the successful repo's worktree must exist"
    );
    assert!(
        !worktree_root.join(&s.repo_names[1]).join(".git").exists(),
        "the failed repo must not have a materialized worktree"
    );

    // Exactly one `workarea_repos` row (only the materialized repo).
    let pool = core.db().await.expect("db");
    let (jcount,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM workarea_repos WHERE workarea_id = ?")
            .bind(&wa.id)
            .fetch_one(&pool)
            .await
            .expect("workarea_repos count");
    assert_eq!(
        jcount, 1,
        "only the materialized repo gets a workarea_repos row in a partial create"
    );

    // The DB status is persisted as `partial` (proves migration 0010's
    // widened CHECK accepts it end-to-end through the real Core).
    let (status_db,): (String,) = sqlx::query_as("SELECT status FROM workareas WHERE id = ?")
        .bind(&wa.id)
        .fetch_one(&pool)
        .await
        .expect("workareas row");
    assert_eq!(status_db, "partial");

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn second_workarea_gets_different_composer() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let s = seed(&core, "beta").await;

    let mut wac = core.workareas_client().await.expect("workareas client");
    let first = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: s.workspace_id.clone(),
            permission_mode: None,
        })
        .await
        .expect("first create")
        .into_inner();
    let second = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: s.workspace_id.clone(),
            permission_mode: None,
        })
        .await
        .expect("second create")
        .into_inner();
    assert_ne!(
        first.composer_name, second.composer_name,
        "second workarea must get a different composer name"
    );
    // The first one in the pool is `bach` per the locked order, second
    // should be `handel` (the next in `COMPOSERS`).
    assert_eq!(first.composer_name, "bach");
    assert_eq!(second.composer_name, "handel");

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn archive_sets_status_and_timestamp() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let s = seed(&core, "gamma").await;

    let mut wac = core.workareas_client().await.expect("workareas client");
    let wa = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: s.workspace_id.clone(),
            permission_mode: None,
        })
        .await
        .expect("CreateWorkarea")
        .into_inner();

    wac.archive_workarea(WorkareaId {
        value: wa.id.clone(),
    })
    .await
    .expect("ArchiveWorkarea");

    let got = wac
        .get_workarea(WorkareaId {
            value: wa.id.clone(),
        })
        .await
        .expect("GetWorkarea after archive")
        .into_inner();
    assert_eq!(got.status, "archived");
    assert!(
        got.archived_at.is_some(),
        "archived_at should be populated after archive"
    );

    // List with include_archived=false should hide it; =true should
    // include it.
    let hidden = wac
        .list_workareas(ListWorkareasRequest {
            workspace_id: s.workspace_id.clone(),
            include_archived: false,
        })
        .await
        .expect("list hidden")
        .into_inner();
    assert!(
        hidden.workareas.is_empty(),
        "archived rows hidden by default"
    );
    let shown = wac
        .list_workareas(ListWorkareasRequest {
            workspace_id: s.workspace_id.clone(),
            include_archived: true,
        })
        .await
        .expect("list shown")
        .into_inner();
    assert_eq!(shown.workareas.len(), 1);

    // Suppress dead-code warning on unused fields of `Seeded`.
    let _ = (&s.project_id, &s.repo_id);

    core.shutdown().await.expect("shutdown");
}
