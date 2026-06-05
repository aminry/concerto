//! Integration tests for the Task 45 VCS Provider (`gh` CLI shell-out).
//!
//! Every test mocks `gh` by writing a tiny shell script into a tempdir
//! and prepending that tempdir to `PATH`. The script inspects `argv`
//! and prints canned JSON / non-zero exits to exercise:
//!
//! 1. `gh auth status` success / failure → `Ok(())` vs
//!    `Error::VcsNotAuthenticated`.
//! 2. `gh pr create` round-trip: create returns a URL, view returns the
//!    full JSON, the cache row is upserted.
//! 3. `gh api …/check-runs` parses the `--jq` line-delimited output.
//!
//! Real-GitHub end-to-end is the operator's job per the task spec; this
//! suite verifies the shell-out glue without leaving the test sandbox.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

use concerto_core::vcs::{gh_cli, VcsHandle};
use concerto_error::Error;
use concerto_persist::{
    NewProject, NewRepository, NewWorkarea, NewWorkareaRepo, NewWorkspace, Persistence,
    PersistenceConfig, ProjectId, RepositoryId, WorkareaId, WorkspaceId,
};
use tempfile::TempDir;

/// Write a mock `gh` script into `dir` and return the dir for `PATH`
/// injection. The script is dispatch-by-first-arg; subcases live as
/// inline cases.
fn install_mock_gh(dir: &std::path::Path, script_body: &str) -> PathBuf {
    let gh_path = dir.join("gh");
    std::fs::write(&gh_path, script_body).expect("write mock gh");
    let mut perms = std::fs::metadata(&gh_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&gh_path, perms).unwrap();
    gh_path
}

async fn make_persistence(tmp: &TempDir) -> Arc<Persistence> {
    let data = tmp.path().join("data");
    tokio::fs::create_dir_all(&data).await.unwrap();
    let cfg = PersistenceConfig {
        db_path: data.join("concerto.db"),
        max_readers: 2,
    };
    Arc::new(Persistence::open(cfg).await.expect("open persistence"))
}

/// Seed the foreign keys needed for a `pull_requests` row: project,
/// repository, workspace, workspace_repos, workarea, workarea_repos.
async fn seed_workarea(persist: &Persistence) -> (WorkareaId, RepositoryId) {
    let project_id = ProjectId(format!("proj-{}", uuid::Uuid::now_v7()));
    let repo_id = RepositoryId(format!("repo-{}", uuid::Uuid::now_v7()));
    let workspace_id = WorkspaceId(format!("ws-{}", uuid::Uuid::now_v7()));
    let workarea_id = WorkareaId(format!("wa-{}", uuid::Uuid::now_v7()));
    let mut writer = persist.writer().await;
    concerto_persist::projects::insert(
        &mut writer,
        NewProject {
            id: project_id.clone(),
            name: "vcs-test".into(),
            icon: None,
            created_at: 1,
        },
    )
    .await
    .unwrap();
    concerto_persist::repositories::insert(
        &mut writer,
        NewRepository {
            id: repo_id.clone(),
            project_id: project_id.0.clone(),
            name: "repo".into(),
            url: "https://github.com/owner/repo".into(),
            local_path: "/tmp/repo".into(),
            clone_strategy: "full".into(),
            default_branch: "main".into(),
        },
    )
    .await
    .unwrap();
    concerto_persist::workspaces::insert(
        &mut writer,
        NewWorkspace {
            id: workspace_id.clone(),
            project_id: project_id.0.clone(),
            name: "vcs-ws".into(),
            slug: "vcs-ws".into(),
            description: None,
            permission_mode: None,
            created_at: 1,
        },
    )
    .await
    .unwrap();
    concerto_persist::workspaces::update_repos(
        &mut writer,
        &workspace_id,
        std::slice::from_ref(&repo_id),
    )
    .await
    .unwrap();
    concerto_persist::workareas::insert(
        &mut writer,
        NewWorkarea {
            id: workarea_id.clone(),
            workspace_id: workspace_id.0.clone(),
            composer_name: "bach".into(),
            branch_name: "concerto/bach".into(),
            worktree_root: "/tmp/wa".into(),
            status: "active".into(),
            permission_mode: None,
            created_at: 1,
        },
    )
    .await
    .unwrap();
    concerto_persist::workareas::insert_workarea_repo(
        &mut writer,
        NewWorkareaRepo {
            workarea_id: workarea_id.clone(),
            repository_id: repo_id.clone(),
            worktree_path: "/tmp/wa/repo".into(),
            branch_override: None,
            // Task 302: default-empty cone set.
            sparse_cones_json: NewWorkareaRepo::empty_cones(),
        },
    )
    .await
    .unwrap();
    (workarea_id, repo_id)
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_status_success_returns_ok() {
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let gh = install_mock_gh(
        &bin_dir,
        // Auth success: exit 0, no stderr.
        "#!/bin/sh\nif [ \"$1\" = \"auth\" ]; then exit 0; fi\nexit 1\n",
    );
    gh_cli::check_auth(&gh).await.expect("auth status ok");
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_status_failure_returns_typed_error() {
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let gh = install_mock_gh(
        &bin_dir,
        // Auth failure: exit 1, stderr carries the canonical phrase.
        "#!/bin/sh\nif [ \"$1\" = \"auth\" ]; then echo 'You are not authenticated' 1>&2; exit 1; fi\nexit 0\n",
    );
    let err = gh_cli::check_auth(&gh).await.expect_err("auth must fail");
    match err {
        Error::VcsNotAuthenticated(msg) => {
            assert!(msg.contains("not authenticated"), "stderr leaked: {msg}");
            assert!(msg.contains("gh auth login"), "remediation hint: {msg}");
        }
        other => panic!("expected VcsNotAuthenticated, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn create_pr_round_trip_persists_cache_row() {
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    // Mock script: handles `pr create` (prints PR URL) and `pr view`
    // (prints PR JSON).
    let script = r#"#!/bin/sh
case "$1 $2" in
  "pr create")
    # Print the URL to stdout; gh real behavior.
    echo 'https://github.com/owner/repo/pull/42'
    exit 0
    ;;
  "pr view")
    cat <<EOF
{
  "number": 42,
  "title": "feat: add thing",
  "body": "Closes #1",
  "state": "OPEN",
  "url": "https://github.com/owner/repo/pull/42",
  "headRefName": "feature/x",
  "baseRefName": "main",
  "headRefOid": "deadbeef"
}
EOF
    exit 0
    ;;
  "auth status")
    exit 0
    ;;
esac
exit 1
"#;
    let gh = install_mock_gh(&bin_dir, script);

    let persist = make_persistence(&tmp).await;
    let (workarea_id, repo_id) = seed_workarea(&persist).await;

    // Build a handle that bypasses PATH resolution by pre-seeding the
    // `OnceCell` via the public API. We invoke `gh` paths directly to
    // confirm the shell-out wiring works, then the handle's
    // `create_pr` to confirm the cache upsert.
    let handle = VcsHandle::new(Arc::clone(&persist));
    // Force the cached path via the public helper: set PATH so
    // `resolve_gh_path()` finds our mock, then trigger `gh()`.
    let old_path = std::env::var_os("PATH");
    let new_path = match &old_path {
        Some(p) => {
            let mut entries: Vec<std::ffi::OsString> = vec![bin_dir.clone().into()];
            entries.extend(std::env::split_paths(p).map(Into::into));
            std::env::join_paths(entries).unwrap()
        }
        None => bin_dir.clone().into_os_string(),
    };
    // SAFETY: this test is single-threaded inside the tokio runtime,
    // and no other thread reads PATH while we mutate it.
    unsafe {
        std::env::set_var("PATH", &new_path);
    }
    let resolved = handle.gh().await.expect("resolve gh");
    assert_eq!(resolved, gh.as_path(), "mock gh must win the PATH walk");

    let pr = handle
        .create_pr(
            &workarea_id,
            &repo_id,
            "main",
            "feature/x",
            "feat: add thing",
            "Closes #1",
        )
        .await
        .expect("create_pr");
    assert_eq!(pr.pr_number, 42);
    assert_eq!(pr.state, "open");
    assert_eq!(pr.head_sha, "deadbeef");
    assert_eq!(pr.title, "feat: add thing");

    // Cache: the row should be listable via the workarea.
    let rows = concerto_persist::pull_requests::list_by_workarea(persist.readers(), &workarea_id)
        .await
        .expect("list_by_workarea");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].pr_number, 42);
    assert_eq!(rows[0].url, "https://github.com/owner/repo/pull/42");

    // Restore PATH.
    unsafe {
        match old_path {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn check_runs_parses_jq_line_delimited_output() {
    let tmp = TempDir::new().unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let script = r#"#!/bin/sh
if [ "$1" = "api" ]; then
    cat <<EOF
{"name": "build", "status": "completed", "conclusion": "success", "details_url": "https://example.com/1"}
{"name": "test", "status": "completed", "conclusion": "failure", "details_url": "https://example.com/2"}
EOF
    exit 0
fi
exit 1
"#;
    let gh = install_mock_gh(&bin_dir, script);
    let runs = gh_cli::get_check_runs(&gh, "owner/repo", "deadbeef")
        .await
        .expect("get_check_runs");
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].name, "build");
    assert_eq!(runs[0].conclusion, "success");
    assert_eq!(runs[1].name, "test");
    assert_eq!(runs[1].conclusion, "failure");
}
