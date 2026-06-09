//! Task 301 integration test: `RepoManager` honours a real
//! [`CloneStrategy`], routes `clone_repo` through `clone_with_strategy`,
//! writes `size_bytes`/`object_count` to the repo-local
//! `concerto-state.json`, and probes a URL via `estimate_size`.
//!
//! In-process against a tempdir SQLite DB + `file://` bare-repo fixtures —
//! no network, no gRPC. Mirrors `fsmonitor_lifecycle.rs`'s `RepoManager`
//! harness.

#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;

use concerto_core::repo_manager::RepoManager;
use concerto_gix_wrap::CloneStrategy;
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

async fn git_config_get(repo: &Path, key: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["config", "--get", key])
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .await
        .expect("spawn git config");
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then_some(s)
    } else {
        None
    }
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
async fn add_persists_real_strategy_and_clone_routes_through_it() {
    let (persistence, manager, _tmp) = make_repo_manager().await;
    let (url, _bare, _work) = make_bare_with_commit().await;

    // add_repository with a real Blobless strategy — no more hardcoded "full".
    let repo = manager
        .add_repository("fixture", &url, "main", CloneStrategy::Blobless, false)
        .await
        .expect("add_repository");
    assert_eq!(
        repo.clone_strategy, "blobless",
        "the returned Repository must carry the real strategy string"
    );

    // The persisted row must carry the real strategy too.
    let row: (String,) = sqlx::query_as("SELECT clone_strategy FROM repositories WHERE id = ?")
        .bind(repo.id.as_str())
        .fetch_one(persistence.readers())
        .await
        .expect("query clone_strategy");
    assert_eq!(row.0, "blobless");

    // clone_repo routes through clone_with_strategy → blobless filter applied.
    manager
        .clone_repo(&repo.id, None)
        .await
        .expect("clone_repo");
    let local = Path::new(&repo.local_path);
    assert!(local.join(".git").exists());
    assert_eq!(
        git_config_get(local, "remote.origin.partialclonefilter")
            .await
            .as_deref(),
        Some("blob:none"),
        "clone_repo must apply the persisted blobless strategy's filter"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn clone_writes_concerto_state_json_with_size_and_object_count() {
    let (_persistence, manager, _tmp) = make_repo_manager().await;
    let (url, _bare, _work) = make_bare_with_commit().await;

    let repo = manager
        .add_repository("fixture", &url, "main", CloneStrategy::Full, false)
        .await
        .expect("add_repository");
    manager
        .clone_repo(&repo.id, None)
        .await
        .expect("clone_repo");

    let local = Path::new(&repo.local_path);
    let state_path = local.join(".git").join("concerto-state.json");
    assert!(
        state_path.exists(),
        "concerto-state.json should be written after clone (design/02 §4)"
    );
    let raw = tokio::fs::read_to_string(&state_path).await.unwrap();
    let json: serde_json::Value = serde_json::from_str(&raw).expect("valid json");
    assert!(
        json.get("size_bytes").is_some(),
        "concerto-state.json must carry size_bytes; got {raw}"
    );
    assert!(
        json.get("object_count").is_some(),
        "concerto-state.json must carry object_count; got {raw}"
    );
    // A real (if tiny) clone has a non-zero object count.
    assert!(
        json["object_count"].as_u64().unwrap_or(0) > 0,
        "object_count should be populated; got {raw}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn estimate_size_returns_populated_report_recommending_full() {
    let (_persistence, manager, _tmp) = make_repo_manager().await;
    let (url, _bare, _work) = make_bare_with_commit().await;

    let report = manager.estimate_size(&url).await.expect("estimate_size");
    assert_eq!(report.recommended, CloneStrategy::Full);
    assert!(!report.recommend_sparse);
    assert_ne!(
        report.recommended,
        CloneStrategy::Treeless,
        "treeless must never be recommended (design/02 §12 R-1)"
    );
    assert_eq!(report.branch_count, 1);
    assert!(report.object_count > 0);
}
