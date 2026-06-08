//! Integration coverage for the browsable-repo-tree + per-repo default-cone
//! feature (design/02 §3.2):
//!
//! 1. `RepoManager::list_tree` lists the IMMEDIATE children of a directory at
//!    the repo's default ref — trees-first, full repo-root-relative paths,
//!    non-recursive.
//! 2. `RepoManager::set_repo_cone_defaults` persists the cone to
//!    `repositories.cone_defaults_json` AND propagates it to every existing
//!    (non-archived) workarea worktree of the repo, returning the count
//!    re-applied. Archived workareas are excluded.
//! 3. An invalid cone path (absent from the repo index) is rejected BEFORE
//!    anything is persisted (the default is left unchanged, nothing
//!    half-applied).
//!
//! In-process against a tempdir SQLite DB + a `file://` bare-repo fixture —
//! no network, no gRPC. Mirrors `cone_stats.rs`'s `RepoManager` harness.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use concerto_core::repo_manager::RepoManager;
use concerto_gix_wrap::{self as gixw, CloneStrategy, ConePath};
use concerto_persist::{
    NewWorkarea, NewWorkareaRepo, NewWorkspace, Persistence, PersistenceConfig, RepositoryId,
    WorkareaId, WorkspaceId,
};
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

/// Build a bare repo whose `main` carries a known nested tree:
///   README.md
///   a/f1.txt  a/f2.txt
///   a/sub/deep.txt        (nested dir under a/)
///   b/g1.txt
/// Returns its `file://` URL.
async fn make_bare_with_tree() -> (String, TempDir, TempDir) {
    make_bare_with_tree_on("main").await
}

/// Same fixture as [`make_bare_with_tree`] but on an explicit branch name, so
/// a test can clone a repo whose real branch differs from the `"main"`
/// fallback `default_branch` stored at add-time (the `list_tree` HEAD case).
async fn make_bare_with_tree_on(branch: &str) -> (String, TempDir, TempDir) {
    let bare = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    git(&["init", "--bare", "-b", branch, "."], bare.path()).await;
    git(&["init", "-b", branch, "."], work.path()).await;

    let w = work.path();
    tokio::fs::write(w.join("README.md"), "top\n")
        .await
        .unwrap();
    tokio::fs::create_dir_all(w.join("a/sub")).await.unwrap();
    tokio::fs::write(w.join("a/f1.txt"), "a1\n").await.unwrap();
    tokio::fs::write(w.join("a/f2.txt"), "a2\n").await.unwrap();
    tokio::fs::write(w.join("a/sub/deep.txt"), "deep\n")
        .await
        .unwrap();
    tokio::fs::create_dir_all(w.join("b")).await.unwrap();
    tokio::fs::write(w.join("b/g1.txt"), "b1\n").await.unwrap();

    git(&["add", "-A"], w).await;
    git(&["commit", "-m", "tree"], w).await;
    git(
        &[
            "remote",
            "add",
            "origin",
            &format!("file://{}", bare.path().display()),
        ],
        w,
    )
    .await;
    git(&["push", "-u", "origin", branch], w).await;
    (format!("file://{}", bare.path().display()), bare, work)
}

async fn make_repo_manager(project_id: &str) -> (Arc<Persistence>, RepoManager, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("concerto.db");
    let persistence = Persistence::open(PersistenceConfig {
        db_path,
        max_readers: 2,
    })
    .await
    .expect("open persistence");
    let persistence = Arc::new(persistence);
    {
        let mut writer = persistence.writer().await;
        sqlx::query("INSERT INTO projects (id, name, created_at) VALUES (?, 'test', 0)")
            .bind(project_id)
            .execute(&mut *writer)
            .await
            .expect("insert project");
    }
    let repos_root = tmp.path().join("repos");
    let manager = RepoManager::new(Arc::clone(&persistence), repos_root);
    (persistence, manager, tmp)
}

/// Clone the fixture (Full) into the manager's repos root. Returns the repo
/// id + its on-disk clone path.
async fn clone_fixture(
    manager: &RepoManager,
    project_id: &str,
    url: &str,
) -> (RepositoryId, PathBuf) {
    let repo = manager
        .add_repository(
            project_id,
            "fixture",
            url,
            "main",
            CloneStrategy::Full,
            false,
        )
        .await
        .expect("add_repository");
    manager
        .clone_repo(&repo.id, None)
        .await
        .expect("clone_repo");
    let local = PathBuf::from(&repo.local_path);
    (repo.id, local)
}

/// Create a workspace + workarea + a `workarea_repos` row whose worktree is a
/// real `git worktree add`-ed checkout of `clone_dir`, cone-initialized so the
/// propagation primitive can re-apply a cone to it. Returns the worktree path.
#[allow(clippy::too_many_arguments)]
async fn add_workarea_with_worktree(
    persist: &Arc<Persistence>,
    project_id: &str,
    clone_dir: &Path,
    worktrees_root: &Path,
    repo_id: &RepositoryId,
    composer: &str,
    archived: bool,
) -> PathBuf {
    let workspace_id = WorkspaceId(format!("ws-{}", uuid::Uuid::now_v7()));
    let workarea_id = WorkareaId(format!("wa-{}", uuid::Uuid::now_v7()));
    let branch = format!("concerto/{composer}");
    let worktree = worktrees_root.join(composer);

    // Real worktree on its own branch, then cone-init it (init --cone
    // --sparse-index) so set_workarea_repo_cones's sparse_set lands cleanly.
    gixw::worktree_add(clone_dir, &branch, &worktree)
        .await
        .expect("worktree_add");
    gixw::sparse_init_cone(&worktree)
        .await
        .expect("sparse_init_cone");

    let mut writer = persist.writer().await;
    concerto_persist::workspaces::insert(
        &mut writer,
        NewWorkspace {
            id: workspace_id.clone(),
            project_id: project_id.to_string(),
            name: format!("ws-{composer}"),
            slug: format!("ws-{composer}"),
            description: None,
            permission_mode: None,
            created_at: 1,
        },
    )
    .await
    .unwrap();
    concerto_persist::workareas::insert(
        &mut writer,
        NewWorkarea {
            id: workarea_id.clone(),
            workspace_id: workspace_id.0.clone(),
            composer_name: composer.into(),
            branch_name: branch,
            worktree_root: worktree.to_string_lossy().into_owned(),
            status: "active".into(),
            permission_mode: None,
            created_at: 1,
        },
    )
    .await
    .unwrap();
    concerto_persist::workareas::insert_workarea_repo(
        &mut writer,
        NewWorkareaRepo {
            workarea_id: workarea_id.clone(),
            repository_id: repo_id.clone(),
            worktree_path: worktree.to_string_lossy().into_owned(),
            branch_override: None,
            sparse_cones_json: NewWorkareaRepo::empty_cones(),
        },
    )
    .await
    .unwrap();
    if archived {
        concerto_persist::workareas::archive(&mut writer, &workarea_id, 2)
            .await
            .unwrap();
    }
    worktree
}

#[tokio::test(flavor = "multi_thread")]
async fn list_tree_lists_immediate_children_trees_first() {
    let (_p, manager, _tmp) = make_repo_manager("p-tree").await;
    let (url, _bare, _work) = make_bare_with_tree().await;
    let (repo_id, _local) = clone_fixture(&manager, "p-tree", &url).await;

    // Root listing (empty path + empty ref → HEAD).
    let root = manager
        .list_tree(&repo_id, "", "")
        .await
        .expect("list_tree");
    let root_view: Vec<(&str, bool)> = root.iter().map(|e| (e.path.as_str(), e.is_dir)).collect();
    assert_eq!(
        root_view,
        vec![("a", true), ("b", true), ("README.md", false)],
        "root: trees-first, alphabetical, full paths"
    );
    // The basename is the trailing segment.
    assert_eq!(root[0].name, "a");

    // Nested listing of `a` → immediate children with full paths,
    // non-recursive (a/sub is a dir; a/sub/deep.txt is NOT listed here).
    let a = manager
        .list_tree(&repo_id, "", "a")
        .await
        .expect("list_tree a");
    let a_view: Vec<(&str, bool)> = a.iter().map(|e| (e.path.as_str(), e.is_dir)).collect();
    assert_eq!(
        a_view,
        vec![("a/sub", true), ("a/f1.txt", false), ("a/f2.txt", false)],
        "nested listing is non-recursive with full paths"
    );
    assert_eq!(a[0].name, "sub", "basename of a/sub is `sub`");
}

/// Regression: an empty wire ref must resolve to `HEAD`, NOT the stored
/// `default_branch`. `AddRepository` stores the literal `"main"` fallback when
/// the caller leaves the branch blank, but the real branch here is `master`,
/// so `git ls-tree main` would fail ("Not a valid object name main"). `HEAD`
/// is a symref to the actually-cloned branch and resolves regardless.
#[tokio::test(flavor = "multi_thread")]
async fn list_tree_uses_head_not_stored_default_branch() {
    let (_p, manager, _tmp) = make_repo_manager("p-head").await;
    // Real branch is `master`, but we add the repo with the `"main"` fallback.
    let (url, _bare, _work) = make_bare_with_tree_on("master").await;
    let repo = manager
        .add_repository(
            "p-head",
            "fixture",
            &url,
            "main",
            CloneStrategy::Full,
            false,
        )
        .await
        .expect("add_repository");
    manager
        .clone_repo(&repo.id, None)
        .await
        .expect("clone_repo");
    assert_eq!(
        repo.default_branch, "main",
        "stored fallback is the wrong ref"
    );

    let root = manager
        .list_tree(&repo.id, "", "")
        .await
        .expect("list_tree must resolve HEAD, not the stored `main`");
    let names: Vec<&str> = root.iter().map(|e| e.path.as_str()).collect();
    assert_eq!(names, vec!["a", "b", "README.md"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn set_repo_cone_defaults_persists_and_propagates() {
    let (persist, manager, tmp) = make_repo_manager("p-prop").await;
    let (url, _bare, _work) = make_bare_with_tree().await;
    let (repo_id, clone_dir) = clone_fixture(&manager, "p-prop", &url).await;

    let worktrees = tmp.path().join("worktrees");
    let wt_bach = add_workarea_with_worktree(
        &persist, "p-prop", &clone_dir, &worktrees, &repo_id, "bach", false,
    )
    .await;
    let wt_byrd = add_workarea_with_worktree(
        &persist, "p-prop", &clone_dir, &worktrees, &repo_id, "byrd", false,
    )
    .await;
    // An archived workarea must NOT be propagated to / counted.
    let _wt_arch = add_workarea_with_worktree(
        &persist, "p-prop", &clone_dir, &worktrees, &repo_id, "arch", true,
    )
    .await;

    // Set the repo default cone to `a`.
    let updated = manager
        .set_repo_cone_defaults(&repo_id, &["a".to_string()])
        .await
        .expect("set_repo_cone_defaults");
    assert_eq!(
        updated, 2,
        "exactly the two non-archived workareas should be re-applied"
    );

    // The repo-level default is persisted as the FROZEN flat JSON array.
    let row = concerto_persist::repositories::get(persist.readers(), &repo_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.cone_defaults_json, r#"["a"]"#);

    // Each non-archived worktree materialized cone `a` (a/f1.txt present,
    // b/g1.txt out-of-cone). The propagation applied the cone on disk.
    for wt in [&wt_bach, &wt_byrd] {
        assert!(
            wt.join("a/f1.txt").exists(),
            "in-cone file should be materialized at {}",
            wt.display()
        );
        assert!(
            !wt.join("b/g1.txt").exists(),
            "out-of-cone file should be collapsed at {}",
            wt.display()
        );
    }

    // The per-workarea sparse_cones_json was persisted too (propagation goes
    // through set_workarea_repo_cones, which writes the junction row).
    let cones: Vec<ConePath> = {
        // Re-read one workarea's junction cone via the resolver-facing reader.
        let raw = concerto_persist::workareas::list_workareas_for_repo(persist.readers(), &repo_id)
            .await
            .unwrap();
        assert_eq!(
            raw.len(),
            2,
            "archived workarea excluded from the repo list"
        );
        let json = concerto_persist::workareas::get_workarea_repo_cones(
            persist.readers(),
            &raw[0],
            &repo_id,
        )
        .await
        .unwrap()
        .unwrap();
        serde_json::from_str(&json).unwrap()
    };
    assert_eq!(cones, vec!["a".to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn set_repo_cone_defaults_rejects_invalid_path_before_persist() {
    let (persist, manager, _tmp) = make_repo_manager("p-bad").await;
    let (url, _bare, _work) = make_bare_with_tree().await;
    let (repo_id, _clone_dir) = clone_fixture(&manager, "p-bad", &url).await;

    // First seed a valid default so we can prove the failed call leaves it
    // unchanged (nothing half-applied).
    manager
        .set_repo_cone_defaults(&repo_id, &["a".to_string()])
        .await
        .expect("seed valid default");

    // A path absent from the repo index → Err, BEFORE any persist.
    let err = manager
        .set_repo_cone_defaults(&repo_id, &["does/not/exist".to_string()])
        .await;
    assert!(err.is_err(), "an invalid cone path must be rejected");

    // The previously-persisted default is untouched.
    let row = concerto_persist::repositories::get(persist.readers(), &repo_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.cone_defaults_json, r#"["a"]"#,
        "a rejected set must not overwrite the existing default"
    );
}
