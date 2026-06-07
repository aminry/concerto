//! Integration test for the Task 30 files-to-copy resolver.
//!
//! Exercises the end-to-end happy path:
//!
//! 1. Set up a fixture project repo at `<repo.local_path>` with:
//!    - `.concerto/.worktreeinclude` containing one rule of each mode
//!      (copy, symlink, exclude).
//!    - the matching files (`.env`, `.env.local`, `.env.production`).
//! 2. Seed `projects` + `repositories` + `workspaces` + `workspace_repos`
//!    rows directly (mirrors the Task 20 lifecycle test pattern).
//! 3. Call `Workareas.CreateWorkarea`.
//! 4. Assert the workarea's repo subdir has:
//!    - `.env`              → copy (regular file with seed bytes)
//!    - `.env.local`        → symlink (target relative, follows to seed bytes)
//!    - `.env.production`   → NOT present (excluded)
//! 5. Assert `workareas.settings_json` carries
//!    `"files_to_copy_applied": true`.
//!
//! Also covers escape rejection: a `.worktreeinclude` rule pointing at a
//! path whose source escapes the project root via a symlink is rejected
//! at create time, and no row is committed.

#![cfg(unix)]

use std::path::Path;

use concerto_proto::v1::CreateWorkareaRequest;
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

struct Seeded {
    workspace_id: String,
    workspace_slug: String,
    repo_name: String,
    repo_local_path: std::path::PathBuf,
    _bare: TempDir,
    _work: TempDir,
}

/// Seed projects / repositories / workspaces / workspace_repos and
/// clone the bare repo into `<data>/repos/<repo_id>/`. Returns the
/// disk path of the clone so the test can drop the
/// `.concerto/.worktreeinclude` and the matched files into it.
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
    // path finds an existing clone.
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
        repo_local_path: local_path,
        _bare: bare,
        _work: work,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn create_workarea_applies_files_to_copy_rules() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let s = seed(&core, "ftc-happy").await;

    // Drop `.concerto/.worktreeinclude` + the matching files into the
    // reference repo (the workspace's only repo's local_path).
    let concerto_dir = s.repo_local_path.join(".concerto");
    tokio::fs::create_dir_all(&concerto_dir).await.unwrap();
    tokio::fs::write(
        concerto_dir.join(".worktreeinclude"),
        ".env\n.env.local !\n!.env.production\n",
    )
    .await
    .unwrap();
    tokio::fs::write(s.repo_local_path.join(".env"), b"KEY=copy\n")
        .await
        .unwrap();
    tokio::fs::write(s.repo_local_path.join(".env.local"), b"KEY=symlink\n")
        .await
        .unwrap();
    tokio::fs::write(s.repo_local_path.join(".env.production"), b"KEY=excluded\n")
        .await
        .unwrap();

    let mut wac = core.workareas_client().await.expect("workareas client");
    let wa = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: s.workspace_id.clone(),
            permission_mode: None,
        })
        .await
        .expect("CreateWorkarea")
        .into_inner();

    let repo_worktree = core
        .data_dir
        .join("workspaces")
        .join(&s.workspace_slug)
        .join(&wa.composer_name)
        .join(&s.repo_name);

    // `.env` → copy (regular file with seed bytes).
    let copied = repo_worktree.join(".env");
    let md = std::fs::symlink_metadata(&copied).expect(".env should exist");
    assert!(md.is_file(), ".env should be a regular file (copy mode)");
    let bytes = tokio::fs::read(&copied).await.unwrap();
    assert_eq!(bytes, b"KEY=copy\n");

    // `.env.local` → symlink with relative target, dereferencing to
    // the source bytes.
    let linked = repo_worktree.join(".env.local");
    let md = std::fs::symlink_metadata(&linked).expect(".env.local should exist");
    assert!(
        md.file_type().is_symlink(),
        ".env.local should be a symlink (symlink mode)"
    );
    let target = std::fs::read_link(&linked).unwrap();
    assert!(
        target.is_relative(),
        ".env.local symlink target must be relative, got {target:?}"
    );
    let bytes = tokio::fs::read(&linked).await.unwrap();
    assert_eq!(bytes, b"KEY=symlink\n");

    // `.env.production` → excluded.
    assert!(
        !repo_worktree.join(".env.production").exists(),
        ".env.production should be excluded"
    );

    // `workareas.settings_json` should carry `files_to_copy_applied: true`.
    let pool = core.db().await.expect("db");
    let (settings_json,): (String,) =
        sqlx::query_as("SELECT settings_json FROM workareas WHERE id = ?")
            .bind(&wa.id)
            .fetch_one(&pool)
            .await
            .expect("settings_json");
    assert!(
        settings_json.contains("\"files_to_copy_applied\""),
        "settings_json should record the flag; got {settings_json}"
    );
    assert!(
        settings_json.contains("true"),
        "files_to_copy_applied should be true; got {settings_json}"
    );

    core.shutdown().await.expect("shutdown");
}

/// Attach a SECOND repo to an existing seeded workspace at `position = 1`
/// (a non-reference repo). Returns its `name` + clone path so the test can
/// assert the reference-worktree files land in this repo's worktree too.
async fn attach_second_repo(
    core: &CoreUnderTest,
    s: &Seeded,
    slug: &str,
) -> (String, std::path::PathBuf) {
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

    let (bare_url, _bare, _work) = make_bare_with_commit().await;
    // Leak the temp dirs so the bare/work survive for the test duration.
    std::mem::forget(_bare);
    std::mem::forget(_work);

    let project_id = format!("proj-{slug}");
    let repo_id = format!("repo2-{slug}");
    let repo_name = format!("name2-{slug}");
    let local_path = core.data_dir.join("repos").join(&repo_id);

    let opts = SqliteConnectOptions::new()
        .filename(&core.db_path)
        .create_if_missing(false);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("open db");
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
    .expect("insert repo2");
    sqlx::query(
        "INSERT INTO workspace_repos (workspace_id, repository_id, position) VALUES (?, ?, 1)",
    )
    .bind(&s.workspace_id)
    .bind(&repo_id)
    .execute(&pool)
    .await
    .expect("insert workspace_repos pos 1");
    pool.close().await;

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
        .expect("git clone repo2");
    assert!(out.status.success(), "repo2 clone failed");

    (repo_name, local_path)
}

#[tokio::test(flavor = "multi_thread")]
async fn create_workarea_reference_worktree_lands_in_all_repos() {
    // Task 309: the reference worktree = the first repo by
    // `workspace_repos.position`. A `.worktreeinclude` + `.env` in the
    // reference repo must materialize the `.env` in EVERY repo's worktree.
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let s = seed(&core, "ftc-multi").await;
    let (repo2_name, _repo2_local) = attach_second_repo(&core, &s, "ftc-multi").await;

    // Reference repo (position 0) carries the rules + the source file.
    let concerto_dir = s.repo_local_path.join(".concerto");
    tokio::fs::create_dir_all(&concerto_dir).await.unwrap();
    tokio::fs::write(concerto_dir.join(".worktreeinclude"), ".env\n")
        .await
        .unwrap();
    tokio::fs::write(s.repo_local_path.join(".env"), b"SHARED=1\n")
        .await
        .unwrap();

    let mut wac = core.workareas_client().await.expect("workareas client");
    let wa = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: s.workspace_id.clone(),
            permission_mode: None,
        })
        .await
        .expect("CreateWorkarea")
        .into_inner();

    let base = core
        .data_dir
        .join("workspaces")
        .join(&s.workspace_slug)
        .join(&wa.composer_name);

    // Reference repo's worktree.
    let ref_env = base.join(&s.repo_name).join(".env");
    assert_eq!(
        tokio::fs::read(&ref_env).await.unwrap(),
        b"SHARED=1\n",
        "reference repo gets its own .env"
    );
    // Non-reference repo's worktree ALSO gets the reference-worktree .env.
    let other_env = base.join(&repo2_name).join(".env");
    assert_eq!(
        tokio::fs::read(&other_env).await.unwrap(),
        b"SHARED=1\n",
        "reference-worktree .env lands in the second repo's worktree"
    );

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn create_workarea_checked_in_worktreeinclude_wins_over_db_rules() {
    // Task 309 / design §3.13: a checked-in `.worktreeinclude` WINS over the
    // local-DB `files_to_copy_rules`. Seed BOTH: the DB rules would copy
    // `db_only.txt`; the checked-in file copies `checked_in.txt` only.
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let s = seed(&core, "ftc-precedence").await;

    // Local-DB rules (would copy db_only.txt).
    {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        let opts = SqliteConnectOptions::new()
            .filename(&core.db_path)
            .create_if_missing(false);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .expect("db");
        sqlx::query("UPDATE projects SET settings_json = ? WHERE id = ?")
            .bind(r#"{"files_to_copy_rules":[{"pattern":"db_only.txt","mode":"copy"}]}"#)
            .bind(format!("proj-{}", "ftc-precedence"))
            .execute(&pool)
            .await
            .expect("update settings_json");
        pool.close().await;
    }

    // Checked-in file (copies checked_in.txt only) — should WIN.
    let concerto_dir = s.repo_local_path.join(".concerto");
    tokio::fs::create_dir_all(&concerto_dir).await.unwrap();
    tokio::fs::write(concerto_dir.join(".worktreeinclude"), "checked_in.txt\n")
        .await
        .unwrap();
    tokio::fs::write(s.repo_local_path.join("checked_in.txt"), b"ci\n")
        .await
        .unwrap();
    tokio::fs::write(s.repo_local_path.join("db_only.txt"), b"db\n")
        .await
        .unwrap();

    let mut wac = core.workareas_client().await.expect("workareas client");
    let wa = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: s.workspace_id.clone(),
            permission_mode: None,
        })
        .await
        .expect("CreateWorkarea")
        .into_inner();

    let repo_worktree = core
        .data_dir
        .join("workspaces")
        .join(&s.workspace_slug)
        .join(&wa.composer_name)
        .join(&s.repo_name);

    assert!(
        repo_worktree.join("checked_in.txt").is_file(),
        "checked-in .worktreeinclude rule applied"
    );
    assert!(
        !repo_worktree.join("db_only.txt").exists(),
        "DB rule must NOT apply — checked-in file wins"
    );

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn create_workarea_uses_db_rules_when_no_checked_in_file() {
    // Task 309: with NO checked-in `.worktreeinclude`, the local-DB
    // `files_to_copy_rules` (via the Task-310 resolver) are the rule set.
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let s = seed(&core, "ftc-dbonly").await;

    {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        let opts = SqliteConnectOptions::new()
            .filename(&core.db_path)
            .create_if_missing(false);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .expect("db");
        sqlx::query("UPDATE projects SET settings_json = ? WHERE id = ?")
            .bind(r#"{"files_to_copy_rules":[{"pattern":".env","mode":"copy"}]}"#)
            .bind(format!("proj-{}", "ftc-dbonly"))
            .execute(&pool)
            .await
            .expect("update settings_json");
        pool.close().await;
    }
    // No `.concerto/.worktreeinclude`.
    tokio::fs::write(s.repo_local_path.join(".env"), b"FROMDB=1\n")
        .await
        .unwrap();

    let mut wac = core.workareas_client().await.expect("workareas client");
    let wa = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: s.workspace_id.clone(),
            permission_mode: None,
        })
        .await
        .expect("CreateWorkarea")
        .into_inner();

    let env = core
        .data_dir
        .join("workspaces")
        .join(&s.workspace_slug)
        .join(&wa.composer_name)
        .join(&s.repo_name)
        .join(".env");
    assert_eq!(
        tokio::fs::read(&env).await.unwrap(),
        b"FROMDB=1\n",
        "DB rules applied when no checked-in file"
    );

    core.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn create_workarea_rejects_path_escape_via_symlink() {
    let core = CoreUnderTest::spawn().await.expect("spawn core");
    let s = seed(&core, "ftc-escape").await;

    // Create an outside-tree file then a symlink at `<repo>/.env`
    // pointing at it. The `.worktreeinclude` matches `.env`, so the
    // resolver must reject the source-side escape.
    let outside = TempDir::new().unwrap();
    tokio::fs::write(outside.path().join("secret"), b"nope\n")
        .await
        .unwrap();
    let concerto_dir = s.repo_local_path.join(".concerto");
    tokio::fs::create_dir_all(&concerto_dir).await.unwrap();
    tokio::fs::write(concerto_dir.join(".worktreeinclude"), ".env\n")
        .await
        .unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret"),
        s.repo_local_path.join(".env"),
    )
    .unwrap();

    let mut wac = core.workareas_client().await.expect("workareas client");
    let err = wac
        .create_workarea(CreateWorkareaRequest {
            workspace_id: s.workspace_id.clone(),
            permission_mode: None,
        })
        .await
        .expect_err("create should fail on escape");
    let msg = err.message();
    assert!(
        msg.contains("file_to_copy.escapes_project_root"),
        "unexpected error: {msg}"
    );

    core.shutdown().await.expect("shutdown");
}
