//! `git rev-list --count` ahead-count helper (Task 404).
//!
//! The Maestro summary cache (`crates/core/src/maestro/summary.rs`) reports a
//! per-repo `commits_ahead: u32` hard fact. There was no ahead-count primitive
//! in `gix-wrap` before this task — `diff_head`/`diff_to_main`/`cone_index_stats`
//! cover diffs and index stats, but not "how many commits is this branch past
//! its base?".
//!
//! Following the 305 placement precedent, the git/`gix` tooling lives in
//! `gix-wrap` (a `git` shell-out through the existing [`cmd::run`] helper), so
//! `core` gains no new git dependency or `cargo deny` surface. The
//! [`commits_ahead`] signature is FROZEN here (see the `lib.rs` doc-comment
//! block).

use std::path::Path;

use concerto_error::Result;

use crate::cmd;

/// Count of commits on the worktree's `HEAD` that are NOT on `base`
/// (i.e. `git rev-list --count <base>..HEAD`). `base` is passed through
/// verbatim — callers building it from user input validate first.
/// Returns `0` (not an error) for a zero/empty count.
///
/// The `<base>..HEAD` two-dot range is the *strictly-ahead* set: commits
/// reachable from `HEAD` but not from `base`. The symmetric `...` form is
/// deliberately NOT used — we want the workarea branch's lead over its base,
/// not the union of both sides' divergence.
pub async fn commits_ahead(worktree_path: &Path, base: &str) -> Result<u32> {
    let range = format!("{base}..HEAD");
    let out = cmd::run(&["rev-list", "--count", &range], worktree_path).await?;
    // `--count` prints a single integer line; an empty/blank output means
    // zero commits ahead. A non-numeric line is treated as `0` rather than an
    // error so a freshly-created branch with no base divergence never trips up
    // the summary-cache refresh.
    Ok(out.stdout.trim().parse::<u32>().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;
    use tokio::process::Command;

    /// Spawn `git` synchronously inside a tempdir; panic on failure.
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

    /// Build a working repo on `main` with one commit; return its dir.
    async fn make_repo_with_base() -> TempDir {
        let work = TempDir::new().unwrap();
        git(&["init", "-b", "main", "."], work.path()).await;
        tokio::fs::write(work.path().join("README.md"), "hello\n")
            .await
            .unwrap();
        git(&["add", "README.md"], work.path()).await;
        git(&["commit", "-m", "initial"], work.path()).await;
        work
    }

    /// Add `n` commits on a feature branch off `main`.
    async fn add_commits(work: &Path, n: usize) {
        git(&["checkout", "-b", "feature"], work).await;
        for i in 0..n {
            tokio::fs::write(work.join(format!("f{i}.txt")), format!("change {i}\n"))
                .await
                .unwrap();
            git(&["add", "."], work).await;
            git(&["commit", "-m", &format!("commit {i}")], work).await;
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ahead_count_matches_commits_past_base() {
        let work = make_repo_with_base().await;
        add_commits(work.path(), 3).await;

        let n = commits_ahead(work.path(), "main").await.expect("ahead");
        assert_eq!(n, 3, "feature should be exactly 3 commits past main");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ahead_count_is_zero_when_not_ahead() {
        let work = make_repo_with_base().await;
        // HEAD == main; the range main..HEAD is empty → 0.
        let n = commits_ahead(work.path(), "main").await.expect("ahead");
        assert_eq!(n, 0, "no commits past base → 0");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ahead_count_one_commit() {
        let work = make_repo_with_base().await;
        add_commits(work.path(), 1).await;
        let n = commits_ahead(work.path(), "main").await.expect("ahead");
        assert_eq!(n, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ahead_count_errors_on_unknown_base() {
        let work = make_repo_with_base().await;
        // A base ref that does not exist is a git error, surfaced as `Err`
        // (not a silent 0) — the empty-output 0 path is only for a valid
        // range that resolves to no commits.
        let r = commits_ahead(work.path(), "does-not-exist").await;
        assert!(r.is_err(), "unknown base should be an error, got {r:?}");
    }
}
