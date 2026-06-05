//! Task 302 — sparse-checkout + cone + sparse-index lifecycle tests for
//! `concerto-gix-wrap`.
//!
//! All coverage runs against a `file://` bare-repo fixture with a known
//! multi-dir tree (`a/`, `b/`, `c/` + a top-level file), built in-test by
//! shelling out to `git` — no network. Asserts the `design/00 §6.3` invariant
//! (cone-mode mandatory, `--sparse-index` always-on):
//!
//! - `sparse_init_cone` + `sparse_set(["a"])` materializes ONLY `a/` (b/ and
//!   c/ collapsed) and `git sparse-checkout list` reports the cone;
//! - the sparse index is active after the set (`ls-files --sparse` reports
//!   out-of-cone trees as collapsed `b/` / `c/` directory entries) — the
//!   lever Task 303's `< 100 ms status` bar leans on;
//! - `sparse_add` extends the cone;
//! - a bad cone path (absent from HEAD) is rejected with a clean
//!   `Error::Validation` BEFORE applying (nothing half-applied);
//! - `is_cone_mode` / `force_cone_mode` flip `core.sparseCheckoutCone`
//!   (the `design/02 §8` non-cone-force path);
//! - `sparse_disable` fully re-materializes the worktree.
//!
//! `git sparse-checkout --sparse-index` needs git ≥ 2.27; the CI matrix
//! (Task 113) ships a modern git on every lane.

use std::path::{Path, PathBuf};

use concerto_error::Error;
use concerto_gix_wrap::{
    clone_full, force_cone_mode, is_cone_mode, sparse_add, sparse_disable, sparse_init_cone,
    sparse_list, sparse_set,
};
use tempfile::TempDir;
use tokio::process::Command;

/// Spawn `git` synchronously inside a tempdir; panic on failure. Hermetic
/// (no global/system config).
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

/// `git <args>` capturing trimmed stdout.
async fn git_out(args: &[&str], cwd: &Path) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .await
        .expect("spawn git");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Build a bare repo `file://` URL whose `main` has a top-level file plus
/// `a/`, `b/`, `c/` directories (each with one file). Returns the URL and
/// keeps the TempDirs alive for the test's lifetime.
async fn make_multidir_bare() -> (String, TempDir, TempDir) {
    let bare = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    git(&["init", "--bare", "-b", "main", "."], bare.path()).await;

    git(&["init", "-b", "main", "."], work.path()).await;
    tokio::fs::write(work.path().join("ROOT.md"), "root\n")
        .await
        .unwrap();
    for d in ["a", "b", "c"] {
        let dir = work.path().join(d);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join(format!("file_{d}.txt")), format!("in {d}\n"))
            .await
            .unwrap();
    }
    git(&["add", "-A"], work.path()).await;
    git(&["commit", "-m", "seed a/b/c"], work.path()).await;
    let url = format!("file://{}", bare.path().display());
    git(&["remote", "add", "origin", &url], work.path()).await;
    git(&["push", "-u", "origin", "main"], work.path()).await;

    (url, bare, work)
}

/// Clone the fixture into a fresh worktree and return its path (+ owning
/// TempDir).
async fn clone_fixture(url: &str) -> (PathBuf, TempDir) {
    let root = TempDir::new().unwrap();
    let dest = root.path().join("clone");
    clone_full(url, &dest, None).await.expect("clone");
    (dest, root)
}

#[tokio::test(flavor = "multi_thread")]
async fn init_cone_and_set_materializes_only_in_cone() {
    let (url, _bare, _work) = make_multidir_bare().await;
    let (wt, _root) = clone_fixture(&url).await;

    sparse_init_cone(&wt).await.expect("init cone");
    // Pass the cone WITH a trailing slash (`a/`) — the form callers + git's
    // cone syntax use. The HEAD-tree probe must normalize it (a trailing
    // slash makes `git ls-tree -d HEAD a/` match nothing) so a valid
    // directory cone is not wrongly rejected.
    sparse_set(&wt, &["a/".to_string()]).await.expect("set a/");

    // `sparse-checkout list` reports the cone (git normalizes `a/` → `a`).
    let listed = sparse_list(&wt).await.expect("list");
    assert!(
        listed.iter().any(|c| c == "a"),
        "expected cone to contain `a`; got {listed:?}"
    );

    // Only `a/` materialized on disk; b/ and c/ collapsed.
    assert!(wt.join("a/file_a.txt").exists(), "a/ must materialize");
    assert!(wt.join("ROOT.md").exists(), "top-level file always present");
    assert!(
        !wt.join("b/file_b.txt").exists() && !wt.join("c/file_c.txt").exists(),
        "out-of-cone b/ and c/ must NOT materialize"
    );

    // Cone mode is on.
    assert!(is_cone_mode(&wt).await.expect("is_cone_mode"));

    // The sparse index is active: `ls-files --sparse` reports the out-of-cone
    // trees as collapsed directory entries `b/` and `c/` (the --sparse-index
    // lever Task 303 leans on).
    let sparse_files = git_out(&["ls-files", "--sparse"], &wt).await;
    assert!(
        sparse_files.lines().any(|l| l == "b/") && sparse_files.lines().any(|l| l == "c/"),
        "sparse index not active — expected collapsed b/ and c/ entries; got:\n{sparse_files}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sparse_add_extends_the_cone() {
    let (url, _bare, _work) = make_multidir_bare().await;
    let (wt, _root) = clone_fixture(&url).await;

    sparse_init_cone(&wt).await.expect("init");
    sparse_set(&wt, &["a".to_string()]).await.expect("set a");
    sparse_add(&wt, &["b".to_string()]).await.expect("add b");

    assert!(wt.join("a/file_a.txt").exists(), "a/ still in cone");
    assert!(wt.join("b/file_b.txt").exists(), "b/ added to cone");
    assert!(!wt.join("c/file_c.txt").exists(), "c/ still out of cone");
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_cone_path_is_rejected_without_partial_apply() {
    let (url, _bare, _work) = make_multidir_bare().await;
    let (wt, _root) = clone_fixture(&url).await;

    sparse_init_cone(&wt).await.expect("init");
    sparse_set(&wt, &["a".to_string()]).await.expect("set a");

    // A path absent from HEAD must be rejected as a clean Error::Validation
    // (→ INVALID_ARGUMENT at the handler) BEFORE git is invoked.
    let err = sparse_set(&wt, &["does/not/exist".to_string()])
        .await
        .expect_err("bad cone path must error");
    assert!(
        matches!(err, Error::Validation(_)),
        "expected Error::Validation, got {err:?}"
    );

    // Nothing half-applied: the prior cone (`a`) is intact, b/ and c/ still
    // collapsed.
    let listed = sparse_list(&wt).await.expect("list");
    assert!(
        listed.iter().any(|c| c == "a") && !wt.join("b/file_b.txt").exists(),
        "prior cone must be intact after a rejected set; listed={listed:?}"
    );

    // A path that exists as a file (blob) but not a directory (tree) is also
    // rejected — cone mode only accepts directory prefixes.
    let err = sparse_set(&wt, &["ROOT.md".to_string()])
        .await
        .expect_err("file-as-cone must error");
    assert!(matches!(err, Error::Validation(_)), "got {err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn force_cone_mode_flips_the_config_key() {
    let (url, _bare, _work) = make_multidir_bare().await;
    let (wt, _root) = clone_fixture(&url).await;

    // A plain full clone has no `core.sparseCheckoutCone` set → not cone mode.
    assert!(!is_cone_mode(&wt).await.expect("is_cone_mode (initial)"));

    // Simulate a manually-cloned NON-cone sparse config (design/02 §8).
    git(&["config", "core.sparseCheckout", "true"], &wt).await;
    git(&["config", "core.sparseCheckoutCone", "false"], &wt).await;
    assert!(
        !is_cone_mode(&wt).await.expect("is_cone_mode (non-cone)"),
        "non-cone sparse config must read as not-cone-mode"
    );

    // Force it to cone mode.
    force_cone_mode(&wt).await.expect("force cone");
    assert!(
        is_cone_mode(&wt).await.expect("is_cone_mode (forced)"),
        "force_cone_mode must set core.sparseCheckoutCone=true"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn disable_re_materializes_the_whole_tree() {
    let (url, _bare, _work) = make_multidir_bare().await;
    let (wt, _root) = clone_fixture(&url).await;

    sparse_init_cone(&wt).await.expect("init");
    sparse_set(&wt, &["a".to_string()]).await.expect("set a");
    assert!(!wt.join("c/file_c.txt").exists(), "c/ collapsed under cone");

    sparse_disable(&wt).await.expect("disable");
    assert!(
        wt.join("b/file_b.txt").exists() && wt.join("c/file_c.txt").exists(),
        "disable must re-materialize the full tree"
    );
}
