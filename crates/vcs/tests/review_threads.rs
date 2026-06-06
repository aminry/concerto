//! Tier-2 tests for Task 316: review-thread sync (GraphQL) + check-run/deploy
//! aggregation + the `checks.<wa>.<repo>` opaque-frame emission + the §6.3
//! invalidation paths — all against 313's `testkit` `FakeGitHub` with recorded
//! GraphQL + REST fixtures (no real GitHub).
//!
//! What the double does NOT cover (→ Phase-3 Tier-3 checklist): real GitHub
//! GraphQL thread structure / pagination, a real resolve round-trip, and real
//! deployment statuses.

use std::sync::Arc;

use concerto_vcs::checks::{
    ChecksAggregator, KIND_CHECK_RUN_UPDATED, KIND_DEPLOYMENT_UPDATED, KIND_THREAD_UPDATED,
};
use concerto_vcs::provider::{ProviderPrId, ThreadId, VcsProvider};
use concerto_vcs::testkit::{fixture, FakeGitHub, SyntheticClock};

const WA: &str = "wa-1";
const REPO_ID: &str = "repo-1";
const REPO_FULL: &str = "acme/widget";

fn provider(gh: &FakeGitHub) -> Arc<dyn VcsProvider> {
    Arc::new(gh.provider())
}

/// Parse a broadcast frame's JSON for assertions.
fn parse(frame: &[u8]) -> serde_json::Value {
    serde_json::from_slice(frame).expect("frame is valid JSON")
}

// ---------------------------------------------------------------------------
// list_review_threads against a recorded GraphQL fixture.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_review_threads_parses_graphql_fixture() {
    let gh = FakeGitHub::start().await;
    gh.mount_graphql(fixture("review_threads.json")).await;
    let provider = provider(&gh);

    let threads = provider
        .list_review_threads(ProviderPrId::new(REPO_FULL, 42))
        .await
        .expect("list_review_threads");

    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0].id, ThreadId("RT_kwDOABCD1".to_string()));
    assert!(!threads[0].resolved);
    assert_eq!(threads[0].path.as_deref(), Some("src/main.rs"));
    assert_eq!(threads[0].comments.len(), 2);
    assert_eq!(threads[0].comments[0], "This nil check is missing.");
    // The second thread is PR-level (null path) + resolved.
    assert!(threads[1].resolved);
    assert_eq!(threads[1].path, None);
}

// ---------------------------------------------------------------------------
// resolve_thread mutation → cache updated + event emitted.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resolve_thread_updates_cache_and_emits() {
    let gh = FakeGitHub::start().await;
    // The list query + the resolve mutation distinguished by body substring.
    gh.mount_graphql_matching("reviewThreads", fixture("review_threads.json"))
        .await;
    gh.mount_graphql_matching("resolveReviewThread", fixture("resolve_thread.json"))
        .await;
    let provider = provider(&gh);
    let agg = ChecksAggregator::new();
    let mut rx = agg.subscribe();
    let pr = ProviderPrId::new(REPO_FULL, 42);

    // Populate the cache (drains the initial per-thread emits).
    agg.list_review_threads(&provider, WA, REPO_ID, pr.clone())
        .await
        .expect("list");
    while rx.try_recv().is_ok() {}

    // Resolve thread 1 (currently unresolved in the cache).
    agg.resolve_thread(
        &provider,
        WA,
        REPO_ID,
        &pr,
        ThreadId("RT_kwDOABCD1".to_string()),
    )
    .await
    .expect("resolve");

    // The cached thread is now resolved.
    let cached = agg.cached_threads(&pr).expect("cached");
    let t = cached
        .iter()
        .find(|t| t.id == ThreadId("RT_kwDOABCD1".to_string()))
        .expect("thread present");
    assert!(t.resolved, "cache flipped to resolved");

    // An event was emitted on the (wa, repo) scope with the resolved frame.
    let ev = rx.try_recv().expect("event emitted");
    assert_eq!(ev.workarea_id, WA);
    assert_eq!(ev.repository_id, REPO_ID);
    let frame = parse(&ev.frame);
    assert_eq!(frame["kind"], KIND_THREAD_UPDATED);
    assert_eq!(frame["entity"]["id"], "RT_kwDOABCD1");
    assert_eq!(frame["entity"]["resolved"], true);
}

// ---------------------------------------------------------------------------
// Check-run aggregation TTL behavior (synthetic clock).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn check_runs_cache_ttl_refetch() {
    let gh = FakeGitHub::start().await;
    gh.mount_get_json(
        "/repos/acme/widget/commits/abc123/check-runs",
        fixture("check_runs.json"),
    )
    .await;
    let provider = provider(&gh);
    let clock = Arc::new(SyntheticClock::new(1_000));
    let clock_for_agg = clock.clone();
    let agg = ChecksAggregator::with_clock(Arc::new(move || clock_for_agg.now()));

    // First read: a fetch + an emit (cache empty).
    let runs = agg
        .check_runs(&provider, WA, REPO_ID, REPO_FULL, "abc123", false)
        .await
        .expect("first");
    assert_eq!(runs.len(), 2);
    assert_eq!(gh.server().received_requests().await.unwrap().len(), 1);

    // Within the 30s TTL: served from cache, NO second HTTP call.
    clock.advance(10);
    agg.check_runs(&provider, WA, REPO_ID, REPO_FULL, "abc123", false)
        .await
        .expect("cached");
    assert_eq!(
        gh.server().received_requests().await.unwrap().len(),
        1,
        "within TTL → no refetch"
    );

    // Past the 30s TTL: stale → refetch (a second HTTP call).
    clock.advance(25);
    agg.check_runs(&provider, WA, REPO_ID, REPO_FULL, "abc123", false)
        .await
        .expect("refetch");
    assert_eq!(
        gh.server().received_requests().await.unwrap().len(),
        2,
        "past TTL → refetch"
    );
}

#[tokio::test]
async fn check_runs_emits_opaque_frame() {
    let gh = FakeGitHub::start().await;
    gh.mount_get_json(
        "/repos/acme/widget/commits/abc123/check-runs",
        fixture("check_runs.json"),
    )
    .await;
    let provider = provider(&gh);
    let agg = ChecksAggregator::new();
    let mut rx = agg.subscribe();

    agg.check_runs(&provider, WA, REPO_ID, REPO_FULL, "abc123", false)
        .await
        .expect("fetch");

    let ev = rx.try_recv().expect("check_run event");
    let frame = parse(&ev.frame);
    assert_eq!(frame["kind"], KIND_CHECK_RUN_UPDATED);
    assert_eq!(frame["entity"]["sha"], "abc123");
    assert_eq!(frame["entity"]["runs"].as_array().unwrap().len(), 2);
    assert_eq!(frame["entity"]["runs"][0]["name"], "build");
}

// ---------------------------------------------------------------------------
// Deployment aggregation (deployment + per-deployment status).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deployments_aggregate_status_and_emit() {
    let gh = FakeGitHub::start().await;
    gh.mount_get_json(
        "/repos/acme/widget/deployments",
        fixture("deployments.json"),
    )
    .await;
    // Each deployment's latest status (the second call per deployment).
    gh.mount_get_json(
        "/repos/acme/widget/deployments/555/statuses",
        fixture("deployment_statuses.json"),
    )
    .await;
    gh.mount_get_json(
        "/repos/acme/widget/deployments/556/statuses",
        fixture("deployment_statuses.json"),
    )
    .await;
    let provider = provider(&gh);
    let agg = ChecksAggregator::new();
    let mut rx = agg.subscribe();

    let deps = agg
        .list_deployments(&provider, WA, REPO_ID, REPO_FULL, "main")
        .await
        .expect("deployments");
    assert_eq!(deps.len(), 2);
    assert_eq!(deps[0].environment, "production");
    assert_eq!(deps[0].state, "success", "status aggregated");

    let ev = rx.try_recv().expect("deployment event");
    let frame = parse(&ev.frame);
    assert_eq!(frame["kind"], KIND_DEPLOYMENT_UPDATED);
    assert_eq!(frame["entity"]["ref"], "main");
    assert_eq!(frame["entity"]["deployments"][0]["state"], "success");
}

// ---------------------------------------------------------------------------
// Webhook-targeted invalidation (a fake hook fires → just the affected thread
// refetched + emitted). Until Task 315 lands, this is the seam 315 will call.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn webhook_invalidation_refetches_threads_and_emits() {
    let gh = FakeGitHub::start().await;
    gh.mount_graphql(fixture("review_threads.json")).await;
    let provider = provider(&gh);
    let agg = ChecksAggregator::new();
    let mut rx = agg.subscribe();
    let pr = ProviderPrId::new(REPO_FULL, 42);

    // Prime the cache.
    agg.list_review_threads(&provider, WA, REPO_ID, pr.clone())
        .await
        .expect("prime");
    while rx.try_recv().is_ok() {}
    let before = gh.graphql_request_count().await;

    // A fake webhook-invalidation hook fires for this PR.
    agg.invalidate_threads(&provider, WA, REPO_ID, pr.clone())
        .await
        .expect("invalidate");

    // It dropped the cache + re-fetched (one more GraphQL call) + re-emitted.
    assert_eq!(
        gh.graphql_request_count().await,
        before + 1,
        "invalidation forces a refetch"
    );
    let ev = rx.try_recv().expect("re-emit after invalidation");
    assert_eq!(ev.workarea_id, WA);
    assert_eq!(parse(&ev.frame)["kind"], KIND_THREAD_UPDATED);
}

// ---------------------------------------------------------------------------
// A second list with identical state does NOT re-emit (poll + webhook that see
// the same state don't double-update — `design/13 §6.2` "no double-update").
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unchanged_threads_do_not_reemit() {
    let gh = FakeGitHub::start().await;
    gh.mount_graphql(fixture("review_threads.json")).await;
    let provider = provider(&gh);
    let agg = ChecksAggregator::new();
    let mut rx = agg.subscribe();
    let pr = ProviderPrId::new(REPO_FULL, 42);

    agg.list_review_threads(&provider, WA, REPO_ID, pr.clone())
        .await
        .expect("first");
    // First fetch emits one event per thread (2).
    let mut first_count = 0;
    while rx.try_recv().is_ok() {
        first_count += 1;
    }
    assert_eq!(first_count, 2);

    // Second fetch with the SAME fixture → no change → no emit.
    agg.list_review_threads(&provider, WA, REPO_ID, pr)
        .await
        .expect("second");
    assert!(rx.try_recv().is_err(), "unchanged state does not re-emit");
}
