//! Task 305 integration coverage: the cone-level telemetry probe
//! (`RepoManager::list_paths_in_cone` → `ConeStats`, read from the git
//! **index**) and the unwired `suggest_cones` Maestro-delegate seam.
//!
//! In-process against a tempdir SQLite DB + `file://` bare-repo fixtures —
//! no network, no gRPC. Mirrors `repo_size_estimate.rs`'s `RepoManager`
//! harness.
//!
//! Coverage (Tier 1, the deterministic/telemetry path — the live-LLM
//! `suggest_cones` is wired in P4, Task 411, and judged at that phase gate):
//! 1. `list_paths_in_cone(["a"])` on a known coned tree → exact in-cone file
//!    count + a non-zero index-recorded `disk_size_bytes`.
//! 2. A cone with no in-cone files → `{0, 0}`.
//! 3. The `ConeSuggester` seam unwired (`None`) → `ConeSuggestError::Unwired`,
//!    which the handler maps to `Status::unimplemented` (NOT empty success).
//! 4. An injected mock `ConeSuggester` is delegated to verbatim (proves the
//!    seam wires without any Maestro).

#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use concerto_core::handlers::repositories::cone_suggest_error_to_status;
use concerto_core::repo_manager::{ConeSuggestError, ConeSuggester, RepoManager};
use concerto_error::Result as CResult;
use concerto_gix_wrap::{self as gixw, CloneStrategy, ConePath};
use concerto_persist::{Persistence, PersistenceConfig, RepositoryId};
use tempfile::TempDir;
use tokio::process::Command;
use tonic::Code;

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

/// Build a bare repo whose `main` carries a known multi-dir tree:
///   README.md            (top-level)
///   a/f1.txt  a/f2.txt   (cone `a` → 2 files)
///   b/g1.txt             (cone `b` → 1 file)
/// Returns its `file://` URL.
async fn make_bare_with_tree() -> (String, TempDir, TempDir) {
    let bare = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    git(&["init", "--bare", "-b", "main", "."], bare.path()).await;
    git(&["init", "-b", "main", "."], work.path()).await;

    tokio::fs::write(work.path().join("README.md"), "top-level readme\n")
        .await
        .unwrap();
    tokio::fs::create_dir(work.path().join("a")).await.unwrap();
    tokio::fs::write(work.path().join("a/f1.txt"), "alpha one\n")
        .await
        .unwrap();
    tokio::fs::write(work.path().join("a/f2.txt"), "alpha two contents\n")
        .await
        .unwrap();
    tokio::fs::create_dir(work.path().join("b")).await.unwrap();
    tokio::fs::write(work.path().join("b/g1.txt"), "bravo one\n")
        .await
        .unwrap();

    git(&["add", "-A"], work.path()).await;
    git(&["commit", "-m", "tree"], work.path()).await;
    git(
        &[
            "remote",
            "add",
            "origin",
            &format!("file://{}", bare.path().display()),
        ],
        work.path(),
    )
    .await;
    git(&["push", "-u", "origin", "main"], work.path()).await;
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

/// Clone the fixture into the manager's repos root, then cone its checkout
/// down to `cone` (cone-mode + sparse-index, via the Task 302 helpers).
/// Returns the cloned repo id + its on-disk path.
async fn clone_and_cone(
    manager: &RepoManager,
    project_id: &str,
    url: &str,
    cone: &[ConePath],
) -> (RepositoryId, std::path::PathBuf) {
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
    let local = std::path::PathBuf::from(&repo.local_path);

    // Cone the checkout (Task 302's lifecycle: init --cone --sparse-index,
    // then set + reapply). `list_paths_in_cone` reads the resulting index.
    gixw::sparse_init_cone(&local)
        .await
        .expect("sparse_init_cone");
    gixw::sparse_set(&local, cone).await.expect("sparse_set");
    (repo.id, local)
}

#[tokio::test(flavor = "multi_thread")]
async fn list_paths_in_cone_counts_exact_files_and_nonzero_size() {
    let (_p, manager, _tmp) = make_repo_manager("p-cone").await;
    let (url, _bare, _work) = make_bare_with_tree().await;
    let (repo_id, _local) = clone_and_cone(&manager, "p-cone", &url, &["a".to_string()]).await;

    let stats = manager
        .list_paths_in_cone(&repo_id, &["a".to_string()])
        .await
        .expect("list_paths_in_cone");

    // Cone `a` materializes exactly a/f1.txt + a/f2.txt.
    assert_eq!(
        stats.file_count, 2,
        "cone `a` should count exactly its two tracked files; got {stats:?}"
    );
    assert!(
        stats.disk_size_bytes > 0,
        "in-cone files have non-zero index-recorded size; got {stats:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn list_paths_in_cone_empty_for_out_of_cone_prefix() {
    let (_p, manager, _tmp) = make_repo_manager("p-empty").await;
    let (url, _bare, _work) = make_bare_with_tree().await;
    // Cone down to `a` → `b/` collapses to a sparse directory entry.
    let (repo_id, _local) = clone_and_cone(&manager, "p-empty", &url, &["a".to_string()]).await;

    // Probing `b` finds no in-cone *file* entries (only the collapsed dir
    // entry, which the index probe skips) → {0, 0}.
    let stats = manager
        .list_paths_in_cone(&repo_id, &["b".to_string()])
        .await
        .expect("list_paths_in_cone");
    assert_eq!(
        stats.file_count, 0,
        "a cone with no in-cone files yields file_count 0; got {stats:?}"
    );
    assert_eq!(
        stats.disk_size_bytes, 0,
        "a cone with no in-cone files yields disk_size_bytes 0; got {stats:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn suggest_cones_unwired_seam_maps_to_unimplemented() {
    let (_p, manager, _tmp) = make_repo_manager("p-seam").await;
    let repo = RepositoryId("does-not-need-to-exist".to_string());

    // No ConeSuggester injected (the P3 default) → the FROZEN unwired signal.
    let err = manager
        .suggest_cones(&repo, "fix the auth bug")
        .await
        .expect_err("unwired seam must NOT be an empty success");
    assert!(
        matches!(err, ConeSuggestError::Unwired),
        "expected ConeSuggestError::Unwired"
    );

    // The handler maps the unwired seam to Status::unimplemented (NOT a panic,
    // NOT InvalidArgument) — the contract Task 411 wires into.
    let status = cone_suggest_error_to_status(err);
    assert_eq!(status.code(), Code::Unimplemented);
}

/// A test-only `ConeSuggester` that proves the seam delegates without any
/// Maestro. P4 (Task 411) injects the real Maestro-backed implementor here.
struct MockSuggester {
    canned: Vec<ConePath>,
}

#[async_trait]
impl ConeSuggester for MockSuggester {
    async fn suggest_cones(
        &self,
        _repo: &RepositoryId,
        issue_text: &str,
    ) -> CResult<Vec<ConePath>> {
        // Prove the args reach the implementor.
        assert!(!issue_text.is_empty(), "issue_text should be forwarded");
        Ok(self.canned.clone())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn suggest_cones_delegates_to_injected_suggester() {
    let (_p, manager, _tmp) = make_repo_manager("p-mock").await;
    let manager = manager.with_cone_suggester(Arc::new(MockSuggester {
        canned: vec!["packages/core".to_string(), "apps/web".to_string()],
    }));
    let repo = RepositoryId("repo-xyz".to_string());

    let suggested = manager
        .suggest_cones(&repo, "implement the new billing flow")
        .await
        .expect("injected suggester is delegated to");
    assert_eq!(
        suggested,
        vec!["packages/core".to_string(), "apps/web".to_string()],
        "the seam must return the injected suggester's cones verbatim"
    );
}
