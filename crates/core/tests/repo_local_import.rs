//! Integration test for `RepoManager::import_local` (D9, non-destructive
//! adopt-in-place).
//!
//! In-process against a tempdir SQLite DB + a freshly `git init`-ed working
//! repo — no network, no gRPC. Mirrors `repo_size_estimate.rs`'s
//! `RepoManager` harness.
//!
//! Asserts:
//! - the adopted `Repository.local_path` is the ORIGINAL temp path (NOT
//!   relocated under `<repos_root>/<id>/`);
//! - the row is discoverable via `repositories::get_by_url` / `list_all`;
//! - the original `.git` is untouched and the seed commit still resolves
//!   (proving non-destructiveness);
//! - importing the same path/url twice de-dups (returns the existing row,
//!   no duplicate registry entry).

#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;

use concerto_core::repo_manager::RepoManager;
use concerto_persist::{Persistence, PersistenceConfig};
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

/// Run `git` and capture trimmed stdout (for `rev-parse HEAD` etc.).
async fn git_capture(args: &[&str], cwd: &Path) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// `git init` a working repo with one commit. Returns the tempdir.
async fn make_local_repo_with_commit() -> TempDir {
    let work = TempDir::new().unwrap();
    git(&["init", "-b", "main", "."], work.path()).await;
    git(&["config", "user.email", "test@example.com"], work.path()).await;
    git(&["config", "user.name", "test"], work.path()).await;
    tokio::fs::write(work.path().join("README.md"), "hello\n")
        .await
        .unwrap();
    git(&["add", "README.md"], work.path()).await;
    git(&["commit", "-m", "initial"], work.path()).await;
    work
}

async fn make_repo_manager() -> (Arc<Persistence>, RepoManager, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("concerto.db");
    let persistence = Persistence::open(PersistenceConfig {
        db_path,
        max_readers: 2,
    })
    .await
    .expect("open persistence");
    let persistence = Arc::new(persistence);
    let repos_root = tmp.path().join("repos");
    let manager = RepoManager::new(Arc::clone(&persistence), repos_root);
    (persistence, manager, tmp)
}

#[tokio::test(flavor = "multi_thread")]
async fn import_local_adopts_in_place_and_is_non_destructive() {
    let (persistence, manager, _tmp) = make_repo_manager().await;
    let work = make_local_repo_with_commit().await;
    let orig_path = work.path().to_path_buf();
    // Record the original HEAD before import to prove it survives.
    let head_before = git_capture(&["rev-parse", "HEAD"], &orig_path).await;
    assert!(!head_before.is_empty());

    let repo = manager
        .import_local("somename", &orig_path)
        .await
        .expect("import_local");

    // Adopted in place: local_path is the ORIGINAL repo location (canonicalized
    // for stable de-dup — e.g. macOS resolves /tmp → /private/tmp), NOT relocated
    // under <repos_root>/<id>/.
    let orig_canonical = tokio::fs::canonicalize(&orig_path)
        .await
        .unwrap_or_else(|_| orig_path.clone());
    assert_eq!(
        Path::new(&repo.local_path),
        orig_canonical.as_path(),
        "import_local must adopt the repo in place (no relocation)"
    );
    assert_eq!(repo.name, "somename");

    // Discoverable via get_by_url (import_local recorded a url) + list_all.
    let by_url = concerto_persist::repositories::get_by_url(persistence.readers(), &repo.url)
        .await
        .expect("get_by_url")
        .expect("row should exist by url");
    assert_eq!(by_url.id, repo.id);

    let all = concerto_persist::repositories::list_all(persistence.readers())
        .await
        .expect("list_all");
    assert_eq!(all.len(), 1, "exactly one registry entry");
    assert_eq!(all[0].id, repo.id);

    // Non-destructive: the original .git survives and HEAD still resolves
    // to the same commit.
    assert!(
        orig_path.join(".git").exists(),
        "the original .git must still exist after import"
    );
    let head_after = git_capture(&["rev-parse", "HEAD"], &orig_path).await;
    assert_eq!(
        head_before, head_after,
        "the seed commit must be intact after import_local"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn import_local_dedups_same_path() {
    let (persistence, manager, _tmp) = make_repo_manager().await;
    let work = make_local_repo_with_commit().await;
    let orig_path = work.path().to_path_buf();

    let first = manager
        .import_local("first", &orig_path)
        .await
        .expect("first import_local");
    // Importing the SAME local path again must de-dup: same row back, no
    // second registry entry.
    let second = manager
        .import_local("second", &orig_path)
        .await
        .expect("second import_local");

    assert_eq!(
        first.id, second.id,
        "re-importing the same path must return the existing row (D9 de-dup)"
    );

    let all = concerto_persist::repositories::list_all(persistence.readers())
        .await
        .expect("list_all");
    assert_eq!(
        all.len(),
        1,
        "de-dup must not create a duplicate registry entry"
    );
}

/// Importing the same repo via a non-canonical spelling (trailing `/` or a
/// `./` component) must de-dup to one registry row because `import_local`
/// canonicalizes the path before deriving the URL key.
#[tokio::test(flavor = "multi_thread")]
async fn import_local_dedups_non_canonical_path_spelling() {
    let (persistence, manager, _tmp) = make_repo_manager().await;
    let work = make_local_repo_with_commit().await;
    let canonical_path = work.path().canonicalize().unwrap();

    // First import via the canonical path.
    let first = manager
        .import_local("canonical", &canonical_path)
        .await
        .expect("first import_local");

    // Second import via a path with a `./` component appended — different
    // string but same filesystem location after canonicalization.
    let dotslash_path = canonical_path.join(".").join("..");
    // Use the parent so we stay inside the actual dir (join("..") of a dir
    // yields the same dir after canonicalize).  Simpler: just re-use the
    // canonical path but construct it via joining an extra intermediate "."
    // component, which the OS collapses on canonicalize.
    let via_dot = canonical_path.join(".");
    let second = manager
        .import_local("via-dot", &via_dot)
        .await
        .expect("second import_local via ./");

    assert_eq!(
        first.id, second.id,
        "a path with a trailing '.' must de-dup to the same registry row after canonicalization"
    );

    let _ = dotslash_path; // keep the binding alive to silence the warning

    let all = concerto_persist::repositories::list_all(persistence.readers())
        .await
        .expect("list_all");
    assert_eq!(
        all.len(),
        1,
        "non-canonical spelling must not create a second registry entry"
    );
}
