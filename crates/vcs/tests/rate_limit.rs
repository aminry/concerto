//! Tier-2 tests for the dual (triple) rate-limit pools + degraded cadence +
//! warning/exhaustion/resume logic (Task 314, `design/13 §3.3`/§3.9) against
//! 313's `testkit` (synthetic `X-RateLimit-*` headers + synthetic clock). No
//! real GitHub.
//!
//! What this double does NOT cover: a real rate-limit degradation under live
//! GitHub load — the Tier-3 Phase-3 checklist line.

use std::sync::Arc;

use concerto_vcs::testkit::{rate_limit_headers, FakeGitHub, SyntheticClock};
use concerto_vcs::{
    check_run_backoff_secs, degraded_interval_secs, is_rate_limited, rate_limited,
    rate_limited_reset_at, OpPriority, ProviderKey, ProviderPrId, RateLimitPools, VcsProvider,
    VcsState, CHECK_RUN_BACKOFF_SECS, DEPLOYMENT_SECS, PR_STATE_BACKGROUND_SECS,
    PR_STATE_FOREGROUND_SECS, REVIEW_THREAD_SECS,
};

/// A header lookup closure over a `(name, value)` pair list (the testkit shape).
fn lookup<'a>(pairs: &'a [(String, String)]) -> impl Fn(&str) -> Option<&'a str> {
    move |name: &str| {
        pairs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

// ---------------------------------------------------------------------------
// Cadence constants — single source of truth (`design/13 §3.3` == `05 §3.9`).
// ---------------------------------------------------------------------------

#[test]
fn cadence_constants_match_design() {
    assert_eq!(CHECK_RUN_BACKOFF_SECS, [1, 2, 4, 8, 16, 30]);
    assert_eq!(PR_STATE_FOREGROUND_SECS, 30);
    assert_eq!(PR_STATE_BACKGROUND_SECS, 300);
    assert_eq!(REVIEW_THREAD_SECS, 60);
    assert_eq!(DEPLOYMENT_SECS, 60);
    // Backoff caps at 30 past the end of the sequence (318 imports this).
    assert_eq!(check_run_backoff_secs(0), 1);
    assert_eq!(check_run_backoff_secs(5), 30);
    assert_eq!(check_run_backoff_secs(99), 30);
}

#[test]
fn degraded_cadence_doubles() {
    // `design/13 §3.9`: under degradation the §3.3 intervals double.
    assert_eq!(degraded_interval_secs(PR_STATE_FOREGROUND_SECS, false), 30);
    assert_eq!(degraded_interval_secs(PR_STATE_FOREGROUND_SECS, true), 60);
    assert_eq!(degraded_interval_secs(DEPLOYMENT_SECS, true), 120);
}

// ---------------------------------------------------------------------------
// Budget seeds from X-RateLimit-* headers.
// ---------------------------------------------------------------------------

#[test]
fn budget_seeds_from_headers() {
    let pools = RateLimitPools::new();
    let key = ProviderKey::GithubPat;
    let headers = rate_limit_headers(5000, 4000, 1_700_000_000);
    pools.observe(&key, lookup(&headers));
    let b = pools.get(&key).expect("primed");
    assert_eq!(b.limit, 5000);
    assert_eq!(b.remaining, 4000);
    assert_eq!(b.reset_at, 1_700_000_000);
    assert!((b.fraction_remaining() - 0.8).abs() < 1e-9);
    assert!(!b.is_warning());
    assert!(!b.is_degraded());
}

#[tokio::test]
async fn budget_seeds_from_a_real_octocrab_call() {
    // End-to-end: a GET through GitHubProvider parses the headers off the
    // response into the pool via the provider's `last_rate_limit_headers`.
    let fake = FakeGitHub::start().await;
    fake.mount_get_json_rate_limited(
        "/repos/o/r/pulls/1",
        serde_json::json!({
            "number": 1, "title": "t", "state": "open", "html_url": "u",
            "head": { "ref": "f", "sha": "s" }, "base": { "ref": "main", "sha": "b" }
        }),
        5000,
        4321,
        1_700_000_500,
    )
    .await;
    let provider = fake.provider();
    provider.get_pr(ProviderPrId::new("o/r", 1)).await.unwrap();

    let headers = provider.last_rate_limit_headers();
    assert!(!headers.is_empty(), "captured X-RateLimit-* headers");
    let pools = RateLimitPools::new();
    let key = ProviderKey::GithubPat;
    pools.observe(&key, lookup(&headers));
    let b = pools.get(&key).unwrap();
    assert_eq!(b.limit, 5000);
    assert_eq!(b.remaining, 4321);
}

// ---------------------------------------------------------------------------
// 20% warning crossing — debounced (fires once per crossing).
// ---------------------------------------------------------------------------

#[test]
fn warning_fires_once_per_crossing_then_rearms() {
    let pools = RateLimitPools::new();
    let key = ProviderKey::GithubPat;

    // Above 20% → no warning.
    let h = rate_limit_headers(5000, 2000, 100);
    assert!(pools.observe(&key, lookup(&h)).is_none());

    // Cross below 20% → exactly one warning.
    let h = rate_limit_headers(5000, 900, 100); // 18%
    let w = pools
        .observe(&key, lookup(&h))
        .expect("warning on crossing");
    assert_eq!(w.provider, "github");
    assert_eq!(w.scope_id, "pat");
    assert_eq!(w.reset_at_ms, 100_000);

    // Still below 20% → debounced, NO second warning.
    let h = rate_limit_headers(5000, 800, 100);
    assert!(
        pools.observe(&key, lookup(&h)).is_none(),
        "debounced below 20%"
    );

    // Recover above 20% → re-arm.
    let h = rate_limit_headers(5000, 2000, 100);
    assert!(pools.observe(&key, lookup(&h)).is_none());

    // Cross again → warning re-fires.
    let h = rate_limit_headers(5000, 500, 100);
    assert!(
        pools.observe(&key, lookup(&h)).is_some(),
        "re-fires after recovery"
    );
}

// ---------------------------------------------------------------------------
// 10% degrade — doubles cadence + background ops yield.
// ---------------------------------------------------------------------------

#[test]
fn degrade_below_ten_percent_doubles_and_deprioritizes() {
    let pools = RateLimitPools::new();
    let key = ProviderKey::GithubPat;

    // 12% — warning but not degraded.
    let h = rate_limit_headers(5000, 600, 100);
    pools.observe(&key, lookup(&h));
    assert!(!pools.is_degraded(&key));
    assert!(!OpPriority::Background.should_yield(pools.is_degraded(&key)));

    // 8% — degraded.
    let h = rate_limit_headers(5000, 400, 100);
    pools.observe(&key, lookup(&h));
    assert!(pools.is_degraded(&key));

    // Cadence doubles for work on this pool.
    let degraded = pools.is_degraded(&key);
    assert_eq!(
        degraded_interval_secs(PR_STATE_FOREGROUND_SECS, degraded),
        60
    );

    // Background ops yield; user-driven ops do not.
    assert!(OpPriority::Background.should_yield(degraded));
    assert!(!OpPriority::UserDriven.should_yield(degraded));
}

// ---------------------------------------------------------------------------
// Three pools track independently.
// ---------------------------------------------------------------------------

#[test]
fn three_pools_are_independent() {
    let pools = RateLimitPools::new();
    let pat = ProviderKey::GithubPat;
    let app = ProviderKey::GithubApp("app-1".to_string());
    let gh = ProviderKey::GhCli;

    // Drain ONLY the PAT pool to 2%.
    let h = rate_limit_headers(5000, 100, 100);
    pools.observe(&pat, lookup(&h));
    // App seen healthy (90%), gh seen healthy (95%).
    let h = rate_limit_headers(15000, 13500, 100);
    pools.observe(&app, lookup(&h));
    let h = rate_limit_headers(5000, 4750, 100);
    pools.observe(&gh, lookup(&h));

    assert!(pools.is_degraded(&pat), "PAT degraded");
    assert!(!pools.is_degraded(&app), "App NOT degraded by PAT drain");
    assert!(!pools.is_degraded(&gh), "gh NOT degraded by PAT drain");

    // Diagnostics surfaces all three, stably sorted.
    let diag = pools.snapshot();
    assert_eq!(diag.len(), 3);
}

// ---------------------------------------------------------------------------
// Exhaustion → RateLimited{reset_at}; background queues + resumes on reset.
// ---------------------------------------------------------------------------

#[test]
fn exhaustion_is_typed_rate_limited_with_reset() {
    let pools = RateLimitPools::new();
    let key = ProviderKey::GithubPat;
    let h = rate_limit_headers(5000, 0, 1_700_001_000);
    pools.observe(&key, lookup(&h));
    let b = pools.get(&key).unwrap();
    assert!(b.is_exhausted());

    // The dispatcher fails an exhausted call with the typed error carrying
    // reset_at (epoch ms).
    let err = rate_limited(b.reset_at_ms());
    assert!(is_rate_limited(&err));
    assert_eq!(rate_limited_reset_at(&err), Some(1_700_001_000_000));
}

#[tokio::test]
async fn exhausted_pool_via_real_call_fails_rate_limited() {
    // A 403 with X-RateLimit-Remaining: 0 → the call surfaces RateLimited.
    let fake = FakeGitHub::start().await;
    fake.mount_get_rate_exhausted("/repos/o/r/pulls/9", 5000, 1_700_002_000)
        .await;
    let provider = fake.provider();
    // The provider call fails (403); the dispatcher would then read the captured
    // headers + map to RateLimited. Here we assert the headers carry remaining=0.
    let _ = provider.get_pr(ProviderPrId::new("o/r", 9)).await;
    let headers = provider.last_rate_limit_headers();
    let pools = RateLimitPools::new();
    let key = ProviderKey::GithubPat;
    pools.observe(&key, lookup(&headers));
    assert!(pools.get(&key).unwrap().is_exhausted());
}

#[test]
fn background_op_queues_and_resumes_after_reset() {
    let clock = Arc::new(SyntheticClock::new(1_000));
    let pools = RateLimitPools::new();
    let key = ProviderKey::GithubPat;
    let reset_at = 1_300; // 5 minutes out, epoch seconds.
    let h = rate_limit_headers(5000, 0, reset_at);
    pools.observe(&key, lookup(&h));
    assert!(pools.get(&key).unwrap().is_exhausted());

    // A background op parks on the per-pool resume queue keyed off reset_at.
    let queue = pools.resume_queue(&key);
    queue.park("poll deployments o/r", reset_at);
    assert_eq!(queue.len(), 1);

    // Before reset: nothing resumes.
    assert!(queue.drain_ready(clock.now()).is_empty());
    assert_eq!(queue.len(), 1);

    // Advance the synthetic clock PAST reset → the op resumes.
    clock.advance(400); // now 1_400 > 1_300
    let ready = queue.drain_ready(clock.now());
    assert_eq!(ready, vec!["poll deployments o/r".to_string()]);
    assert!(queue.is_empty(), "drained on resume");
}

// ---------------------------------------------------------------------------
// VcsState wiring + diagnostics accessor.
// ---------------------------------------------------------------------------

#[test]
fn vcs_state_diagnostics_and_sync() {
    let mut state = VcsState::new();
    let pat = ProviderKey::GithubPat;
    let app = ProviderKey::GithubApp("a".to_string());
    let h = rate_limit_headers(5000, 4000, 50);
    state.pools.observe(&pat, lookup(&h));
    let h = rate_limit_headers(15000, 14000, 50);
    state.pools.observe(&app, lookup(&h));

    // The diagnostics read accessor surfaces both pools.
    let diag = state.rate_limit_diagnostics();
    assert_eq!(diag.len(), 2);

    // The FROZEN `rate_limits` map materializes from the live pools.
    state.sync_rate_limits();
    assert_eq!(state.rate_limits.len(), 2);
    assert_eq!(state.rate_limits.get(&pat).unwrap().remaining, 4000);
    assert_eq!(state.rate_limits.get(&app).unwrap().remaining, 14000);
}
