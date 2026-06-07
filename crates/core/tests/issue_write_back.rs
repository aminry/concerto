//! Task 320.5 — Linear/Jira issue write-back on coordinated-merge completion
//! (Tier 2).
//!
//! Exercises the post-merge write-back hook in
//! [`WorkareaManager::merge_workarea_pr_set`]'s success path against:
//!
//! - the NAMED Tier-2 double — the `concerto-vcs` `testkit` **`FakeLinear`** /
//!   **`FakeJira`** wiremock servers, recording the Linear `issueUpdate` GraphQL
//!   mutation / the Jira `POST /transitions` REST call so the test asserts the
//!   right transition was sent per provider; and
//! - a scripted `PrSetVcs` merger + a real `SchedulerHandle` with scripted
//!   checks (the same harness Task 320's `coordinated_merge.rs` uses) so a full
//!   coordinated merge runs to completion before the write-back step.
//!
//! Proves: the per-project opt-in gate (`projects.settings_json.issue_write_back`,
//! default off), the correct transition per provider, the best-effort
//! non-blocking contract (a write-back error leaves the `MergeReport` "merged"
//! and emits a `failed` event), and the skip/no-op paths (absent ref, GitHub
//! ref).
//!
//! What this double does NOT cover (→ the Phase-3 Tier-3 checklist line
//! "transition a real Linear and Jira issue on coordinated-merge completion"):
//! real Linear/Jira workflow-state resolution against a live tracker (the
//! team-specific "Done" state ids), real OAuth-token refresh mid-write, or real
//! API error shapes.

#![cfg(unix)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use concerto_core::repo_manager::RepoManager;
use concerto_core::scheduler::wait_checks::{CheckRunSnapshot, CheckRunsSource};
use concerto_core::scheduler::SchedulerHandle;
use concerto_core::workspace_manager::{
    MergeOpts, MergeProgress, PrSetVcs, WorkareaEvent, WorkareaManager,
};
use concerto_error::{Error, Result};
use concerto_keychain::SecretValue;
use concerto_persist::{
    pull_requests, NewProject, NewPullRequest, NewRepository, NewWorkarea, NewWorkspace,
    Persistence, PersistenceConfig, ProjectId, PullRequestId, RepositoryId, WorkareaId,
    WorkspaceId,
};
use concerto_vcs::provider::{
    MergeMethod, MergeReport as ProviderMergeReport, RevertReport as ProviderRevertReport,
};
use concerto_vcs::testkit::{fixture, FakeJira, FakeLinear};
use concerto_vcs::{IssueProvider, IssueWriteBack, LinearJiraWriteBack, WriteBackTokens};

// ---------------------------------------------------------------------------
// Scripted doubles (a minimal merge double — checks always pass).
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FakeMerger {
    merged: Mutex<Vec<(String, i64)>>,
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
        self.merged
            .lock()
            .unwrap()
            .push((repository_full_name.to_string(), pr_number));
        Ok(ProviderMergeReport {
            merged: true,
            merge_commit_sha: Some(format!("sha-{repository_full_name}-{pr_number}")),
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
        Ok(ProviderRevertReport {
            reverted: true,
            revert_pr_url: Some(format!(
                "https://github.com/{repository_full_name}/pull/{pr_number}"
            )),
            message: "reverted".into(),
        })
    }
}

/// Checks source: every SHA passes terminally.
struct AllPass;

#[async_trait]
impl CheckRunsSource for AllPass {
    async fn check_runs(&self, _repo: &RepositoryId, _sha: &str) -> Result<Vec<CheckRunSnapshot>> {
        Ok(vec![CheckRunSnapshot {
            name: "ci".into(),
            status: "completed".into(),
            conclusion: "success".into(),
        }])
    }
}

/// A static token resolver: returns one token for any provider (the fake never
/// verifies it). `None` ⇒ no credential connected.
struct StaticTokens(Option<&'static str>);

#[async_trait]
impl WriteBackTokens for StaticTokens {
    async fn token(&self, _provider: IssueProvider) -> Result<Option<SecretValue>> {
        Ok(self.0.map(|t| SecretValue::new(t.to_string())))
    }
}

/// A write-back that always errors (simulate auth lapse / tracker outage) so the
/// test proves a failure never fails the merge.
struct FailingWriteBack;

#[async_trait]
impl IssueWriteBack for FailingWriteBack {
    async fn transition_on_merge(
        &self,
        _issue_ref: &concerto_vcs::IssueRef,
        _transition: concerto_vcs::IssueTransition,
    ) -> Result<()> {
        Err(Error::Vcs("simulated tracker outage".into()))
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
    project_id: ProjectId,
    repo: RepositoryId,
}

async fn setup() -> Ctx {
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
    let repo = RepositoryId("repo-a".to_string());

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
        concerto_persist::repositories::insert(
            &mut w,
            NewRepository {
                id: repo.clone(),
                project_id: project_id.0.clone(),
                name: repo.0.clone(),
                url: format!("https://github.com/acme/{}", repo.0),
                local_path: format!("/tmp/{}", repo.0),
                clone_strategy: "full".into(),
                default_branch: "main".into(),
            },
        )
        .await
        .unwrap();
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
    let manager = WorkareaManager::new(
        Arc::clone(&persist),
        repo_manager,
        Arc::new(dir.path().join("data")),
        Arc::new(dir.path().join("config")),
    );

    Ctx {
        _dir: dir,
        persist,
        manager,
        workarea_id,
        project_id,
        repo,
    }
}

/// Stamp `projects.settings_json.issue_write_back`.
async fn set_opt_in(ctx: &Ctx, on: bool) {
    let payload = serde_json::json!({ "issue_write_back": on }).to_string();
    let mut w = ctx.persist.writer().await;
    concerto_persist::projects::set_settings_json(&mut w, &ctx.project_id, &payload)
        .await
        .unwrap();
}

/// Stamp `workareas.settings_json.source_issue_ref`.
async fn set_source_issue_ref(ctx: &Ctx, url: &str) {
    let payload = serde_json::json!({ "source_issue_ref": url }).to_string();
    let mut w = ctx.persist.writer().await;
    concerto_persist::workareas::set_settings_json(&mut w, &ctx.workarea_id, &payload)
        .await
        .unwrap();
}

async fn seed_pr(ctx: &Ctx) {
    let mut w = ctx.persist.writer().await;
    pull_requests::upsert(
        &mut w,
        NewPullRequest {
            id: PullRequestId(uuid::Uuid::now_v7().to_string()),
            workarea_id: ctx.workarea_id.clone(),
            repository_id: ctx.repo.clone(),
            provider: "github".into(),
            pr_number: 10,
            base_ref: "main".into(),
            head_ref: "feature".into(),
            state: "open".into(),
            title: "T".into(),
            body: String::new(),
            url: String::new(),
            head_sha: "head-a".into(),
            merge_order: 0,
            external_id: String::new(),
            repository_full_name: format!("acme/{}", ctx.repo.0),
            created_at: 1,
            updated_at: 1,
        },
    )
    .await
    .unwrap();
}

fn wire(ctx: &Ctx, write_back: Arc<dyn IssueWriteBack>) -> WorkareaManager {
    let scheduler = SchedulerHandle::new(Arc::clone(&ctx.persist), None);
    scheduler.set_check_runs_source(Arc::new(AllPass));
    ctx.manager
        .clone()
        .with_pr_set_vcs(Arc::new(FakeMerger::default()))
        .with_scheduler(scheduler)
        .with_issue_write_back(write_back)
}

/// Run the merge to completion, returning the report + the broadcast events seen.
async fn run_merge(
    manager: &WorkareaManager,
    workarea_id: &WorkareaId,
) -> (
    concerto_core::workspace_manager::MergeReport,
    Vec<WorkareaEvent>,
) {
    let mut sub = manager.subscribe();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<MergeProgress>(64);
    let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let report = manager
        .merge_workarea_pr_set(workarea_id, MergeOpts::default(), tx)
        .await
        .expect("merge");
    drain.await.unwrap();
    let mut events = Vec::new();
    while let Ok(ev) = sub.try_recv() {
        events.push(ev);
    }
    (report, events)
}

fn write_back_outcome(events: &[WorkareaEvent]) -> Option<(String, String, String)> {
    events.iter().find_map(|e| match e {
        WorkareaEvent::PrSetIssueWriteBack {
            provider,
            external_id,
            outcome,
            ..
        } => Some((provider.clone(), external_id.clone(), outcome.clone())),
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn opt_in_linear_transition_records_issue_update() {
    let ctx = setup().await;
    seed_pr(&ctx).await;
    set_opt_in(&ctx, true).await;
    set_source_issue_ref(&ctx, "https://linear.app/acme/issue/ENG-123/fix").await;

    let linear = FakeLinear::start().await;
    linear
        .mount_graphql_matching("team", fixture("linear_issue_states.json"))
        .await;
    linear
        .mount_graphql_matching("issueUpdate", fixture("linear_issue_update.json"))
        .await;

    let wb: Arc<dyn IssueWriteBack> = Arc::new(
        LinearJiraWriteBack::new(Arc::new(StaticTokens(Some("lin"))))
            .unwrap()
            .with_linear_base(&linear.base_uri()),
    );
    let manager = wire(&ctx, wb);

    let (report, events) = run_merge(&manager, &ctx.workarea_id).await;
    assert_eq!(report.merged_steps, 1);
    assert_eq!(report.paused_at_step, None);

    assert_eq!(
        linear.graphql_request_count("issueUpdate").await,
        1,
        "the issueUpdate mutation was sent"
    );
    let outcome = write_back_outcome(&events).expect("write-back event");
    assert_eq!(
        outcome,
        ("linear".into(), "ENG-123".into(), "written".into())
    );
}

#[tokio::test]
async fn opt_in_jira_transition_posts_done() {
    let ctx = setup().await;
    seed_pr(&ctx).await;
    set_opt_in(&ctx, true).await;
    set_source_issue_ref(&ctx, "https://acme.atlassian.net/browse/PROJ-45").await;

    let jira = FakeJira::start().await;
    jira.mount_get_json(
        "/rest/api/3/issue/PROJ-45/transitions",
        fixture("jira_transitions.json"),
    )
    .await;
    jira.mount_post_status("/rest/api/3/issue/PROJ-45/transitions", 204)
        .await;

    let wb: Arc<dyn IssueWriteBack> = Arc::new(
        LinearJiraWriteBack::new(Arc::new(StaticTokens(Some("jira"))))
            .unwrap()
            .with_jira_base(&jira.base_uri()),
    );
    let manager = wire(&ctx, wb);

    let (report, events) = run_merge(&manager, &ctx.workarea_id).await;
    assert_eq!(report.merged_steps, 1);

    assert_eq!(
        jira.post_request_count("/rest/api/3/issue/PROJ-45/transitions")
            .await,
        1,
        "the transition POST landed"
    );
    let outcome = write_back_outcome(&events).expect("write-back event");
    assert_eq!(outcome, ("jira".into(), "PROJ-45".into(), "written".into()));
}

#[tokio::test]
async fn opt_in_off_makes_no_tracker_mutation() {
    let ctx = setup().await;
    seed_pr(&ctx).await;
    // Opt-in OFF (default — never set the key); a source ref IS present.
    set_source_issue_ref(&ctx, "https://linear.app/acme/issue/ENG-123/fix").await;

    let linear = FakeLinear::start().await;
    linear
        .mount_graphql_matching("team", fixture("linear_issue_states.json"))
        .await;
    linear
        .mount_graphql_matching("issueUpdate", fixture("linear_issue_update.json"))
        .await;

    let wb: Arc<dyn IssueWriteBack> = Arc::new(
        LinearJiraWriteBack::new(Arc::new(StaticTokens(Some("lin"))))
            .unwrap()
            .with_linear_base(&linear.base_uri()),
    );
    let manager = wire(&ctx, wb);

    let (report, events) = run_merge(&manager, &ctx.workarea_id).await;
    assert_eq!(report.merged_steps, 1, "merge still completes");
    // No HTTP call to the tracker at all, and NO write-back event.
    assert_eq!(linear.request_count().await, 0, "no tracker mutation");
    assert!(
        write_back_outcome(&events).is_none(),
        "opt-in off emits no write-back event"
    );
}

#[tokio::test]
async fn write_back_failure_does_not_fail_the_merge() {
    let ctx = setup().await;
    seed_pr(&ctx).await;
    set_opt_in(&ctx, true).await;
    set_source_issue_ref(&ctx, "https://linear.app/acme/issue/ENG-123/fix").await;

    let manager = wire(&ctx, Arc::new(FailingWriteBack));

    let (report, events) = run_merge(&manager, &ctx.workarea_id).await;
    // The merge is STILL reported merged.
    assert_eq!(report.merged_steps, 1);
    assert_eq!(report.paused_at_step, None);
    // A `failed` write-back event was emitted (the contract).
    let outcome = write_back_outcome(&events).expect("write-back event");
    assert_eq!(outcome.0, "linear");
    assert_eq!(outcome.2, "failed");
}

#[tokio::test]
async fn absent_issue_ref_is_skipped() {
    let ctx = setup().await;
    seed_pr(&ctx).await;
    set_opt_in(&ctx, true).await;
    // No source_issue_ref set on the workarea.

    let manager = wire(&ctx, Arc::new(FailingWriteBack));

    let (report, events) = run_merge(&manager, &ctx.workarea_id).await;
    assert_eq!(report.merged_steps, 1);
    let outcome = write_back_outcome(&events).expect("write-back event");
    assert_eq!(outcome.2, "skipped");
}

#[tokio::test]
async fn github_issue_ref_is_skipped() {
    let ctx = setup().await;
    seed_pr(&ctx).await;
    set_opt_in(&ctx, true).await;
    set_source_issue_ref(&ctx, "https://github.com/acme/repo-a/issues/7").await;

    // The write-back impl would error if ever called, proving GitHub never
    // reaches the tracker transition.
    let manager = wire(&ctx, Arc::new(FailingWriteBack));

    let (report, events) = run_merge(&manager, &ctx.workarea_id).await;
    assert_eq!(report.merged_steps, 1);
    let outcome = write_back_outcome(&events).expect("write-back event");
    assert_eq!(
        outcome.2, "skipped",
        "GitHub issue refs no-op (PR keywords)"
    );
}
