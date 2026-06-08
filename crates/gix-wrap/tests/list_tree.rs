//! `list_tree` tests for `concerto-gix-wrap` (design/02 §3.2).
//!
//! Backs the browsable repo-tree picker: lists the immediate (non-recursive)
//! children of a directory at a ref. Built against an in-test repo with a
//! known nested tree — no network.

use std::path::Path;

use concerto_gix_wrap::list_tree;
use tempfile::TempDir;
use tokio::process::Command;

/// Spawn `git` synchronously inside a tempdir; panic on failure. Hermetic.
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
        "git {:?} failed: stdout={} stderr={}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// Build a repo with a nested tree:
///   README.md            (top-level file)
///   src/lib.rs
///   src/api/mod.rs       (nested dir under src/)
///   docs/guide.md
async fn make_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    let p = dir.path();
    git(&["init", "-b", "main", "."], p).await;
    tokio::fs::write(p.join("README.md"), "hi\n").await.unwrap();
    tokio::fs::create_dir_all(p.join("src/api")).await.unwrap();
    tokio::fs::write(p.join("src/lib.rs"), "//lib\n")
        .await
        .unwrap();
    tokio::fs::write(p.join("src/api/mod.rs"), "//mod\n")
        .await
        .unwrap();
    tokio::fs::create_dir_all(p.join("docs")).await.unwrap();
    tokio::fs::write(p.join("docs/guide.md"), "doc\n")
        .await
        .unwrap();
    git(&["add", "-A"], p).await;
    git(&["commit", "-m", "initial"], p).await;
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn lists_root_children_trees_first() {
    let repo = make_repo().await;
    // Empty path + empty ref (→ HEAD) lists the root's immediate children.
    let entries = list_tree(repo.path(), "", "").await.expect("list_tree");
    // docs/ + src/ are trees (alphabetical), then README.md (a blob).
    assert_eq!(
        entries,
        vec![
            ("docs".to_string(), true),
            ("src".to_string(), true),
            ("README.md".to_string(), false),
        ],
        "root listing should be trees-first, alphabetical, full paths"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn lists_nested_dir_children_with_full_paths() {
    let repo = make_repo().await;
    // Listing `src` returns its immediate children with FULL repo-relative
    // paths (`src/api`, `src/lib.rs`) — not basenames, not recursive.
    let entries = list_tree(repo.path(), "HEAD", "src")
        .await
        .expect("list_tree src");
    assert_eq!(
        entries,
        vec![
            ("src/api".to_string(), true),
            ("src/lib.rs".to_string(), false),
        ],
        "nested listing should carry full paths, trees-first, non-recursive"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn trailing_and_leading_slashes_are_tolerated() {
    let repo = make_repo().await;
    let entries = list_tree(repo.path(), "", "/src/")
        .await
        .expect("list_tree /src/");
    assert_eq!(
        entries,
        vec![
            ("src/api".to_string(), true),
            ("src/lib.rs".to_string(), false),
        ],
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_ref_is_an_error() {
    let repo = make_repo().await;
    let err = list_tree(repo.path(), "no-such-ref", "").await;
    assert!(err.is_err(), "an unknown ref should surface as an error");
}
