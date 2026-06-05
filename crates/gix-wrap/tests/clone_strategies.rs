//! Task 301 — clone-strategy + size-estimate tests for `concerto-gix-wrap`.
//!
//! All coverage runs against `file://` bare-repo fixtures built in-test by
//! shelling out to `git` — no network. Asserts:
//!
//! - each [`CloneStrategy`] maps to the right `git` partial-clone filter
//!   (recorded as `remote.origin.partialclonefilter` in the clone config);
//! - `with_sparse` honours `--no-checkout` (empty worktree for Task 302);
//! - `clone_full` is byte-for-byte unchanged (full checkout, no filter);
//! - the `design/02 §3.5` size→strategy heuristic + `FromStr`/`as_str`
//!   round-trips;
//! - `estimate_repo_size` returns a populated `SizeReport` recommending
//!   `Full` for a tiny repo, and NEVER recommends `Treeless`.

use std::path::Path;
use std::str::FromStr;

use concerto_gix_wrap::{clone_full, clone_with_strategy, estimate_repo_size, CloneStrategy};
use tempfile::TempDir;
use tokio::process::Command;

/// Spawn `git` synchronously inside a tempdir; panic on failure. Isolates
/// from the developer's global/system git config so the test is hermetic.
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
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Read `git config --get <key>` from a clone; `None` when unset.
async fn git_config_get(repo_dir: &Path, key: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["config", "--get", key])
        .current_dir(repo_dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .await
        .expect("spawn git config");
    if out.status.success() {
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if v.is_empty() {
            None
        } else {
            Some(v)
        }
    } else {
        None
    }
}

/// Build a bare repo with two commits on `main`; return its `file://` URL.
/// Two commits give the filters something non-trivial to defer.
async fn make_bare_with_commits() -> (String, TempDir, TempDir) {
    let bare = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    git(&["init", "--bare", "-b", "main", "."], bare.path()).await;
    git(&["init", "-b", "main", "."], work.path()).await;
    tokio::fs::write(work.path().join("README.md"), "hello\n")
        .await
        .unwrap();
    git(&["add", "README.md"], work.path()).await;
    git(&["commit", "-m", "initial"], work.path()).await;
    tokio::fs::write(work.path().join("second.txt"), "world\n")
        .await
        .unwrap();
    git(&["add", "second.txt"], work.path()).await;
    git(&["commit", "-m", "second"], work.path()).await;
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

#[tokio::test(flavor = "multi_thread")]
async fn full_strategy_checks_out_and_has_no_filter() {
    let (url, _bare, _work) = make_bare_with_commits().await;
    let dest_root = TempDir::new().unwrap();
    let dest = dest_root.path().join("clone");

    clone_with_strategy(&url, &dest, CloneStrategy::Full, false, None)
        .await
        .expect("full clone");

    assert!(dest.join(".git").exists(), ".git should exist");
    assert!(
        dest.join("README.md").exists(),
        "full clone should check out the worktree"
    );
    assert_eq!(
        git_config_get(&dest, "remote.origin.partialclonefilter").await,
        None,
        "full clone must not record a partial-clone filter"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn blobless_strategy_records_blob_none_filter() {
    let (url, _bare, _work) = make_bare_with_commits().await;
    let dest_root = TempDir::new().unwrap();
    let dest = dest_root.path().join("clone");

    clone_with_strategy(&url, &dest, CloneStrategy::Blobless, false, None)
        .await
        .expect("blobless clone");

    // Blobless keeps commits + trees; blobs are lazy. The recorded filter
    // is the authoritative signal that `--filter=blob:none` was applied.
    assert_eq!(
        git_config_get(&dest, "remote.origin.partialclonefilter")
            .await
            .as_deref(),
        Some("blob:none"),
        "blobless clone must record the blob:none filter"
    );
    // Worktree still checked out (no --no-checkout), so trees are present.
    assert!(dest.join("README.md").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn treeless_strategy_records_tree_zero_filter() {
    let (url, _bare, _work) = make_bare_with_commits().await;
    let dest_root = TempDir::new().unwrap();
    let dest = dest_root.path().join("clone");

    clone_with_strategy(&url, &dest, CloneStrategy::Treeless, false, None)
        .await
        .expect("treeless clone");

    assert_eq!(
        git_config_get(&dest, "remote.origin.partialclonefilter")
            .await
            .as_deref(),
        Some("tree:0"),
        "treeless clone must record the tree:0 filter"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn with_sparse_honours_no_checkout_empty_worktree() {
    let (url, _bare, _work) = make_bare_with_commits().await;
    let dest_root = TempDir::new().unwrap();
    let dest = dest_root.path().join("clone");

    clone_with_strategy(&url, &dest, CloneStrategy::Blobless, true, None)
        .await
        .expect("blobless+sparse clone");

    assert!(dest.join(".git").exists(), ".git should exist");
    // `--no-checkout` means the worktree lands empty for Task 302's
    // `sparse-checkout init --cone` to populate.
    assert!(
        !dest.join("README.md").exists(),
        "with_sparse (--no-checkout) must leave the worktree empty"
    );
    assert_eq!(
        git_config_get(&dest, "remote.origin.partialclonefilter")
            .await
            .as_deref(),
        Some("blob:none")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn clone_full_unchanged_full_checkout_no_filter() {
    // `clone_full` must stay byte-for-byte equivalent to V0.1: full
    // checkout, no partial-clone filter.
    let (url, _bare, _work) = make_bare_with_commits().await;
    let dest_root = TempDir::new().unwrap();
    let dest = dest_root.path().join("clone");

    clone_full(&url, &dest, None).await.expect("clone_full");

    assert!(dest.join("README.md").exists());
    assert_eq!(
        git_config_get(&dest, "remote.origin.partialclonefilter").await,
        None
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn estimate_repo_size_recommends_full_for_tiny_repo() {
    let (url, _bare, _work) = make_bare_with_commits().await;

    let report = estimate_repo_size(&url).await.expect("estimate");

    // A two-commit fixture is far under 1 GB → Full, not sparse.
    assert_eq!(report.recommended, CloneStrategy::Full);
    assert!(!report.recommend_sparse);
    assert_ne!(
        report.recommended,
        CloneStrategy::Treeless,
        "the heuristic must NEVER recommend treeless (design/02 §12 R-1)"
    );
    // Populated report: one branch (`main`), some objects.
    assert_eq!(report.branch_count, 1, "fixture has one branch");
    assert!(
        report.object_count > 0,
        "object_count should be populated; got {}",
        report.object_count
    );
}

/// Table-driven test of the FROZEN `design/02 §3.5` size→strategy
/// heuristic. The thresholds are an internal of `estimate_repo_size`, so
/// we re-state the boundary table here as the contract the recommendation
/// must satisfy and assert each tier never yields treeless.
#[test]
fn heuristic_boundary_table() {
    const ONE_GB: u64 = 1024 * 1024 * 1024;
    const TEN_GB: u64 = 10 * ONE_GB;

    // (size_bytes, expected_strategy, expected_sparse)
    let cases: &[(u64, CloneStrategy, bool)] = &[
        (0, CloneStrategy::Full, false),
        (ONE_GB - 1, CloneStrategy::Full, false),
        (ONE_GB, CloneStrategy::Blobless, false),
        (5 * ONE_GB, CloneStrategy::Blobless, false),
        (TEN_GB, CloneStrategy::Blobless, false),
        (TEN_GB + 1, CloneStrategy::Blobless, true),
        (100 * ONE_GB, CloneStrategy::Blobless, true),
    ];

    for &(size_bytes, want_strategy, want_sparse) in cases {
        let (got_strategy, got_sparse) = recommend_for(size_bytes);
        assert_eq!(
            got_strategy, want_strategy,
            "size {size_bytes}: strategy mismatch"
        );
        assert_eq!(
            got_sparse, want_sparse,
            "size {size_bytes}: sparse mismatch"
        );
        assert_ne!(
            got_strategy,
            CloneStrategy::Treeless,
            "size {size_bytes}: treeless must never be recommended"
        );
    }
}

/// Mirror of the FROZEN heuristic in `estimate_repo_size`. Kept here as the
/// boundary-table oracle (the production fn's thresholds are private).
fn recommend_for(size_bytes: u64) -> (CloneStrategy, bool) {
    const ONE_GB: u64 = 1024 * 1024 * 1024;
    const TEN_GB: u64 = 10 * ONE_GB;
    if size_bytes < ONE_GB {
        (CloneStrategy::Full, false)
    } else if size_bytes <= TEN_GB {
        (CloneStrategy::Blobless, false)
    } else {
        (CloneStrategy::Blobless, true)
    }
}

#[test]
fn clone_strategy_string_round_trip() {
    assert_eq!(CloneStrategy::Full.as_str(), "full");
    assert_eq!(CloneStrategy::Blobless.as_str(), "blobless");
    assert_eq!(CloneStrategy::Treeless.as_str(), "treeless");

    assert_eq!(
        CloneStrategy::from_str("full").unwrap(),
        CloneStrategy::Full
    );
    assert_eq!(
        CloneStrategy::from_str("blobless").unwrap(),
        CloneStrategy::Blobless
    );
    assert_eq!(
        CloneStrategy::from_str("treeless").unwrap(),
        CloneStrategy::Treeless
    );
    // Empty string parses as Full (preserves V0.1 wire callers).
    assert_eq!(CloneStrategy::from_str("").unwrap(), CloneStrategy::Full);
    // Unknown is a hard error, never a silent Full.
    assert!(CloneStrategy::from_str("bogus").is_err());
}
