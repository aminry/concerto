//! Concerto git operations wrapper (Task 18).
//!
//! Hybrid `shell-out` + `gix` per `design/02 §3.1`. The public API lives
//! in [`api`]; the [`cmd`] module is the internal shell-out helper for
//! `git` subprocess invocations.
//!
//! V0.1 surface, frozen by Task 18:
//!
//! - [`api::clone_full`] — full clone (no sparse, no blobless).
//! - [`api::fetch`] — incremental fetch.
//! - [`api::list_branches`] — local + remote refs.
//! - [`api::rev_parse_head`] — HEAD commit OID.
//! - [`api::worktree_add`] — `git worktree add`.
//!
//! Sparse-checkout, blobless / treeless clones, fsmonitor, and the
//! maintenance scheduler are all V1.0 (Tasks 28+).

pub mod api;
pub mod cmd;

pub use api::{
    clone_full, fetch, list_branches, rev_parse_head, worktree_add, BranchRef, CloneProgressEvent,
    FetchReport, ProgressSink,
};

#[cfg(test)]
mod tests {
    //! Crate-level unit tests against tempfs fixtures.
    //!
    //! Each test builds a local bare repo + a working repo with one
    //! commit, pushes to the bare, and exercises the public API against
    //! `file://` URLs. No network is required.

    use std::path::Path;

    use super::api::*;
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

    /// Build (bare_url, bare_path) — a bare repo with one commit on `main`.
    async fn make_bare_with_commit() -> (String, TempDir, TempDir) {
        let bare_dir = TempDir::new().unwrap();
        let work_dir = TempDir::new().unwrap();
        git(&["init", "--bare", "-b", "main", "."], bare_dir.path()).await;

        git(&["init", "-b", "main", "."], work_dir.path()).await;
        tokio::fs::write(work_dir.path().join("README.md"), "hello\n")
            .await
            .unwrap();
        git(&["add", "README.md"], work_dir.path()).await;
        git(&["commit", "-m", "initial"], work_dir.path()).await;
        git(
            &[
                "remote",
                "add",
                "origin",
                &format!("file://{}", bare_dir.path().display()),
            ],
            work_dir.path(),
        )
        .await;
        git(&["push", "-u", "origin", "main"], work_dir.path()).await;

        let url = format!("file://{}", bare_dir.path().display());
        (url, bare_dir, work_dir)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn clone_full_writes_a_git_dir() {
        let (url, _bare, _work) = make_bare_with_commit().await;
        let dest_root = TempDir::new().unwrap();
        let dest = dest_root.path().join("clone");

        clone_full(&url, &dest, None).await.expect("clone");
        assert!(dest.join(".git").exists(), ".git should exist after clone");
        assert!(
            dest.join("README.md").exists(),
            "checked-out worktree should include README.md"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn clone_full_streams_progress() {
        let (url, _bare, _work) = make_bare_with_commit().await;
        let dest_root = TempDir::new().unwrap();
        let dest = dest_root.path().join("clone");

        let (tx, mut rx) = tokio::sync::mpsc::channel::<CloneProgressEvent>(32);
        clone_full(&url, &dest, Some(tx)).await.expect("clone");

        // Drain the receiver and assert we got at least one event,
        // including the terminal `done`.
        let mut events: Vec<CloneProgressEvent> = Vec::new();
        while let Some(e) = rx.recv().await {
            events.push(e);
        }
        assert!(
            events.iter().any(|e| e.done),
            "expected a terminal done event; got {events:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rev_parse_head_returns_an_oid() {
        let (url, _bare, _work) = make_bare_with_commit().await;
        let dest_root = TempDir::new().unwrap();
        let dest = dest_root.path().join("clone");
        clone_full(&url, &dest, None).await.expect("clone");

        let oid = rev_parse_head(&dest).await.expect("rev_parse");
        assert_eq!(oid.len(), 40, "expected 40-char OID, got {oid:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn list_branches_returns_main() {
        let (url, _bare, _work) = make_bare_with_commit().await;
        let dest_root = TempDir::new().unwrap();
        let dest = dest_root.path().join("clone");
        clone_full(&url, &dest, None).await.expect("clone");

        let branches = list_branches(&dest).await.expect("list");
        assert!(
            branches.iter().any(|b| b.name == "main" && !b.is_remote),
            "expected local `main`; got {branches:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fetch_succeeds_on_clean_clone() {
        let (url, _bare, _work) = make_bare_with_commit().await;
        let dest_root = TempDir::new().unwrap();
        let dest = dest_root.path().join("clone");
        clone_full(&url, &dest, None).await.expect("clone");

        let report = fetch(&dest).await.expect("fetch");
        // No assertion on `updated` — git can emit either depending on
        // whether it touched FETCH_HEAD; we only assert no error.
        let _ = report;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn worktree_add_creates_a_worktree() {
        let (url, _bare, _work) = make_bare_with_commit().await;
        let dest_root = TempDir::new().unwrap();
        let dest = dest_root.path().join("clone");
        clone_full(&url, &dest, None).await.expect("clone");

        let wt = dest_root.path().join("wt");
        worktree_add(&dest, "feature-x", &wt)
            .await
            .expect("worktree_add");
        assert!(wt.exists(), "worktree dir should exist");
        assert!(
            wt.join("README.md").exists(),
            "worktree should be checked out"
        );
    }
}
