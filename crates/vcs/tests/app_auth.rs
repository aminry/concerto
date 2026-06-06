//! Tier-2 tests for the GitHub **App** installation-token auth path (Task 314,
//! R-7) against 313's `testkit` `FakeGitHub` (synthetic App-token endpoint +
//! synthetic clock). No real GitHub, no real App.
//!
//! What this double does NOT cover: a real GitHub App installation-token mint
//! against GitHub + a real degraded-cadence transition under live load — the
//! Tier-3 Phase-3 checklist line "mint a real GitHub App installation token +
//! observe a real degraded-cadence transition".

use std::sync::Arc;

use concerto_vcs::testkit::{FakeGitHub, SyntheticClock, TEST_APP_PRIVATE_KEY_PEM};
use concerto_vcs::{GithubNowSecs, ProviderPrId, VcsProvider};

const APP_ID: u64 = 12345;
const INSTALLATION_ID: u64 = 99;

/// A `NowSecs` closure backed by a shared synthetic clock.
fn clock_now(clock: &Arc<SyntheticClock>) -> GithubNowSecs {
    let c = Arc::clone(clock);
    Arc::new(move || c.now())
}

#[tokio::test]
async fn mints_installation_token_and_calls_with_it() {
    let fake = FakeGitHub::start().await;
    let clock = Arc::new(SyntheticClock::new(1_000_000));
    // Token valid for 1h.
    fake.mount_app_token(INSTALLATION_ID, "ghs_install_token", clock.now(), 3600)
        .await;
    // A PR GET that requires the installation token to be attached.
    fake.mount_get_json(
        "/repos/o/r/pulls/7",
        serde_json::json!({
            "number": 7, "title": "t", "state": "open",
            "html_url": "https://github.com/o/r/pull/7",
            "head": { "ref": "f", "sha": "abc" },
            "base": { "ref": "main", "sha": "def" }
        }),
    )
    .await;

    let provider = fake.app_provider(
        APP_ID,
        INSTALLATION_ID,
        TEST_APP_PRIVATE_KEY_PEM,
        clock_now(&clock),
    );
    let pr = provider
        .get_pr(ProviderPrId::new("o/r", 7))
        .await
        .expect("get_pr via App token");
    assert_eq!(pr.id.number, 7);
    // Exactly one token mint happened (then cached).
    assert_eq!(fake.token_mint_count().await, 1, "one mint on first call");
}

#[tokio::test]
async fn caches_token_across_calls_no_remint() {
    let fake = FakeGitHub::start().await;
    let clock = Arc::new(SyntheticClock::new(2_000_000));
    fake.mount_app_token(INSTALLATION_ID, "ghs_a", clock.now(), 3600)
        .await;
    fake.mount_get_json(
        "/repos/o/r/pulls/1",
        serde_json::json!({
            "number": 1, "title": "t", "state": "open",
            "html_url": "u", "head": { "ref": "f", "sha": "s" },
            "base": { "ref": "main", "sha": "b" }
        }),
    )
    .await;

    let provider = fake.app_provider(
        APP_ID,
        INSTALLATION_ID,
        TEST_APP_PRIVATE_KEY_PEM,
        clock_now(&clock),
    );
    for _ in 0..3 {
        provider.get_pr(ProviderPrId::new("o/r", 1)).await.unwrap();
    }
    // Three API calls, but the token is minted ONCE (cached, still fresh).
    assert_eq!(
        fake.token_mint_count().await,
        1,
        "cached token reused across calls"
    );
}

#[tokio::test]
async fn refreshes_token_when_synthetic_clock_passes_expiry() {
    let fake = FakeGitHub::start().await;
    let start = 3_000_000;
    let clock = Arc::new(SyntheticClock::new(start));
    // Token valid for 1h; mount returns expires_at = mint_time + 3600 each POST.
    fake.mount_app_token(INSTALLATION_ID, "ghs_x", start, 3600)
        .await;
    fake.mount_get_json(
        "/repos/o/r/pulls/2",
        serde_json::json!({
            "number": 2, "title": "t", "state": "open",
            "html_url": "u", "head": { "ref": "f", "sha": "s" },
            "base": { "ref": "main", "sha": "b" }
        }),
    )
    .await;

    let provider = fake.app_provider(
        APP_ID,
        INSTALLATION_ID,
        TEST_APP_PRIVATE_KEY_PEM,
        clock_now(&clock),
    );
    // First call mints.
    provider.get_pr(ProviderPrId::new("o/r", 2)).await.unwrap();
    assert_eq!(fake.token_mint_count().await, 1);

    // Advance the synthetic clock PAST expiry (1h + skew) → the next call must
    // re-mint transparently.
    clock.advance(3600 + 120);
    provider.get_pr(ProviderPrId::new("o/r", 2)).await.unwrap();
    assert_eq!(
        fake.token_mint_count().await,
        2,
        "expired token triggers a transparent refresh"
    );
}

#[tokio::test]
async fn rejects_malformed_private_key() {
    let clock = Arc::new(SyntheticClock::new(0));
    let result = concerto_vcs::GitHubProvider::with_app_installation(
        APP_ID,
        INSTALLATION_ID,
        b"not a pem key",
        clock_now(&clock),
    );
    let err = match result {
        Ok(_) => panic!("malformed PEM should have been rejected"),
        Err(e) => e,
    };
    // The error names the failure but NEVER echoes key material.
    let msg = format!("{err}");
    assert!(msg.contains("private key"), "got: {msg}");
}
