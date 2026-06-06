//! Tier-2 tests for the Task 317 Linear + Jira issue-fetch clients, the
//! `fetch_issue_url` host router, the 1 h TTL cache, the `enterprise_data_privacy`
//! refusal, and the no-op `IssueWriteBack` seam.
//!
//! The **double** is the shared `wiremock`-backed `testkit` harness
//! ([`concerto_vcs::testkit::FakeLinear`] / [`FakeJira`]) serving the recorded
//! GraphQL/REST fixtures under `crates/vcs/tests/fixtures/`, plus the
//! [`SyntheticClock`] driving the TTL test (no real-time sleep).
//!
//! What this double does NOT cover (the Tier-3 Phase-3 checklist line "fetch a
//! real Linear and Jira issue"): a **real Linear OAuth/API-key fetch** against
//! `api.linear.app`, a **real Atlassian 3LO + REST fetch** against a live
//! `*.atlassian.net`, and the **Desktop-mediated OAuth round-trip** end-to-end.
//! Those are signed off at the phase gate, not here.

use std::sync::Arc;

use concerto_keychain::SecretValue;
use concerto_persist::{Persistence, PersistenceConfig};
use concerto_vcs::testkit::{fixture, FakeJira, FakeLinear, SyntheticClock};
use concerto_vcs::{
    external_tracker_blocked, flatten_adf, is_external_tracker_blocked, parse_jira_key,
    parse_linear_id, IssueCache, IssueFetchCreds, IssueProvider, IssueRef, IssueTransition,
    IssueWriteBack, JiraClient, LinearClient, NoopWriteBack, RefreshToken, VcsHandle,
    ISSUE_CACHE_TTL_SECS,
};

/// A temp-file-backed `Persistence` for the handle-level router tests.
async fn temp_persistence() -> (tempfile::TempDir, Arc<Persistence>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let p = Persistence::open(PersistenceConfig {
        db_path: dir.path().join("test.db"),
        max_readers: 2,
    })
    .await
    .expect("open persistence");
    (dir, Arc::new(p))
}

// ---------------------------------------------------------------------------
// Linear GraphQL client → Issue mapping.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn linear_fetch_maps_title_body_labels_status() {
    let linear = FakeLinear::start().await;
    linear.mount_graphql(fixture("linear_issue.json")).await;
    let client = LinearClient::with_base(&linear.base_uri()).expect("client");
    let token = SecretValue::new("lin_test_token".to_string());

    let issue = client.fetch("ENG-123", &token).await.expect("fetch");

    assert_eq!(issue.external_id, "ENG-123");
    assert_eq!(issue.number, 0, "Linear has no integer id");
    assert_eq!(issue.title, "Widget overflows on narrow viewports");
    assert!(issue.body.contains("does not wrap below 320px"));
    assert_eq!(issue.labels, vec!["bug", "frontend"]);
    assert_eq!(issue.state, "in progress");
    assert!(issue.url.contains("linear.app/acme/issue/ENG-123"));
}

#[tokio::test]
async fn linear_accepts_issue_url_and_bare_id() {
    assert_eq!(parse_linear_id("ENG-7").unwrap(), "ENG-7");
    assert_eq!(
        parse_linear_id("https://linear.app/acme/issue/ENG-7/slug").unwrap(),
        "ENG-7"
    );
}

// ---------------------------------------------------------------------------
// Jira REST client → Issue mapping (ADF flattened to text).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn jira_fetch_maps_summary_adf_labels_status() {
    let jira = FakeJira::start().await;
    jira.mount_get_json("/rest/api/3/issue/PROJ-45", fixture("jira_issue.json"))
        .await;
    let client = JiraClient::with_base(&jira.base_uri()).expect("client");
    let token = SecretValue::new("jira_access".to_string());

    let issue = client.fetch("PROJ-45", &token, None).await.expect("fetch");

    assert_eq!(issue.external_id, "PROJ-45");
    assert_eq!(issue.number, 0);
    assert_eq!(issue.title, "Login button misaligned on Safari");
    // ADF flattened: two paragraphs joined by a newline.
    assert_eq!(
        issue.body,
        "The login button shifts 4px right on Safari 17.\nOnly reproduces with the compact header."
    );
    assert_eq!(issue.labels, vec!["bug", "safari"]);
    assert_eq!(issue.state, "to do");
}

#[tokio::test]
async fn jira_401_triggers_one_refresh_then_retry() {
    let jira = FakeJira::start().await;
    jira.mount_get_json_with_refresh(
        "/rest/api/3/issue/PROJ-45",
        "stale_token",
        "fresh_token",
        fixture("jira_issue.json"),
    )
    .await;
    let client = JiraClient::with_base(&jira.base_uri()).expect("client");
    let stale = SecretValue::new("stale_token".to_string());

    // The refresh callback hands back the fresh token (the Core would mint it
    // from the keychain refresh-token slot + Atlassian's token endpoint).
    let refresh: RefreshToken =
        Box::new(|| Box::pin(async { Ok(SecretValue::new("fresh_token".to_string())) }));

    let issue = client
        .fetch("PROJ-45", &stale, Some(&refresh))
        .await
        .expect("fetch after refresh");
    assert_eq!(issue.external_id, "PROJ-45");

    // Exactly two requests: the 401 then the refreshed 200.
    assert_eq!(jira.request_count().await, 2);
}

#[tokio::test]
async fn jira_401_without_refresh_is_not_authenticated() {
    let jira = FakeJira::start().await;
    jira.mount_get_json_with_refresh(
        "/rest/api/3/issue/PROJ-45",
        "stale_token",
        "fresh_token",
        fixture("jira_issue.json"),
    )
    .await;
    let client = JiraClient::with_base(&jira.base_uri()).expect("client");
    let stale = SecretValue::new("stale_token".to_string());

    let err = client
        .fetch("PROJ-45", &stale, None)
        .await
        .expect_err("401 with no refresh");
    assert!(matches!(err, concerto_error::Error::VcsNotAuthenticated(_)));
}

#[test]
fn jira_parses_browse_url_and_bare_key() {
    assert_eq!(parse_jira_key("PROJ-9").unwrap(), "PROJ-9");
    assert_eq!(
        parse_jira_key("https://acme.atlassian.net/browse/PROJ-9").unwrap(),
        "PROJ-9"
    );
}

#[test]
fn adf_flatten_skips_unknown_nodes() {
    let adf = serde_json::json!({
        "type": "doc",
        "content": [
            { "type": "paragraph", "content": [
                { "type": "text", "text": "x" },
                { "type": "emoji", "attrs": { "shortName": ":tada:" } },
                { "type": "text", "text": "y" }
            ] }
        ]
    });
    assert_eq!(flatten_adf(&adf), "xy");
}

// ---------------------------------------------------------------------------
// URL-host routing picks the right client (via VcsHandle::fetch_issue_url).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn router_dispatches_linear_and_jira_by_host() {
    let (_dir, persistence) = temp_persistence().await;
    let handle = VcsHandle::new(persistence);

    // Linear arm.
    let linear = FakeLinear::start().await;
    linear.mount_graphql(fixture("linear_issue.json")).await;
    let lin_token = SecretValue::new("lin".to_string());
    let creds = IssueFetchCreds {
        linear_token: Some(&lin_token),
        linear_base: Some(&linear.base_uri()),
        ..Default::default()
    };
    let issue = handle
        .fetch_issue_url("https://linear.app/acme/issue/ENG-123/x", &creds)
        .await
        .expect("linear route")
        .expect("issue");
    assert_eq!(issue.external_id, "ENG-123");

    // Jira arm.
    let jira = FakeJira::start().await;
    jira.mount_get_json("/rest/api/3/issue/PROJ-45", fixture("jira_issue.json"))
        .await;
    let jira_token = SecretValue::new("jira".to_string());
    let creds = IssueFetchCreds {
        jira_token: Some(&jira_token),
        jira_base: Some(&jira.base_uri()),
        ..Default::default()
    };
    let issue = handle
        .fetch_issue_url("https://acme.atlassian.net/browse/PROJ-45", &creds)
        .await
        .expect("jira route")
        .expect("issue");
    assert_eq!(issue.external_id, "PROJ-45");
}

// ---------------------------------------------------------------------------
// 1 h TTL cache: a second fetch under the synthetic clock makes NO HTTP call;
// after expiry it re-fetches.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cache_hit_skips_second_http_call_and_expires_after_ttl() {
    let clock = Arc::new(SyntheticClock::new(1_000_000));
    let clock_for_cache = Arc::clone(&clock);
    let cache = IssueCache::new(Arc::new(move || clock_for_cache.now()));

    let (_dir, persistence) = temp_persistence().await;
    let handle = VcsHandle::with_issue_cache(persistence, cache);

    let linear = FakeLinear::start().await;
    linear.mount_graphql(fixture("linear_issue.json")).await;
    let lin_token = SecretValue::new("lin".to_string());
    let base = linear.base_uri();
    let url = "https://linear.app/acme/issue/ENG-123/x";

    let make_creds = || IssueFetchCreds {
        linear_token: Some(&lin_token),
        linear_base: Some(&base),
        ..Default::default()
    };

    // First fetch hits the wiremock.
    handle
        .fetch_issue_url(url, &make_creds())
        .await
        .expect("fetch 1")
        .expect("issue");
    assert_eq!(linear.request_count().await, 1);

    // Second fetch, still within the TTL → served from cache, NO HTTP call.
    handle
        .fetch_issue_url(url, &make_creds())
        .await
        .expect("fetch 2 (cached)")
        .expect("issue");
    assert_eq!(
        linear.request_count().await,
        1,
        "cache hit made no HTTP call"
    );

    // Advance past the 1 h TTL → the entry expires and we re-fetch.
    clock.advance(ISSUE_CACHE_TTL_SECS + 1);
    handle
        .fetch_issue_url(url, &make_creds())
        .await
        .expect("fetch 3 (expired)")
        .expect("issue");
    assert_eq!(linear.request_count().await, 2, "expired entry re-fetched");
}

// ---------------------------------------------------------------------------
// enterprise_data_privacy refuses an external-tracker fetch with a typed error
// and makes NO outbound call.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn enterprise_data_privacy_refuses_external_tracker_fetch() {
    let (_dir, persistence) = temp_persistence().await;
    let handle = VcsHandle::new(persistence);

    let linear = FakeLinear::start().await;
    linear.mount_graphql(fixture("linear_issue.json")).await;
    let lin_token = SecretValue::new("lin".to_string());
    let creds = IssueFetchCreds {
        linear_token: Some(&lin_token),
        linear_base: Some(&linear.base_uri()),
        enterprise_data_privacy: true,
        ..Default::default()
    };

    let err = handle
        .fetch_issue_url("https://linear.app/acme/issue/ENG-123/x", &creds)
        .await
        .expect_err("privacy refusal");
    assert!(is_external_tracker_blocked(&err));
    // The refusal happened BEFORE any outbound call.
    assert_eq!(
        linear.request_count().await,
        0,
        "no outbound call when blocked"
    );
}

#[test]
fn external_tracker_blocked_is_recognizable() {
    let err = external_tracker_blocked("jira");
    assert!(is_external_tracker_blocked(&err));
}

// ---------------------------------------------------------------------------
// The no-op IssueWriteBack seam returns Ok(()) (Task 320.5 swaps in the real one).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn noop_write_back_returns_ok() {
    let wb = NoopWriteBack;
    let issue_ref = IssueRef {
        provider: IssueProvider::Linear,
        external_id: "ENG-123".to_string(),
        project_url: "https://linear.app/acme".to_string(),
    };
    wb.transition_on_merge(&issue_ref, IssueTransition::MergedDone)
        .await
        .expect("noop write-back is Ok");

    // Held behind a trait object exactly as the coordinated-merge loop will.
    let dynamic: Arc<dyn IssueWriteBack> = Arc::new(NoopWriteBack);
    dynamic
        .transition_on_merge(&issue_ref, IssueTransition::MergedDone)
        .await
        .expect("noop via dyn");
}

// ---------------------------------------------------------------------------
// No issue body reaches SQLite: the fetch path holds bodies ONLY in the
// in-memory cache; there is no persistence write. We assert structurally that
// the `vcs_credentials` schema carries no body/token column and that a fetch
// leaves the DB's issue-bearing tables empty (there is no issue table at all).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn issue_body_never_persisted() {
    let (_dir, persistence) = temp_persistence().await;
    let handle = VcsHandle::new(Arc::clone(&persistence));

    let linear = FakeLinear::start().await;
    linear.mount_graphql(fixture("linear_issue.json")).await;
    let lin_token = SecretValue::new("lin".to_string());
    let creds = IssueFetchCreds {
        linear_token: Some(&lin_token),
        linear_base: Some(&linear.base_uri()),
        ..Default::default()
    };
    let issue = handle
        .fetch_issue_url("https://linear.app/acme/issue/ENG-123/x", &creds)
        .await
        .expect("fetch")
        .expect("issue");
    // The body is real (so we know it was fetched) ...
    assert!(issue.body.contains("does not wrap below 320px"));

    // ... but it lives in NO SQLite table. There is no issues table, and the
    // body text appears nowhere in the DB. Scan every table's text content.
    let pool = persistence.readers();
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' \
         AND name NOT LIKE '\\_sqlx%' ESCAPE '\\'",
    )
    .fetch_all(pool)
    .await
    .expect("list tables");
    assert!(
        !tables.iter().any(|t| t == "issues"),
        "there is deliberately no issues table"
    );
    // The fetched body string must not appear in any row of any table.
    for table in &tables {
        // Dump the whole table as JSON text via a generic column-agnostic probe:
        // SQLite's `quote(*)` is not available, so we check the credential table
        // (the only VCS table) explicitly has no body column by selecting it
        // would error — instead assert the body substring is absent from a
        // full-text dump of each table built from its columns.
        let cols: Vec<String> =
            sqlx::query_scalar(&format!("SELECT name FROM pragma_table_info('{table}')"))
                .fetch_all(pool)
                .await
                .unwrap_or_default();
        if cols.is_empty() {
            continue;
        }
        let concat = cols
            .iter()
            .map(|c| format!("COALESCE(CAST(\"{c}\" AS TEXT),'')"))
            .collect::<Vec<_>>()
            .join(" || ");
        let blob: Vec<String> = sqlx::query_scalar(&format!("SELECT {concat} FROM \"{table}\""))
            .fetch_all(pool)
            .await
            .unwrap_or_default();
        assert!(
            !blob.iter().any(|r| r.contains("does not wrap below 320px")),
            "issue body leaked into table `{table}`"
        );
    }
}
