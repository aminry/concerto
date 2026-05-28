//! Status parity test (Task 29).
//!
//! Builds a small repo with a known mix of added / modified / deleted /
//! untracked files, then asserts that `concerto_gix_wrap::status` and
//! `git status --porcelain=v1` agree on the set of changed paths.
//!
//! Per Task 29 pre-decision 10, we focus on the file list and coarse
//! state, not on the exact byte-for-byte porcelain output. Rename
//! detection is left for the dedicated `gix-wrap::diff` tests; `git
//! status` itself does not perform rename detection by default.

use std::path::{Path, PathBuf};

use concerto_gix_wrap::{status, StatusState};
use tempfile::TempDir;
use tokio::process::Command;

/// Shell out to `git` for fixture setup.
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

/// Capture `git status --porcelain=v1` output as a sorted list of
/// (path, two-letter-code) pairs.
async fn git_status_paths(cwd: &Path) -> Vec<(String, String)> {
    let out = Command::new("git")
        .args(["status", "--porcelain=v1"])
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .await
        .expect("spawn git status");
    assert!(
        out.status.success(),
        "git status failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut entries: Vec<(String, String)> = stdout
        .lines()
        .filter_map(|l| {
            if l.len() < 3 {
                return None;
            }
            let code = l[..2].to_string();
            let path = l[3..].to_string();
            Some((path, code))
        })
        .collect();
    entries.sort();
    entries
}

#[tokio::test(flavor = "multi_thread")]
async fn parity_with_git_status_porcelain() {
    let dir = TempDir::new().unwrap();
    git(&["init", "-q", "-b", "main", "."], dir.path()).await;
    // Seed: keep.txt (unchanged), modify.txt (will be modified),
    // delete.txt (will be deleted).
    tokio::fs::write(dir.path().join("keep.txt"), "keep\n")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("modify.txt"), "v1\n")
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("delete.txt"), "doomed\n")
        .await
        .unwrap();
    git(&["add", "-A"], dir.path()).await;
    git(&["commit", "-q", "-m", "seed"], dir.path()).await;

    // Stage one addition (added.txt), modify a tracked file, delete a
    // tracked file, leave one file untracked.
    tokio::fs::write(dir.path().join("added.txt"), "new!\n")
        .await
        .unwrap();
    git(&["add", "added.txt"], dir.path()).await;
    tokio::fs::write(dir.path().join("modify.txt"), "v2\n")
        .await
        .unwrap();
    tokio::fs::remove_file(dir.path().join("delete.txt"))
        .await
        .unwrap();
    tokio::fs::write(dir.path().join("untracked.txt"), "??\n")
        .await
        .unwrap();

    // Capture both views.
    let baseline = git_status_paths(dir.path()).await;
    let report = status(dir.path()).await.expect("status");

    // Same set of paths.
    let baseline_paths: Vec<PathBuf> = baseline.iter().map(|(p, _)| PathBuf::from(p)).collect();
    let mut report_paths: Vec<PathBuf> = report.files.iter().map(|e| e.path.clone()).collect();
    report_paths.sort();
    let mut baseline_paths_sorted = baseline_paths.clone();
    baseline_paths_sorted.sort();
    assert_eq!(
        report_paths, baseline_paths_sorted,
        "status() and `git status --porcelain` disagree on the file set"
    );

    // Spot-check the state classification.
    let by_name: std::collections::HashMap<_, _> = report
        .files
        .iter()
        .map(|e| (e.path.to_string_lossy().into_owned(), e.state.clone()))
        .collect();
    assert!(matches!(by_name.get("added.txt"), Some(StatusState::Added)));
    assert!(matches!(
        by_name.get("modify.txt"),
        Some(StatusState::Modified)
    ));
    assert!(matches!(
        by_name.get("delete.txt"),
        Some(StatusState::Deleted)
    ));
    assert!(matches!(
        by_name.get("untracked.txt"),
        Some(StatusState::Untracked)
    ));
}
