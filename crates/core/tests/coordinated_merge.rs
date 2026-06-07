//! Task 320 — coordinated PR-set merge loop + coordinated revert (Tier 2).
//!
//! Exercises [`WorkareaManager::merge_workarea_pr_set`] /
//! [`WorkareaManager::revert_workarea_pr_set`] /
//! [`WorkareaManager::get_workarea_merge_plan`] in-process against:
//!
//! - a **scripted [`PrSetVcs`] double** (`FakeMerger`) — returns chosen
//!   `MergeReport`s (with a per-PR post-merge merge-commit SHA) + `RevertReport`s
//!   without spinning up `gh`/octocrab, so the loop's ordering / pause-on-fail /
//!   reverse-order revert / override logic is provable deterministically; and
//! - a real [`SchedulerHandle`] wired to a **scripted [`CheckRunsSource`]**
//!   (`ScriptedChecks`) keyed by the merge SHA — so `wait_for_check_runs` resolves
//!   pass / fail / timeout from a recorded check-run sequence.
//!
//! One test additionally drives the NAMED Tier-2 double, the `concerto-vcs`
//! `testkit` **`FakeGitHub`**, through the real `GitHubProvider::merge_pr` to
//! prove the loop consumes a real `MergeReport` carrying the mocked merge-commit
//! SHA (`design/13 §7.2`).
//!
//! What this does NOT cover (→ the Phase-3 Tier-3 checklist line): a real GitHub
//! coordinated PR-set merge against a live repo with a live webhook, real
//! merge-commit SHAs, and real check-run propagation latency.

#![cfg(unix)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use concerto_core::repo_manager::RepoManager;
use concerto_core::scheduler::wait_checks::{CheckRunSnapshot, CheckRunsSource};
use concerto_core::scheduler::SchedulerHandle;
use concerto_core::workspace_manager::{
    FailureKind, MergeOpts, MergeProgress, PrSetVcs, RevertOpts, RevertOutcome, WorkareaEvent,
    WorkareaManager,
};
use concerto_error::{Error, Result};
use concerto_persist::{
    pull_requests, NewProject, NewPullRequest, NewRepository, NewWorkarea, NewWorkspace,
    Persistence, PersistenceConfig, ProjectId, PullRequestId, RepositoryId, WorkareaId,
    WorkspaceId,
};
use concerto_vcs::provider::{
    MergeMethod, MergeReport as ProviderMergeReport, RevertReport as ProviderRevertReport,
};

// ---------------------------------------------------------------------------
// Scripted doubles
// ---------------------------------------------------------------------------

/// Scripted single-PR merge/revert double. Keyed by `(repo_full_name, pr_number)`
/// it returns a chosen `MergeReport` (with a merge-commit SHA) / `RevertReport`,
/// and records every merge call so a test can assert ordering / non-merge.
#[derive(Default)]
struct FakeMerger {
    /// `(repo_full, pr_number)` → merge SHA to return.
    merge_sha: HashMap<(String, i64), String>,
    /// `(repo_full, pr_number)` → revert outcome (`reverted` bool).
    revert_ok: HashMap<(String, i64), bool>,
    /// PRs whose `merge_pr` should error (simulate a conflict / 405).
    merge_err: HashMap<(String, i64), String>,
    /// Recorded merge calls, in order.
    merged: Mutex<Vec<(String, i64)>>,
    /// Recorded revert calls, in order.
    reverted: Mutex<Vec<(String, i64)>>,
}

impl FakeMerger {
    fn with_merge(mut self, repo_full: &str, pr: i64, sha: &str) -> Self {
        self.merge_sha
            .insert((repo_full.to_string(), pr), sha.to_string());
        self
    }
    fn with_merge_err(mut self, repo_full: &str, pr: i64, msg: &str) -> Self {
        self.merge_err
            .insert((repo_full.to_string(), pr), msg.to_string());
        self
    }
    fn with_revert(mut self, repo_full: &str, pr: i64, ok: bool) -> Self {
        self.revert_ok.insert((repo_full.to_string(), pr), ok);
        self
    }
    fn merged_order(&self) -> Vec<(String, i64)> {
        self.merged.lock().unwrap().clone()
    }
    fn reverted_order(&self) -> Vec<(String, i64)> {
        self.reverted.lock().unwrap().clone()
    }
}

#[async_trait]
impl PrSetVcs for FakeMerger {
    async fn merge_pr(
        &self,
        _repository_id: &RepositoryId,
        repository_full_name: &str,
        pr_number: i64,
        _method: MergeMethod,
    ) -> Result<ProviderMergeReport> {
        let key = (repository_full_name.to_string(), pr_number);
        if let Some(msg) = self.merge_err.get(&key) {
            return Err(Error::Vcs(msg.clone()));
        }
        self.merged.lock().unwrap().push(key.clone());
        let sha = self
            .merge_sha
            .get(&key)
            .cloned()
            .unwrap_or_else(|| format!("merge-{}-{}", repository_full_name, pr_number));
        Ok(ProviderMergeReport {
            merged: true,
            merge_commit_sha: Some(sha),
            message: "merged".into(),
        })
    }

    async fn revert_pr(
        &self,
        _repository_id: &RepositoryId,
        repository_full_name: &str,
        pr_number: i64,
        _hard_reset: bool,
    ) -> Result<ProviderRevertReport> {
        let key = (repository_full_name.to_string(), pr_number);
        self.reverted.lock().unwrap().push(key.clone());
        let ok = self.revert_ok.get(&key).copied().unwrap_or(true);
        Ok(ProviderRevertReport {
            reverted: ok,
            revert_pr_url: ok
                .then(|| format!("https://github.com/{repository_full_name}/pull/999")),
            message: if ok {
                "reverted".into()
            } else {
                "revert failed".into()
            },
        })
    }
}

/// Scripted check-runs source keyed by SHA. A SHA mapped to `Some(snapshots)`
/// returns those runs (terminal-pass / terminal-fail); a SHA mapped to `None`
/// (or absent) returns one perpetually-pending run so `wait_for_check_runs`
/// times out.
struct ScriptedChecks {
    by_sha: HashMap<String, Vec<CheckRunSnapshot>>,
}

impl ScriptedChecks {
    fn new() -> Self {
        Self {
            by_sha: HashMap::new(),
        }
    }
    fn pass(mut self, sha: &str) -> Self {
        self.by_sha.insert(
            sha.to_string(),
            vec![CheckRunSnapshot {
                name: "ci".into(),
                status: "completed".into(),
                conclusion: "success".into(),
            }],
        );
        self
    }
    fn fail(mut self, sha: &str) -> Self {
        self.by_sha.insert(
            sha.to_string(),
            vec![CheckRunSnapshot {
                name: "ci".into(),
                status: "completed".into(),
                conclusion: "failure".into(),
            }],
        );
        self
    }
}

#[async_trait]
impl CheckRunsSource for ScriptedChecks {
    async fn check_runs(&self, _repo: &RepositoryId, sha: &str) -> Result<Vec<CheckRunSnapshot>> {
        match self.by_sha.get(sha) {
            Some(runs) => Ok(runs.clone()),
            // Unknown SHA → a perpetually-pending run (never terminal) → timeout.
            None => Ok(vec![CheckRunSnapshot {
                name: "ci".into(),
                status: "in_progress".into(),
                conclusion: String::new(),
            }]),
        }
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Ctx {
    _dir: tempfile::TempDir,
    persist: Arc<Persistence>,
    manager: WorkareaManager,
    workarea_id: WorkareaId,
    repos: Vec<RepositoryId>,
    config_dir: std::path::PathBuf,
}

async fn setup(repo_ids: &[&str]) -> Ctx {
    let dir = tempfile::tempdir().expect("tempdir");
    let persist = Arc::new(
        Persistence::open(PersistenceConfig {
            db_path: dir.path().join("test.db"),
            max_readers: 2,
        })
        .await
        .expect("open"),
    );

    let project_id = ProjectId("proj-1".to_string());
    let workspace_id = WorkspaceId("ws-1".to_string());
    let workarea_id = WorkareaId("wa-1".to_string());
    let repos: Vec<RepositoryId> = repo_ids
        .iter()
        .map(|r| RepositoryId(r.to_string()))
        .collect();

    {
        let mut w = persist.writer().await;
        concerto_persist::projects::insert(
            &mut w,
            NewProject {
                id: project_id.clone(),
                name: "Test".into(),
                icon: None,
                created_at: 1,
            },
        )
        .await
        .unwrap();
        for r in &repos {
            concerto_persist::repositories::insert(
                &mut w,
                NewRepository {
                    id: r.clone(),
                    project_id: project_id.0.clone(),
                    name: r.0.clone(),
                    url: format!("https://github.com/acme/{}", r.0),
                    local_path: format!("/tmp/{}", r.0),
                    clone_strategy: "full".into(),
                    default_branch: "main".into(),
                },
            )
            .await
            .unwrap();
        }
        concerto_persist::workspaces::insert(
            &mut w,
            NewWorkspace {
                id: workspace_id.clone(),
                project_id: project_id.0.clone(),
                name: "WS".into(),
                slug: "ws".into(),
                description: None,
                permission_mode: None,
                created_at: 1,
            },
        )
        .await
        .unwrap();
        concerto_persist::workareas::insert(
            &mut w,
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
    }

    let repo_manager = RepoManager::new(Arc::clone(&persist), dir.path().join("repos"));
    let config_dir = dir.path().join("config");
    let manager = WorkareaManager::new(
        Arc::clone(&persist),
        repo_manager,
        Arc::new(dir.path().join("data")),
        Arc::new(config_dir.clone()),
    );

    Ctx {
        _dir: dir,
        persist,
        manager,
        workarea_id,
        repos,
        config_dir,
    }
}

fn new_pr(
    workarea_id: &WorkareaId,
    repository_id: &RepositoryId,
    pr_number: i64,
    merge_order: i64,
) -> NewPullRequest {
    NewPullRequest {
        id: PullRequestId(uuid::Uuid::now_v7().to_string()),
        workarea_id: workarea_id.clone(),
        repository_id: repository_id.clone(),
        provider: "github".into(),
        pr_number,
        base_ref: "main".into(),
        head_ref: "feature".into(),
        state: "open".into(),
        title: "T".into(),
        body: String::new(),
        url: String::new(),
        head_sha: format!("head-{}-{}", repository_id.0, pr_number),
        merge_order,
        external_id: String::new(),
        repository_full_name: format!("acme/{}", repository_id.0),
        created_at: 1,
        updated_at: 1,
    }
}

async fn seed_pr(ctx: &Ctx, repo: &RepositoryId, pr: i64, order: i64) {
    let mut w = ctx.persist.writer().await;
    pull_requests::upsert(&mut w, new_pr(&ctx.workarea_id, repo, pr, order))
        .await
        .unwrap();
}

/// Build a manager wired with the given merger + scripted checks (real
/// SchedulerHandle, source injected).
fn wire(ctx: &Ctx, merger: Arc<FakeMerger>, checks: ScriptedChecks) -> WorkareaManager {
    let scheduler = SchedulerHandle::new(Arc::clone(&ctx.persist), None);
    scheduler.set_check_runs_source(Arc::new(checks));
    ctx.manager
        .clone()
        .with_pr_set_vcs(merger)
        .with_scheduler(scheduler)
}

/// Drain a `MergeWorkareaPrSet` run into (the frames, the terminal report).
async fn run_merge(
    manager: &WorkareaManager,
    workarea_id: &WorkareaId,
    opts: MergeOpts,
) -> (
    Vec<MergeProgress>,
    concerto_core::workspace_manager::MergeReport,
) {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<MergeProgress>(64);
    let collector = tokio::spawn(async move {
        let mut frames = Vec::new();
        while let Some(f) = rx.recv().await {
            frames.push(f);
        }
        frames
    });
    let report = manager
        .merge_workarea_pr_set(workarea_id, opts, tx)
        .await
        .expect("merge loop");
    let frames = collector.await.unwrap();
    (frames, report)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ordered_merge_happy_path_all_checks_pass() {
    let ctx = setup(&["repo-a", "repo-b", "repo-c"]).await;
    // Insert OUT of merge_order: orders 2, 0, 1 → expected merge order b, c, a.
    seed_pr(&ctx, &ctx.repos[0], 10, 2).await;
    seed_pr(&ctx, &ctx.repos[1], 20, 0).await;
    seed_pr(&ctx, &ctx.repos[2], 30, 1).await;

    let merger = Arc::new(
        FakeMerger::default()
            .with_merge("acme/repo-a", 10, "sha-a")
            .with_merge("acme/repo-b", 20, "sha-b")
            .with_merge("acme/repo-c", 30, "sha-c"),
    );
    let checks = ScriptedChecks::new()
        .pass("sha-a")
        .pass("sha-b")
        .pass("sha-c");
    let manager = wire(&ctx, Arc::clone(&merger), checks);

    let mut sub = manager.subscribe();
    let (frames, report) = run_merge(&manager, &ctx.workarea_id, MergeOpts::default()).await;

    assert_eq!(report.total, 3);
    assert_eq!(report.merged_steps, 3);
    assert_eq!(report.paused_at_step, None);

    // Merged in merge_order: b(20), c(30), a(10).
    assert_eq!(
        merger.merged_order(),
        vec![
            ("acme/repo-b".to_string(), 20),
            ("acme/repo-c".to_string(), 30),
            ("acme/repo-a".to_string(), 10),
        ]
    );

    // Final frame is SetMerged{total:3}.
    assert!(matches!(
        frames.last(),
        Some(MergeProgress::SetMerged { total: 3 })
    ));
    // 3 StepStarted + 3 StepCompleted + 1 SetMerged.
    let started = frames
        .iter()
        .filter(|f| matches!(f, MergeProgress::StepStarted { .. }))
        .count();
    let completed = frames
        .iter()
        .filter(|f| matches!(f, MergeProgress::StepCompleted { .. }))
        .count();
    assert_eq!((started, completed), (3, 3));

    // The broadcast saw a PrSetMerged.
    let mut saw_merged = false;
    while let Ok(ev) = sub.try_recv() {
        if matches!(ev, WorkareaEvent::PrSetMerged { total: 3, .. }) {
            saw_merged = true;
        }
    }
    assert!(saw_merged, "PrSetMerged broadcast on full merge");
}

#[tokio::test]
async fn pause_on_fail_stops_loop_and_leaves_later_members_unmerged() {
    let ctx = setup(&["repo-a", "repo-b", "repo-c"]).await;
    seed_pr(&ctx, &ctx.repos[0], 10, 0).await;
    seed_pr(&ctx, &ctx.repos[1], 20, 1).await;
    seed_pr(&ctx, &ctx.repos[2], 30, 2).await;

    let merger = Arc::new(
        FakeMerger::default()
            .with_merge("acme/repo-a", 10, "sha-a")
            .with_merge("acme/repo-b", 20, "sha-b")
            .with_merge("acme/repo-c", 30, "sha-c"),
    );
    // Member 2's checks FAIL; member 3's would pass but must never be reached.
    let checks = ScriptedChecks::new()
        .pass("sha-a")
        .fail("sha-b")
        .pass("sha-c");
    let manager = wire(&ctx, Arc::clone(&merger), checks);

    let (frames, report) = run_merge(&manager, &ctx.workarea_id, MergeOpts::default()).await;

    assert_eq!(report.merged_steps, 1, "only step 1 merged");
    assert_eq!(report.paused_at_step, Some(2));

    // repo-c (step 3) was NEVER merged.
    let merged = merger.merged_order();
    assert!(
        !merged.iter().any(|(r, _)| r == "acme/repo-c"),
        "members after the failed step are not merged: {merged:?}"
    );

    // A StepFailed{step:2, ChecksFailed} + SetPaused frame.
    assert!(frames.iter().any(|f| matches!(
        f,
        MergeProgress::StepFailed {
            step: 2,
            kind: FailureKind::ChecksFailed,
            ..
        }
    )));
    assert!(frames.iter().any(|f| matches!(
        f,
        MergeProgress::SetPaused {
            paused_at_step: 2,
            ..
        }
    )));
}

#[tokio::test]
async fn checks_timeout_pauses_the_loop() {
    let ctx = setup(&["repo-a"]).await;
    seed_pr(&ctx, &ctx.repos[0], 10, 0).await;

    let merger = Arc::new(FakeMerger::default().with_merge("acme/repo-a", 10, "sha-a"));
    // No mapping for "sha-a" → ScriptedChecks returns a perpetually-pending run.
    let checks = ScriptedChecks::new();
    let manager = wire(&ctx, Arc::clone(&merger), checks);

    // Tiny timeout so the wait resolves to a timeout near-instantly.
    let opts = MergeOpts {
        timeout: std::time::Duration::from_millis(1),
        ..MergeOpts::default()
    };
    let (frames, report) = run_merge(&manager, &ctx.workarea_id, opts).await;

    assert_eq!(report.paused_at_step, Some(1));
    assert_eq!(report.merged_steps, 0);
    assert!(frames.iter().any(|f| matches!(
        f,
        MergeProgress::StepFailed {
            kind: FailureKind::ChecksTimeout,
            ..
        }
    )));
}

#[tokio::test]
async fn merge_conflict_pauses_with_conflict_kind() {
    let ctx = setup(&["repo-a"]).await;
    seed_pr(&ctx, &ctx.repos[0], 10, 0).await;

    let merger = Arc::new(FakeMerger::default().with_merge_err(
        "acme/repo-a",
        10,
        "Pull Request is not mergeable",
    ));
    let manager = wire(&ctx, Arc::clone(&merger), ScriptedChecks::new());

    let (frames, report) = run_merge(&manager, &ctx.workarea_id, MergeOpts::default()).await;
    assert_eq!(report.paused_at_step, Some(1));
    assert!(frames.iter().any(|f| matches!(
        f,
        MergeProgress::StepFailed {
            kind: FailureKind::MergeConflict,
            ..
        }
    )));
}

#[tokio::test]
async fn empty_pr_set_is_a_zero_step_success() {
    let ctx = setup(&["repo-a"]).await;
    // No PRs seeded.
    let merger = Arc::new(FakeMerger::default());
    let manager = wire(&ctx, merger, ScriptedChecks::new());

    let (frames, report) = run_merge(&manager, &ctx.workarea_id, MergeOpts::default()).await;
    assert_eq!(report.total, 0);
    assert_eq!(report.merged_steps, 0);
    assert_eq!(report.paused_at_step, None);
    assert!(matches!(
        frames.as_slice(),
        [MergeProgress::SetMerged { total: 0 }]
    ));
}

#[tokio::test]
async fn coordinated_revert_walks_reverse_merge_order() {
    let ctx = setup(&["repo-a", "repo-b", "repo-c"]).await;
    // merge_order 0,1,2 → forward a,b,c → reverse revert c,b,a.
    seed_pr(&ctx, &ctx.repos[0], 10, 0).await;
    seed_pr(&ctx, &ctx.repos[1], 20, 1).await;
    seed_pr(&ctx, &ctx.repos[2], 30, 2).await;

    let merger = Arc::new(
        FakeMerger::default()
            .with_merge("acme/repo-a", 10, "sha-a")
            .with_merge("acme/repo-b", 20, "sha-b")
            .with_merge("acme/repo-c", 30, "sha-c")
            .with_revert("acme/repo-a", 10, true)
            .with_revert("acme/repo-b", 20, true)
            .with_revert("acme/repo-c", 30, true),
    );
    let checks = ScriptedChecks::new()
        .pass("sha-a")
        .pass("sha-b")
        .pass("sha-c");
    let manager = wire(&ctx, Arc::clone(&merger), checks);

    // First merge the whole set (so all three are state=merged + revertible).
    let (_f, report) = run_merge(&manager, &ctx.workarea_id, MergeOpts::default()).await;
    assert_eq!(report.merged_steps, 3);

    // Now coordinated revert.
    let mut sub = manager.subscribe();
    let revert = manager
        .revert_workarea_pr_set(&ctx.workarea_id, RevertOpts::default())
        .await
        .expect("revert");

    // Walked in reverse merge_order: c(30), b(20), a(10).
    assert_eq!(
        merger.reverted_order(),
        vec![
            ("acme/repo-c".to_string(), 30),
            ("acme/repo-b".to_string(), 20),
            ("acme/repo-a".to_string(), 10),
        ]
    );
    assert_eq!(revert.steps.len(), 3);
    assert!(revert
        .steps
        .iter()
        .all(|s| s.outcome == RevertOutcome::Reverted));

    let mut reverted_events = 0;
    while let Ok(ev) = sub.try_recv() {
        if matches!(ev, WorkareaEvent::PrReverted { .. }) {
            reverted_events += 1;
        }
    }
    assert_eq!(reverted_events, 3, "one PrReverted broadcast per member");
}

#[tokio::test]
async fn revert_skips_unmerged_and_tolerates_partial_failure() {
    let ctx = setup(&["repo-a", "repo-b"]).await;
    seed_pr(&ctx, &ctx.repos[0], 10, 0).await;
    seed_pr(&ctx, &ctx.repos[1], 20, 1).await;

    // Only repo-a was merged; repo-b's revert (if attempted) would fail — but it
    // is still `open`, so it must be SKIPPED, not failed.
    let merger = Arc::new(
        FakeMerger::default()
            .with_merge("acme/repo-a", 10, "sha-a")
            .with_merge("acme/repo-b", 20, "sha-b")
            .with_revert("acme/repo-a", 10, true),
    );
    // repo-b's checks fail TERMINALLY (so the pause is immediate, not a 10m
    // timeout): map the merge SHA repo-b returns to a terminal-failure run.
    let checks = ScriptedChecks::new().pass("sha-a").fail("sha-b");
    let manager = wire(&ctx, Arc::clone(&merger), checks);

    // Merge — repo-b's checks fail so the loop pauses with only repo-a merged.
    let (_f, mr) = run_merge(&manager, &ctx.workarea_id, MergeOpts::default()).await;
    assert_eq!(mr.merged_steps, 1);

    let revert = manager
        .revert_workarea_pr_set(&ctx.workarea_id, RevertOpts::default())
        .await
        .expect("revert");

    // Reverse order: repo-b first (skipped — never merged), then repo-a (reverted).
    let outcomes: Vec<(&str, &RevertOutcome)> = revert
        .steps
        .iter()
        .map(|s| (s.repository_full_name.as_str(), &s.outcome))
        .collect();
    assert_eq!(
        outcomes,
        vec![
            ("acme/repo-b", &RevertOutcome::Skipped),
            ("acme/repo-a", &RevertOutcome::Reverted),
        ]
    );
    // Only repo-a's revert was actually attempted.
    assert_eq!(
        merger.reverted_order(),
        vec![("acme/repo-a".to_string(), 10)]
    );
}

#[tokio::test]
async fn allow_failing_checks_override_continues_when_policy_permits() {
    let ctx = setup(&["repo-a"]).await;
    seed_pr(&ctx, &ctx.repos[0], 10, 0).await;

    // managed.json explicitly permits the merge-anyway override.
    std::fs::create_dir_all(&ctx.config_dir).unwrap();
    std::fs::write(
        ctx.config_dir.join("managed.json"),
        r#"{"allowMergeWithFailingChecks": true}"#,
    )
    .unwrap();

    let merger = Arc::new(FakeMerger::default().with_merge("acme/repo-a", 10, "sha-a"));
    // Checks FAIL, but the override is on + permitted → continue + merge.
    let checks = ScriptedChecks::new().fail("sha-a");
    let manager = wire(&ctx, Arc::clone(&merger), checks);

    let opts = MergeOpts {
        allow_failing_checks: true,
        ..MergeOpts::default()
    };
    let (frames, report) = run_merge(&manager, &ctx.workarea_id, opts).await;

    assert_eq!(report.merged_steps, 1, "override merges despite red checks");
    assert_eq!(report.paused_at_step, None);
    assert_eq!(merger.merged_order(), vec![("acme/repo-a".to_string(), 10)]);
    assert!(matches!(
        frames.last(),
        Some(MergeProgress::SetMerged { total: 1 })
    ));
}

#[tokio::test]
async fn allow_failing_checks_locked_by_managed_json_is_permission_denied() {
    let ctx = setup(&["repo-a"]).await;
    seed_pr(&ctx, &ctx.repos[0], 10, 0).await;

    // managed.json explicitly FORBIDS the override.
    std::fs::create_dir_all(&ctx.config_dir).unwrap();
    std::fs::write(
        ctx.config_dir.join("managed.json"),
        r#"{"allowMergeWithFailingChecks": false}"#,
    )
    .unwrap();

    let merger = Arc::new(FakeMerger::default().with_merge("acme/repo-a", 10, "sha-a"));
    let manager = wire(
        &ctx,
        Arc::clone(&merger),
        ScriptedChecks::new().fail("sha-a"),
    );

    let (tx, _rx) = tokio::sync::mpsc::channel::<MergeProgress>(8);
    let opts = MergeOpts {
        allow_failing_checks: true,
        ..MergeOpts::default()
    };
    let err = manager
        .merge_workarea_pr_set(&ctx.workarea_id, opts, tx)
        .await
        .expect_err("locked policy rejects the override");
    assert_eq!(err.wire_code(), "policy.locked");
    // No PR was merged — the policy gate runs BEFORE any merge.
    assert!(merger.merged_order().is_empty());
}

#[tokio::test]
async fn get_merge_plan_returns_ordered_steps() {
    let ctx = setup(&["repo-a", "repo-b", "repo-c"]).await;
    // Arbitrary, non-contiguous, includes negative (Task 319 semantics).
    seed_pr(&ctx, &ctx.repos[0], 10, 5).await;
    seed_pr(&ctx, &ctx.repos[1], 20, -3).await;
    seed_pr(&ctx, &ctx.repos[2], 30, 0).await;

    let plan = ctx
        .manager
        .get_workarea_merge_plan(&ctx.workarea_id)
        .await
        .expect("plan");

    let order: Vec<(i64, i64)> = plan
        .steps
        .iter()
        .map(|s| (s.pr_number, s.merge_order))
        .collect();
    // Sorted by merge_order: -3 (repo-b/20), 0 (repo-c/30), 5 (repo-a/10).
    assert_eq!(order, vec![(20, -3), (30, 0), (10, 5)]);
    // 1-based step indices + correct total.
    assert_eq!(plan.steps[0].step, 1);
    assert_eq!(plan.steps[2].step, 3);
    assert!(plan.steps.iter().all(|s| s.total == 3));
}

#[tokio::test]
async fn get_merge_plan_rejects_unknown_workarea() {
    let ctx = setup(&["repo-a"]).await;
    let err = ctx
        .manager
        .get_workarea_merge_plan(&WorkareaId("nope".into()))
        .await
        .expect_err("unknown workarea");
    assert_eq!(err.wire_code(), "not_found");
}

#[tokio::test]
async fn merge_consumes_real_merge_report_from_fakegithub_double() {
    // The NAMED Tier-2 double: drive the real `GitHubProvider::merge_pr` against
    // the `concerto-vcs` testkit `FakeGitHub` wiremock and confirm the loop
    // consumes a real `MergeReport` carrying the mocked merge-commit SHA
    // (`design/13 §7.2` — the SHA `wait_for_check_runs` then gates on).
    use concerto_vcs::provider::VcsProvider;
    use concerto_vcs::testkit::FakeGitHub;

    let fake = FakeGitHub::start().await;
    // GitHub's PUT /repos/{owner}/{repo}/pulls/{n}/merge → {merged, sha, message}.
    fake.mount_put_json(
        "/repos/acme/repo-a/pulls/10/merge",
        200,
        serde_json::json!({
            "merged": true,
            "sha": "real-merge-sha-from-github",
            "message": "Pull Request successfully merged"
        }),
    )
    .await;
    let provider = fake.provider();

    let report = provider
        .merge_pr(
            concerto_vcs::provider::ProviderPrId::new("acme/repo-a".to_string(), 10),
            MergeMethod::Merge,
        )
        .await
        .expect("merge via FakeGitHub");
    assert!(report.merged);
    assert_eq!(
        report.merge_commit_sha.as_deref(),
        Some("real-merge-sha-from-github"),
        "the loop feeds this post-merge SHA into wait_for_check_runs"
    );
}
