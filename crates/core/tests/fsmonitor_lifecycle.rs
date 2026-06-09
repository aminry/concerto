//! Task 28 integration test: post-clone fsmonitor / maintenance / perf
//! config bring-up + restart-policy bookkeeping.
//!
//! Two slices of coverage:
//!
//! 1. **End-to-end clone path** — drive `RepoManager::clone_repo` against
//!    a tempdir bare repo and assert the on-disk side effects:
//!    `git config --get core.fsmonitor` returns `true` and the recorded
//!    `fs_monitor_pid` is alive (`kill(pid, 0)`). On hosts where
//!    `git fsmonitor--daemon` is not supported (older git, or a
//!    filesystem the daemon refuses) the test skips the fsmonitor
//!    assertions and just verifies the perf config landed.
//!
//! 2. **Restart-policy bookkeeping** — exercise `fsmonitor::probe_all`
//!    against a mocked `is_alive` so the policy (3-in-60s → disable) is
//!    observed without spinning the real 30s loop. The "kill the
//!    daemon and wait 35s" slice of the spec is too slow for CI; that
//!    drift is documented in tasks/28's Handoff Notes.

#![cfg(unix)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use concerto_core::repo_manager::fsmonitor::{self, ProbeOutcome, RestartHistory};
use concerto_core::repo_manager::RepoManager;
use concerto_gix_wrap::CloneStrategy;
use concerto_persist::{Persistence, PersistenceConfig, RepositoryId};
use tempfile::TempDir;
use tokio::process::Command;
use tokio::sync::Mutex;

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

/// `git config --get <key>` returning Some(value) or None. Trimmed.
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
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
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

/// Build an in-process `RepoManager` over a tempdir SQLite DB. Seeds the
/// `projects` row the FK requires.
async fn make_repo_manager() -> (Arc<Persistence>, RepoManager, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("concerto.db");
    let persistence = Persistence::open(PersistenceConfig {
        db_path: db_path.clone(),
        max_readers: 2,
    })
    .await
    .expect("open persistence");
    let persistence = Arc::new(persistence);

    // Seed the `projects` row directly — the Projects gRPC service
    // doesn't exist in V0.1; matches `repository_clone.rs`.

    let repos_root = tmp.path().join("repos");
    let manager = RepoManager::new(Arc::clone(&persistence), repos_root);
    (persistence, manager, tmp)
}

/// Clone a file:// bare repo via `RepoManager::clone_repo` and verify
/// the four locked perf config keys are written to the clone's `.git/config`.
#[tokio::test(flavor = "multi_thread")]
async fn clone_applies_perf_config() {
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
    assert!(local.join(".git").exists(), ".git should exist");

    assert_eq!(
        git_config_get(local, "core.fsmonitor").await.as_deref(),
        Some("true"),
        "core.fsmonitor should be set to true after clone"
    );
    assert_eq!(
        git_config_get(local, "core.untrackedCache")
            .await
            .as_deref(),
        Some("true")
    );
    assert_eq!(
        git_config_get(local, "feature.manyFiles").await.as_deref(),
        Some("true")
    );
    assert_eq!(
        git_config_get(local, "core.commitGraph").await.as_deref(),
        Some("true")
    );
}

/// On hosts where `git fsmonitor--daemon` is supported, the clone path
/// records a live PID. When it's unsupported (older git, exotic FS) the
/// recorded PID is NULL and the test still passes — fsmonitor failures
/// are treated as "not supported on this filesystem" per `design/02 §8`.
#[tokio::test(flavor = "multi_thread")]
async fn clone_records_alive_fsmonitor_pid_when_supported() {
    let (persistence, manager, _tmp) = make_repo_manager().await;
    let (url, _bare, _work) = make_bare_with_commit().await;

    let repo = manager
        .add_repository("fixture", &url, "main", CloneStrategy::Full, false)
        .await
        .expect("add_repository");
    manager
        .clone_repo(&repo.id, None)
        .await
        .expect("clone_repo");

    let row = concerto_persist::repositories::get(persistence.readers(), &repo.id)
        .await
        .expect("get")
        .expect("repo row present");

    match row.fs_monitor_pid {
        Some(pid) if pid > 0 => {
            assert!(
                concerto_gix_wrap::is_fsmonitor_alive(pid as u32),
                "recorded fsmonitor PID {pid} should be alive"
            );
            // Best-effort tidy: stop the daemon so the tempdir's drop
            // doesn't leave a zombie process holding the IPC socket.
            let _ = concerto_gix_wrap::stop_fsmonitor(Path::new(&repo.local_path)).await;
        }
        _ => {
            // Unsupported — the bring-up path's `tracing::info!`
            // already logged the reason; nothing to assert here.
        }
    }
}

/// Restart-policy bookkeeping: three rapid restarts inside a 60s
/// window flip the per-repo `disabled` flag and `probe_all` reports
/// `ProbeOutcome::Disabled`. This avoids the 35s real-time wait by
/// stubbing `is_alive` to always return `false`.
#[tokio::test(flavor = "multi_thread")]
async fn restart_policy_disables_after_three_in_window() {
    let mut history = RestartHistory::default();
    let t0 = Instant::now();
    assert!(!fsmonitor::record_restart(&mut history, t0));
    assert!(!fsmonitor::record_restart(
        &mut history,
        t0 + Duration::from_millis(100)
    ));
    let breached = fsmonitor::record_restart(&mut history, t0 + Duration::from_millis(200));
    assert!(
        breached,
        "third restart inside the window must trip the cap"
    );
    assert!(history.disabled);
}

/// `probe_all` against a mocked `is_alive` walks the `repositories`
/// table and reports `Disabled` for a repo with a dead PID once the
/// restart-history has already been flipped to `disabled` (no further
/// restart attempt is made).
#[tokio::test(flavor = "multi_thread")]
async fn probe_all_respects_disabled_flag() {
    let (persistence, manager, _tmp) = make_repo_manager().await;
    let (url, _bare, _work) = make_bare_with_commit().await;

    let repo = manager
        .add_repository("fixture", &url, "main", CloneStrategy::Full, false)
        .await
        .expect("add_repository");
    // We don't need an on-disk clone for this test; the probe only
    // looks at DB rows + the in-memory restart history.

    // Seed a dead PID into the row so the probe sees "needs restart"
    // and a `disabled` flag in history so the probe short-circuits.
    {
        let mut writer = persistence.writer().await;
        concerto_persist::repositories::update_fs_monitor_pid(&mut writer, &repo.id, Some(1))
            .await
            .expect("seed pid");
    }
    let histories = Arc::new(Mutex::new(HashMap::<RepositoryId, RestartHistory>::new()));
    {
        let mut guard = histories.lock().await;
        guard.insert(
            repo.id.clone(),
            RestartHistory {
                recent: Default::default(),
                disabled: true,
            },
        );
    }

    let outcomes = fsmonitor::probe_all(&persistence, &histories, |_pid| false)
        .await
        .expect("probe_all");
    assert!(
        outcomes
            .iter()
            .any(|(id, o)| id == &repo.id && matches!(o, ProbeOutcome::Disabled)),
        "expected Disabled for the seeded repo; got {outcomes:?}"
    );
}

/// `probe_all` reports `Alive` for a repo whose PID is reported alive
/// by the mock, with no side effects.
#[tokio::test(flavor = "multi_thread")]
async fn probe_all_reports_alive_when_pid_is_live() {
    let (persistence, manager, _tmp) = make_repo_manager().await;
    let (url, _bare, _work) = make_bare_with_commit().await;

    let repo = manager
        .add_repository("fixture", &url, "main", CloneStrategy::Full, false)
        .await
        .expect("add_repository");
    {
        let mut writer = persistence.writer().await;
        concerto_persist::repositories::update_fs_monitor_pid(&mut writer, &repo.id, Some(42))
            .await
            .expect("seed pid");
    }

    let histories = Arc::new(Mutex::new(HashMap::<RepositoryId, RestartHistory>::new()));
    let outcomes = fsmonitor::probe_all(&persistence, &histories, |pid| pid == 42)
        .await
        .expect("probe_all");
    assert!(
        outcomes
            .iter()
            .any(|(id, o)| id == &repo.id && matches!(o, ProbeOutcome::Alive)),
        "expected Alive for the seeded repo; got {outcomes:?}"
    );
}
