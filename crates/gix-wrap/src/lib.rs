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
//! Task 28 adds the fsmonitor + maintenance + performance-config
//! helpers; signatures frozen:
//!
//! - [`api::apply_perf_config`] — `core.fsmonitor` + `core.untrackedCache`
//!   + `feature.manyFiles` + `core.commitGraph` via `git config`.
//! - [`api::start_fsmonitor`] — start `git fsmonitor--daemon`, return PID.
//! - [`api::is_fsmonitor_alive`] — `kill(pid, 0)` probe.
//! - [`api::stop_fsmonitor`] — `git fsmonitor--daemon stop` (idempotent).
//! - [`api::register_maintenance`] — `git maintenance start` (best-effort).
//!
//! Task 29 adds the status + diff hot path; signatures frozen:
//!
//! - [`status::status`] — `git status --porcelain=v1 -z` parsed into a
//!   [`status::StatusReport`].
//! - [`diff::diff_head`] — worktree-vs-HEAD diff as a [`diff::DiffPayload`].
//! - [`diff::diff_to_main`] — worktree-vs-`<branch>` diff.
//!
//! Task 301 adds the V1.0 clone-strategy surface; signatures frozen:
//!
//! - [`api::CloneStrategy`] — `Full | Blobless | Treeless`, serializing to
//!   the `repositories.clone_strategy` TEXT values.
//! - [`api::clone_with_strategy`] — clone with an explicit strategy +
//!   optional `--sparse --no-checkout` flags (`clone_full` is untouched).
//! - [`api::estimate_repo_size`] — pre-clone size probe → [`api::SizeReport`]
//!   implementing the `design/02 §3.5` size→strategy heuristic.
//!
//! Task 302 adds the sparse-checkout + cone + sparse-index lifecycle;
//! signatures frozen (`design/00 §6.3`: cone-mode mandatory, sparse-index
//! always-on):
//!
//! - [`sparse::sparse_init_cone`] — `sparse-checkout init --cone --sparse-index`.
//! - [`sparse::sparse_set`] — replace the cone (`set --sparse-index`, bad
//!   paths rejected) + reapply.
//! - [`sparse::sparse_add`] — add to the cone + reapply.
//! - [`sparse::sparse_reapply_index`] — `reapply --sparse-index`.
//! - [`sparse::sparse_disable`] — `sparse-checkout disable` (full materialize).
//! - [`sparse::is_cone_mode`] / [`sparse::force_cone_mode`] — the `design/02
//!   §8` non-cone-force path.
//!
//! Idle blob prewarm remains Task 304; cone-level size telemetry remains
//! Task 305.

pub mod api;
pub mod cmd;
pub mod diff;
pub mod sparse;
pub mod status;

pub use api::{
    apply_perf_config, clone_full, clone_with_strategy, commit_index, estimate_repo_size, fetch,
    hard_reset, is_fsmonitor_alive, list_branches, ref_exists, register_maintenance,
    rev_parse_head, start_fsmonitor, stop_fsmonitor, update_ref, worktree_add, BranchRef,
    CloneProgressEvent, CloneStrategy, FetchReport, ProgressSink, SizeReport,
};
// Task 302 sparse lifecycle surface.
pub use sparse::{
    force_cone_mode, is_cone_mode, sparse_add, sparse_disable, sparse_init_cone, sparse_list,
    sparse_reapply_index, sparse_set, ConePath,
};
// Task 29 hot-path surface — status + diff against HEAD / a branch.
pub use diff::{diff_head, diff_to_main, DiffHunk, DiffKind, DiffPayload, FileDiff};
pub use status::{status, StatusEntry, StatusReport, StatusState};

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
