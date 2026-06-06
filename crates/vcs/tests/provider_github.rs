//! Tier-2 tests for `crates/vcs` (Task 313).
//!
//! The test **double** is the shared `wiremock`-backed `testkit` harness
//! ([`concerto_vcs::testkit::FakeGitHub`]) driving recorded REST fixtures under
//! `crates/vcs/tests/fixtures/`. It proves the `GitHubProvider` request-shaping
//! and response-projection logic, the `choose_backend` dispatch table, the
//! `fetch_issue` URL-host router, and the trait swap-fixture contract
//! (the `design/18 §3.7` registry requirement of at least one OSS impl plus a
//! swap test fixture).
//!
//! What this double does NOT cover (the Tier-3 Phase-3 checklist line): the real
//! GitHub API round-trip — real auth, real rate limits, live webhooks, a real
//! coordinated PR-set merge against a real repo. Those are signed off at the
//! phase gate, not here.

use std::sync::Arc;

use concerto_vcs::testkit::{fixture, rate_limit_headers, FakeGitHub, SyntheticClock};
use concerto_vcs::{
    choose_backend, is_no_vcs_credentials, is_unimplemented, route_issue_host, Backend,
    CreatePrRequest, IssueHost, MergeMethod, ProviderPrId, RepoCapabilities, VcsOp, VcsProvider,
};
use url::Url;

// ---------------------------------------------------------------------------
// GitHubProvider REST methods against the recorded wiremock fixtures.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_pr_posts_and_projects_response() {
    let gh = FakeGitHub::start().await;
    gh.mount_post_json("/repos/acme/widget/pulls", 201, fixture("create_pr.json"))
        .await;
    let provider = gh.provider();

    let pr = provider
        .create_pr(CreatePrRequest {
            repo_full_name: "acme/widget".to_string(),
            head: "feature/add-widget".to_string(),
            base: "main".to_string(),
            title: "Add the widget".to_string(),
            body: "This PR adds the widget.".to_string(),
            draft: false,
        })
        .await
        .expect("create_pr");

    assert_eq!(pr.id.repo_full_name, "acme/widget");
    assert_eq!(pr.id.number, 101);
    assert_eq!(pr.id.node_id.as_deref(), Some("PR_kwDOABCD123"));
    assert_eq!(pr.title, "Add the widget");
    assert_eq!(pr.state, "open");
    assert_eq!(pr.base_ref, "main");
    assert_eq!(pr.head_ref, "feature/add-widget");
    assert_eq!(pr.url, "https://github.com/acme/widget/pull/101");
}

#[tokio::test]
async fn get_pr_fetches_and_projects() {
    let gh = FakeGitHub::start().await;
    gh.mount_get_json("/repos/acme/widget/pulls/101", fixture("get_pr.json"))
        .await;
    let provider = gh.provider();

    let pr = provider
        .get_pr(ProviderPrId::new("acme/widget", 101))
        .await
        .expect("get_pr");

    assert_eq!(pr.id.number, 101);
    assert_eq!(pr.head_sha, "cccccccccccccccccccccccccccccccccccccccc");
    assert_eq!(pr.state, "open");
}

#[tokio::test]
async fn list_check_runs_projects_each_run() {
    let gh = FakeGitHub::start().await;
    gh.mount_get_json(
        "/repos/acme/widget/commits/deadbeef/check-runs",
        fixture("check_runs.json"),
    )
    .await;
    let provider = gh.provider();

    let runs = provider
        .list_check_runs("acme/widget", "deadbeef")
        .await
        .expect("list_check_runs");

    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].name, "build");
    assert_eq!(runs[0].status, "completed");
    assert_eq!(runs[0].conclusion, "success");
    // The in-progress run has a null conclusion → projected to an empty string.
    assert_eq!(runs[1].name, "test");
    assert_eq!(runs[1].status, "in_progress");
    assert_eq!(runs[1].conclusion, "");
}

#[tokio::test]
async fn merge_pr_puts_and_reports() {
    let gh = FakeGitHub::start().await;
    gh.mount_put_json(
        "/repos/acme/widget/pulls/101/merge",
        200,
        fixture("merge_pr.json"),
    )
    .await;
    let provider = gh.provider();

    let report = provider
        .merge_pr(ProviderPrId::new("acme/widget", 101), MergeMethod::Squash)
        .await
        .expect("merge_pr");

    assert!(report.merged);
    assert_eq!(
        report.merge_commit_sha.as_deref(),
        Some("dddddddddddddddddddddddddddddddddddddddd")
    );
    assert_eq!(report.message, "Pull Request successfully merged");
}

#[tokio::test]
async fn list_deployments_projects_with_empty_state() {
    let gh = FakeGitHub::start().await;
    // Deployments are filtered by `?ref=main`; the harness matches the query.
    gh.mount_get_json_q(
        "/repos/acme/widget/deployments",
        "ref",
        "main",
        fixture("deployments.json"),
    )
    .await;
    let provider = gh.provider();

    let deployments = provider
        .list_deployments("acme/widget", "main")
        .await
        .expect("list_deployments");

    assert_eq!(deployments.len(), 2);
    assert_eq!(deployments[0].id, "555");
    assert_eq!(deployments[0].environment, "production");
    assert_eq!(deployments[0].ref_, "main");
    // Per-deployment status aggregation is Task 316's → state is empty here.
    assert_eq!(deployments[0].state, "");
}

#[tokio::test]
async fn fetch_issue_gets_and_projects() {
    let gh = FakeGitHub::start().await;
    gh.mount_get_json("/repos/acme/widget/issues/7", fixture("issue.json"))
        .await;
    let provider = gh.provider();

    // The URL is parsed for repo+number; the actual HTTP call hits the mock base.
    let url = Url::parse("https://github.com/acme/widget/issues/7").unwrap();
    let issue = provider
        .fetch_issue(&url)
        .await
        .expect("fetch_issue")
        .expect("issue present");

    assert_eq!(issue.number, 7);
    assert_eq!(issue.title, "Widget falls over on Tuesdays");
    assert_eq!(issue.state, "open");
    assert_eq!(issue.labels, vec!["bug", "priority:high"]);
}

// ---------------------------------------------------------------------------
// Signature-frozen stubs return the typed `Unimplemented` (Task 316/320).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn graphql_and_revert_stubs_return_unimplemented() {
    let gh = FakeGitHub::start().await;
    let provider = gh.provider();
    let id = ProviderPrId::new("acme/widget", 101);

    let err = provider
        .list_review_threads(id.clone())
        .await
        .expect_err("stub");
    assert!(is_unimplemented(&err), "list_review_threads is a 316 stub");

    let err = provider.revert_pr(id).await.expect_err("stub");
    assert!(is_unimplemented(&err), "revert_pr is a 320 stub");

    let err = provider
        .resolve_thread(concerto_vcs::ThreadId("T_1".to_string()))
        .await
        .expect_err("stub");
    assert!(is_unimplemented(&err), "resolve_thread is a 316 stub");
}

// ---------------------------------------------------------------------------
// choose_backend dispatch table (`design/13 §6.1`).
// ---------------------------------------------------------------------------

#[test]
fn choose_backend_dispatch_table() {
    // fetch_issue always routes to the URL-host router, regardless of creds.
    assert_eq!(
        choose_backend(
            RepoCapabilities {
                has_github_app: false,
                has_octocrab_token: true,
                gh_available: true,
            },
            VcsOp::FetchIssue,
        )
        .unwrap(),
        Backend::IssueRouter
    );

    // A configured token → octocrab.
    assert_eq!(
        choose_backend(
            RepoCapabilities {
                has_github_app: false,
                has_octocrab_token: true,
                gh_available: false,
            },
            VcsOp::PrOp,
        )
        .unwrap(),
        Backend::Octocrab
    );

    // No token but `gh` available → the CLI fallback.
    assert_eq!(
        choose_backend(
            RepoCapabilities {
                has_github_app: false,
                has_octocrab_token: false,
                gh_available: true,
            },
            VcsOp::PrOp,
        )
        .unwrap(),
        Backend::GhCli
    );

    // Neither → the typed NoVcsCredentials decision error.
    let err = choose_backend(
        RepoCapabilities {
            has_github_app: false,
            has_octocrab_token: false,
            gh_available: false,
        },
        VcsOp::PrOp,
    )
    .expect_err("no creds");
    assert!(is_no_vcs_credentials(&err));
}

// ---------------------------------------------------------------------------
// fetch_issue URL-host router (`design/13 §6.1`).
// ---------------------------------------------------------------------------

#[test]
fn route_issue_host_classifies_hosts() {
    assert_eq!(
        route_issue_host(&Url::parse("https://github.com/acme/widget/issues/7").unwrap()).unwrap(),
        IssueHost::GitHub
    );
    assert_eq!(
        route_issue_host(&Url::parse("https://linear.app/acme/issue/ENG-1").unwrap()).unwrap(),
        IssueHost::Linear
    );
    assert_eq!(
        route_issue_host(&Url::parse("https://acme.atlassian.net/browse/ENG-1").unwrap()).unwrap(),
        IssueHost::Jira
    );
    // An unrecognized host is a Validation error, not a panic.
    assert!(
        route_issue_host(&Url::parse("https://example.com/x").unwrap()).is_err(),
        "unknown host rejected"
    );
}

// ---------------------------------------------------------------------------
// The `design/18 §3.7` trait swap-fixture test: exercise the FROZEN
// `VcsProvider` surface against two impls behind one `dyn` reference.
// ---------------------------------------------------------------------------

/// A second, in-memory `VcsProvider` impl (the "swap" target) proving the trait
/// is genuinely provider-agnostic: a caller holding `Arc<dyn VcsProvider>` works
/// against this fake exactly as against `GitHubProvider`. This satisfies the
/// registry contract's "≥1 OSS impl + a test fixture for swap".
struct InMemoryProvider {
    pr: concerto_vcs::PullRequest,
}

#[async_trait::async_trait]
impl VcsProvider for InMemoryProvider {
    async fn create_pr(
        &self,
        _req: CreatePrRequest,
    ) -> concerto_error::Result<concerto_vcs::PullRequest> {
        Ok(self.pr.clone())
    }
    async fn get_pr(&self, _id: ProviderPrId) -> concerto_error::Result<concerto_vcs::PullRequest> {
        Ok(self.pr.clone())
    }
    async fn list_check_runs(
        &self,
        _repo: &str,
        _sha: &str,
    ) -> concerto_error::Result<Vec<concerto_vcs::CheckRun>> {
        Ok(vec![])
    }
    async fn merge_pr(
        &self,
        _id: ProviderPrId,
        _method: MergeMethod,
    ) -> concerto_error::Result<concerto_vcs::MergeReport> {
        Ok(concerto_vcs::MergeReport {
            merged: true,
            merge_commit_sha: None,
            message: "in-memory".to_string(),
        })
    }
    async fn revert_pr(
        &self,
        _id: ProviderPrId,
    ) -> concerto_error::Result<concerto_vcs::RevertReport> {
        Ok(concerto_vcs::RevertReport {
            reverted: true,
            revert_pr_url: None,
            message: "in-memory".to_string(),
        })
    }
    async fn list_review_threads(
        &self,
        _id: ProviderPrId,
    ) -> concerto_error::Result<Vec<concerto_vcs::ReviewThread>> {
        Ok(vec![])
    }
    async fn resolve_thread(&self, _id: concerto_vcs::ThreadId) -> concerto_error::Result<()> {
        Ok(())
    }
    async fn list_deployments(
        &self,
        _repo: &str,
        _ref_: &str,
    ) -> concerto_error::Result<Vec<concerto_vcs::Deployment>> {
        Ok(vec![])
    }
    async fn fetch_issue(&self, _url: &Url) -> concerto_error::Result<Option<concerto_vcs::Issue>> {
        Ok(None)
    }
}

#[tokio::test]
async fn trait_swap_fixture_two_impls_behind_dyn() {
    // Impl A: the real octocrab GitHubProvider against the wiremock double.
    let gh = FakeGitHub::start().await;
    gh.mount_get_json("/repos/acme/widget/pulls/101", fixture("get_pr.json"))
        .await;
    let github: Arc<dyn VcsProvider> = Arc::new(gh.provider());

    // Impl B: the in-memory swap target.
    let in_memory: Arc<dyn VcsProvider> = Arc::new(InMemoryProvider {
        pr: concerto_vcs::PullRequest {
            id: ProviderPrId::new("acme/widget", 101),
            title: "swapped".to_string(),
            body: String::new(),
            state: "open".to_string(),
            url: "https://example/pr/101".to_string(),
            base_ref: "main".to_string(),
            head_ref: "feat".to_string(),
            head_sha: "0".repeat(40),
        },
    });

    // The SAME caller code runs against both — the trait is the only contract.
    for provider in [&github, &in_memory] {
        let pr = provider
            .get_pr(ProviderPrId::new("acme/widget", 101))
            .await
            .expect("get_pr via dyn VcsProvider");
        assert_eq!(pr.id.number, 101);
    }

    // And the merge surface is uniform across impls.
    let report = in_memory
        .merge_pr(ProviderPrId::new("acme/widget", 101), MergeMethod::Merge)
        .await
        .expect("merge via dyn");
    assert!(report.merged);
}

// ---------------------------------------------------------------------------
// The synthetic rate-limit + clock hooks Task 314 consumes (frozen surface).
// ---------------------------------------------------------------------------

#[test]
fn rate_limit_headers_and_synthetic_clock() {
    let headers = rate_limit_headers(5000, 4999, 1_700_000_000);
    assert!(headers
        .iter()
        .any(|(k, v)| k == "x-ratelimit-remaining" && v == "4999"));

    let clock = SyntheticClock::new(1_000);
    assert_eq!(clock.now(), 1_000);
    assert_eq!(clock.advance(60), 1_060);
    assert_eq!(clock.now(), 1_060);
}

#[tokio::test]
async fn fake_github_attaches_synthetic_rate_limit_headers() {
    // Proves the Task-314-facing hook works end-to-end: the provider can read a
    // rate-limited response body (header inspection itself is 314's to wire).
    let gh = FakeGitHub::start().await;
    gh.mount_get_json_rate_limited(
        "/repos/acme/widget/pulls/101",
        fixture("get_pr.json"),
        5000,
        4998,
        1_700_000_000,
    )
    .await;
    let provider = gh.provider();
    let pr = provider
        .get_pr(ProviderPrId::new("acme/widget", 101))
        .await
        .expect("get_pr with rate-limit headers");
    assert_eq!(pr.id.number, 101);
}
