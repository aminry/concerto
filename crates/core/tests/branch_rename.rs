//! Task 312: the branch-rename hook + the `OneShotLlm` seam.
//!
//! In-process tests against a real `WorkareaManager` over a tempdir DB. Each
//! repo is a real git clone (bare remote + worktrees), so the cross-repo
//! `git branch -m` loop, the remote-conflict skip + `-N` suffix
//! (`design/03 §8`), and the partial-success contract run against actual git —
//! no agent host, no network (`file://` remotes).
//!
//! Covers (`tasks/v1.0/312` Verification 4):
//! - `rename_workarea_branch` renames every repo's branch in a 2-repo workarea
//!   + updates `workareas.branch_name` + broadcasts `BranchRenamed`;
//! - a repo whose remote already has `new` (different content) is skipped +
//!   suffixed `-N` while its sibling renames cleanly (partial success);
//! - `suggest_workarea_branch_name` returns the deterministic kebab-case slug
//!   (the LIVE Phase-3 path, D1) and honors an injected `branch_rename` pref.
//!
//! The deterministic-slug + `compose_action_prompt` unit coverage lives in
//! `crates/core/src/llm/oneshot.rs`; this file covers the manager wiring.

#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;

use concerto_core::repo_manager::RepoManager;
use concerto_core::workspace_manager::{RepoRenameOutcome, WorkareaEvent, WorkareaManager};
use concerto_persist::{Persistence, PersistenceConfig, RepositoryId};
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
        .env("GIT_TERMINAL_PROMPT", "0")
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

/// A repo on disk: a bare remote + a clone at `local_path`. The clone is the
/// `repositories.local_path` the workarea materializes worktrees from.
struct RepoOnDisk {
    bare: TempDir,
    local_path: std::path::PathBuf,
}

/// Clone a fresh bare-backed repo into `<data>/repos/<repo_id>`.
async fn make_repo(data_dir: &Path, repo_id: &str) -> RepoOnDisk {
    let bare = TempDir::new().unwrap();
    let work = TempDir::new().unwrap();
    git(&["init", "--bare", "-b", "main", "."], bare.path()).await;
    git(&["init", "-b", "main", "."], work.path()).await;
    tokio::fs::write(work.path().join("README.md"), "hello\n")
        .await
        .unwrap();
    git(&["add", "README.md"], work.path()).await;
    git(&["commit", "-m", "initial"], work.path()).await;
    let url = format!("file://{}", bare.path().display());
    git(&["remote", "add", "origin", url.as_str()], work.path()).await;
    git(&["push", "-u", "origin", "main"], work.path()).await;

    let local_path = data_dir.join("repos").join(repo_id);
    tokio::fs::create_dir_all(local_path.parent().unwrap())
        .await
        .unwrap();
    git(
        &["clone", url.as_str(), &local_path.to_string_lossy()],
        Path::new("."),
    )
    .await;
    RepoOnDisk { bare, local_path }
}

struct Fixture {
    _tmp: TempDir,
    persistence: Arc<Persistence>,
    mgr: WorkareaManager,
    repos: Vec<RepoOnDisk>,
    repo_ids: Vec<String>,
    workspace_id: String,
}

/// Build a manager + a workspace with `n` real repos attached (position order).
async fn make_fixture(n: usize) -> Fixture {
    let tmp = TempDir::new().unwrap();
    let data_dir = tmp.path().join("data");
    tokio::fs::create_dir_all(&data_dir).await.unwrap();
    let db_path = data_dir.join("concerto.db");
    let persistence = Arc::new(
        Persistence::open(PersistenceConfig {
            db_path,
            max_readers: 2,
        })
        .await
        .expect("open"),
    );

    let mut repos = Vec::new();
    let mut repo_ids = Vec::new();
    {
        let mut w = persistence.writer().await;
        sqlx::query(
            "INSERT INTO workspaces (id, name, slug, created_at)
             VALUES ('ws', 'ws', 'ws', 0)",
        )
        .execute(&mut *w)
        .await
        .expect("workspace");
    }
    for i in 0..n {
        let repo_id = format!("r{i}");
        let repo = make_repo(&data_dir, &repo_id).await;
        let url = format!("file://{}", repo.bare.path().display());
        let mut w = persistence.writer().await;
        sqlx::query(
            "INSERT INTO repositories (id, name, url, local_path, clone_strategy, default_branch)
             VALUES (?, ?, ?, ?, 'full', 'main')",
        )
        .bind(&repo_id)
        .bind(&repo_id)
        .bind(&url)
        .bind(repo.local_path.to_string_lossy().to_string())
        .execute(&mut *w)
        .await
        .expect("repo");
        sqlx::query(
            "INSERT INTO workspace_repos (workspace_id, repository_id, position) VALUES ('ws', ?, ?)",
        )
        .bind(&repo_id)
        .bind(i as i64)
        .execute(&mut *w)
        .await
        .expect("workspace_repos");
        repos.push(repo);
        repo_ids.push(repo_id);
    }

    let repo_manager = RepoManager::new(Arc::clone(&persistence), data_dir.join("repos"));
    let mgr = WorkareaManager::new(
        Arc::clone(&persistence),
        repo_manager,
        Arc::new(data_dir),
        Arc::new(tmp.path().join("config")),
    );

    Fixture {
        _tmp: tmp,
        persistence,
        mgr,
        repos,
        repo_ids,
        workspace_id: "ws".to_string(),
    }
}

/// The local branch names present in `worktree`'s repo.
async fn local_branches(worktree: &Path) -> Vec<String> {
    let out = Command::new("git")
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(worktree)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .await
        .expect("git branch");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn rename_renames_every_repo_and_updates_branch_name() {
    let fx = make_fixture(2).await;
    let mut rx = fx.mgr.subscribe();

    let wa = fx
        .mgr
        .create_workarea(&fx.workspace_id, None)
        .await
        .expect("create");
    assert_eq!(wa.status, "active", "both repos materialized");
    let old_branch = wa.branch_name.clone();

    let report = fx
        .mgr
        .rename_workarea_branch(&wa.id, "feat/idempotency-keys")
        .await
        .expect("rename");

    assert_eq!(report.branch_name, "feat/idempotency-keys");
    assert_eq!(report.renamed_count(), 2, "both repos renamed cleanly");
    assert_eq!(report.skipped_count(), 0);
    assert!(report
        .steps
        .iter()
        .all(|s| matches!(s.outcome, RepoRenameOutcome::Renamed)));

    // `workareas.branch_name` updated.
    let refreshed = fx.mgr.get(&wa.id).await.unwrap().unwrap();
    assert_eq!(refreshed.branch_name, "feat/idempotency-keys");

    // Each repo's worktree is actually on the new branch (old gone).
    let repos = concerto_persist::workareas::list_workarea_repos(fx.persistence.readers(), &wa.id)
        .await
        .unwrap();
    for (_rid, wt) in &repos {
        let branches = local_branches(Path::new(wt)).await;
        assert!(
            branches.contains(&"feat/idempotency-keys".to_string()),
            "worktree should be on the new branch; got {branches:?}"
        );
        assert!(
            !branches.contains(&old_branch),
            "old branch should be gone; got {branches:?}"
        );
    }

    // BranchRenamed broadcast.
    let mut saw = false;
    while let Ok(ev) = rx.try_recv() {
        if let WorkareaEvent::BranchRenamed {
            id,
            to,
            renamed,
            skipped,
            ..
        } = ev
        {
            assert_eq!(id, wa.id);
            assert_eq!(to, "feat/idempotency-keys");
            assert_eq!(renamed, 2);
            assert_eq!(skipped, 0);
            saw = true;
        }
    }
    assert!(saw, "expected a BranchRenamed event");
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_conflict_repo_is_skipped_and_suffixed_while_sibling_renames() {
    let fx = make_fixture(2).await;

    // Push a DIFFERENT-content branch named `feature/x` to repo r0's remote,
    // so when the rename targets `feature/x` r0 sees a remote conflict.
    let r0 = &fx.repos[0];
    let r0_url = format!("file://{}", r0.bare.path().display());
    let conflict_work = TempDir::new().unwrap();
    git(
        &[
            "clone",
            r0_url.as_str(),
            &conflict_work.path().to_string_lossy(),
        ],
        Path::new("."),
    )
    .await;
    git(&["checkout", "-b", "feature/x"], conflict_work.path()).await;
    tokio::fs::write(conflict_work.path().join("OTHER.md"), "different\n")
        .await
        .unwrap();
    git(&["add", "OTHER.md"], conflict_work.path()).await;
    git(&["commit", "-m", "other work"], conflict_work.path()).await;
    git(&["push", "-u", "origin", "feature/x"], conflict_work.path()).await;

    // Make r0's clone aware of the remote `feature/x` ref (the remote-tracking
    // ref `list_branches` reads for the conflict check).
    git(&["fetch", "origin"], &r0.local_path).await;

    let wa = fx
        .mgr
        .create_workarea(&fx.workspace_id, None)
        .await
        .expect("create");

    let report = fx
        .mgr
        .rename_workarea_branch(&wa.id, "feature/x")
        .await
        .expect("rename");

    // Workarea-level name is the requested name.
    assert_eq!(report.branch_name, "feature/x");
    // One sibling renamed cleanly; the conflicting repo was skipped + suffixed.
    assert_eq!(report.renamed_count(), 1, "the sibling repo renamed");
    assert_eq!(
        report.skipped_count(),
        1,
        "the conflicting repo was skipped"
    );

    // Find the per-repo outcomes by repository_id (r0 is the conflicting one).
    let r0_id = &fx.repo_ids[0];
    let r1_id = &fx.repo_ids[1];
    let r0_step = report
        .steps
        .iter()
        .find(|s| &s.repository_id == r0_id)
        .expect("r0 step");
    let r1_step = report
        .steps
        .iter()
        .find(|s| &s.repository_id == r1_id)
        .expect("r1 step");

    match &r0_step.outcome {
        RepoRenameOutcome::SkippedRemoteConflict { actual } => {
            assert_eq!(actual, "feature/x-2", "suffixed per design/03 §8");
        }
        other => panic!("expected r0 skipped+suffixed; got {other:?}"),
    }
    assert!(matches!(r1_step.outcome, RepoRenameOutcome::Renamed));

    // r0's worktree is on the suffixed branch; r1's is on the requested name.
    let r0_wt = concerto_persist::workareas::get_workarea_repo_worktree_path(
        fx.persistence.readers(),
        &wa.id,
        &RepositoryId(r0_id.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    let r1_wt = concerto_persist::workareas::get_workarea_repo_worktree_path(
        fx.persistence.readers(),
        &wa.id,
        &RepositoryId(r1_id.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(local_branches(Path::new(&r0_wt))
        .await
        .contains(&"feature/x-2".to_string()));
    assert!(local_branches(Path::new(&r1_wt))
        .await
        .contains(&"feature/x".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn suggest_uses_deterministic_slug() {
    let fx = make_fixture(1).await;
    let wa = fx
        .mgr
        .create_workarea(&fx.workspace_id, None)
        .await
        .expect("create");

    let name = fx
        .mgr
        .suggest_workarea_branch_name(&wa.id, "Add idempotency keys to the payments endpoint")
        .await
        .expect("suggest");
    // The LIVE deterministic path (D1): kebab-case slug from the message.
    assert_eq!(name, "add-idempotency-keys-to-the-payments-endpoint");
}

#[tokio::test(flavor = "multi_thread")]
async fn suggest_honors_checked_in_branch_rename_pref() {
    let fx = make_fixture(1).await;

    // Drop a checked-in `.concerto/action_prefs.toml` with a ticket-prefix
    // branch_rename pref into the reference repo (the resolver reads it).
    let concerto = fx.repos[0].local_path.join(".concerto");
    tokio::fs::create_dir_all(&concerto).await.unwrap();
    tokio::fs::write(
        concerto.join("action_prefs.toml"),
        "branch_rename = \"kebab-case with the Linear ticket prefix when one exists.\"\n",
    )
    .await
    .unwrap();

    let wa = fx
        .mgr
        .create_workarea(&fx.workspace_id, None)
        .await
        .expect("create");

    let name = fx
        .mgr
        .suggest_workarea_branch_name(&wa.id, "Fix the flaky retry in CON-451 checkout flow")
        .await
        .expect("suggest");
    assert!(
        name.starts_with("con-451-"),
        "expected the Linear ticket prefix; got {name}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rename_to_same_name_is_a_noop() {
    let fx = make_fixture(1).await;
    let wa = fx
        .mgr
        .create_workarea(&fx.workspace_id, None)
        .await
        .expect("create");

    let report = fx
        .mgr
        .rename_workarea_branch(&wa.id, &wa.branch_name)
        .await
        .expect("rename");
    assert_eq!(report.branch_name, wa.branch_name);
    assert_eq!(report.renamed_count(), 1);
}
