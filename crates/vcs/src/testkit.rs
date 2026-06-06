//! `#[cfg(feature = "testkit")]` shared VCS test double (Task 313, D2 /
//! `design/13 §10`).
//!
//! The ONE wiremock-backed fixture harness 314/315/316/317/320/320.5 reuse
//! (mirrors how Phase 2 built the loopback-Iroh double once). Consumers enable
//! it as a dev-dependency:
//!
//! ```toml
//! [dev-dependencies]
//! concerto-vcs = { path = "../vcs", features = ["testkit"] }
//! ```
//!
//! It exposes:
//! - [`FakeGitHub`] / [`FakeLinear`] / [`FakeJira`] — each owns a
//!   [`wiremock::MockServer`], exposes its base URL, mounts recorded fixtures,
//!   and (for GitHub) builds a [`GitHubProvider`] pointed at the mock base.
//! - [`SyntheticClock`] + [`rate_limit_headers`] — the synthetic-clock + the
//!   `X-RateLimit-*` header hook Task 314 consumes to drive its dual rate-limit
//!   pools deterministically.
//! - [`fixture`] — load a recorded JSON fixture from `crates/vcs/tests/fixtures/`.
//!
//! The harness proves provider logic against recorded responses; it does NOT
//! cover the real GitHub API round-trip (the Tier-3 phase-checklist line).

use std::sync::atomic::{AtomicI64, Ordering};

use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::github::GitHubProvider;

/// A wiremock-backed fake GitHub REST + GraphQL endpoint.
pub struct FakeGitHub {
    server: MockServer,
}

impl FakeGitHub {
    /// Start a fresh fake GitHub on an ephemeral loopback port.
    pub async fn start() -> Self {
        Self {
            server: MockServer::start().await,
        }
    }

    /// The base URI to point a [`GitHubProvider`] at (e.g. `http://127.0.0.1:PORT`).
    pub fn base_uri(&self) -> String {
        self.server.uri()
    }

    /// The underlying server, for tests that need to register bespoke mocks.
    pub fn server(&self) -> &MockServer {
        &self.server
    }

    /// Build a [`GitHubProvider`] authenticated with a dummy token, pointed at
    /// this fake's base URI. The token is irrelevant to the mock; it satisfies
    /// the builder.
    pub fn provider(&self) -> GitHubProvider {
        GitHubProvider::with_token_and_base("test-token", &self.base_uri())
            .expect("build GitHubProvider against wiremock base")
    }

    /// Mount a `GET <path>` → JSON-body response (status 200).
    pub async fn mount_get_json(&self, path_str: &str, body: serde_json::Value) {
        Mock::given(method("GET"))
            .and(path(path_str))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Mount a `GET <path>?<key>=<value>` → JSON-body response (status 200).
    pub async fn mount_get_json_q(
        &self,
        path_str: &str,
        key: &str,
        value: &str,
        body: serde_json::Value,
    ) {
        Mock::given(method("GET"))
            .and(path(path_str))
            .and(query_param(key, value))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Mount a `POST <path>` → JSON-body response with `status`.
    pub async fn mount_post_json(&self, path_str: &str, status: u16, body: serde_json::Value) {
        Mock::given(method("POST"))
            .and(path(path_str))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Mount a `PUT <path>` → JSON-body response with `status`.
    pub async fn mount_put_json(&self, path_str: &str, status: u16, body: serde_json::Value) {
        Mock::given(method("PUT"))
            .and(path(path_str))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Mount a `GET <path>` → JSON body **with synthetic `X-RateLimit-*`
    /// headers** (Task 314's rate-limit pool double). `remaining`/`reset_at`
    /// are echoed verbatim so 314 can assert its budget math.
    pub async fn mount_get_json_rate_limited(
        &self,
        path_str: &str,
        body: serde_json::Value,
        limit: u32,
        remaining: u32,
        reset_at: i64,
    ) {
        let mut tmpl = ResponseTemplate::new(200).set_body_json(body);
        for (k, v) in rate_limit_headers(limit, remaining, reset_at) {
            tmpl = tmpl.append_header(k.as_str(), v.as_str());
        }
        Mock::given(method("GET"))
            .and(path(path_str))
            .respond_with(tmpl)
            .mount(&self.server)
            .await;
    }

    /// Mount a `PUT <path>` → JSON body **with synthetic `X-RateLimit-*`
    /// headers** (Task 314 — the merge path bills the rate-limit pool too).
    pub async fn mount_put_json_rate_limited(
        &self,
        path_str: &str,
        body: serde_json::Value,
        limit: u32,
        remaining: u32,
        reset_at: i64,
    ) {
        let mut tmpl = ResponseTemplate::new(200).set_body_json(body);
        for (k, v) in rate_limit_headers(limit, remaining, reset_at) {
            tmpl = tmpl.append_header(k.as_str(), v.as_str());
        }
        Mock::given(method("PUT"))
            .and(path(path_str))
            .respond_with(tmpl)
            .mount(&self.server)
            .await;
    }

    /// Mount a `POST /graphql` → JSON response (status 200), the GitHub GraphQL
    /// review-thread query/mutation double (Task 316). When a test needs to
    /// distinguish the query from the mutation, use
    /// [`FakeGitHub::mount_graphql_matching`] instead (body substring match).
    pub async fn mount_graphql(&self, body: serde_json::Value) {
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Mount a `POST /graphql` whose request body contains `needle` → JSON
    /// `body` (Task 316). Lets one fake serve the review-thread query and the
    /// `resolveReviewThread` mutation distinctly (match on `"reviewThreads"`
    /// vs `"resolveReviewThread"`). wiremock matches the most-recently-mounted
    /// matcher first, so mount the more specific needle last.
    pub async fn mount_graphql_matching(&self, needle: &str, body: serde_json::Value) {
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(wiremock::matchers::body_string_contains(needle))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Count the `POST /graphql` requests the fake has served (Task 316's
    /// cache-hit assertion: a cached read makes NO second GraphQL call).
    pub async fn graphql_request_count(&self) -> usize {
        self.server
            .received_requests()
            .await
            .map(|reqs| {
                reqs.iter()
                    .filter(|r| {
                        r.method == wiremock::http::Method::POST && r.url.path() == "/graphql"
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    /// Mount a `GET <path>` → `403` with `X-RateLimit-Remaining: 0` (the
    /// `design/13 §8` exhaustion row). Task 314 asserts the call fails with the
    /// typed `RateLimited{reset_at}` error.
    pub async fn mount_get_rate_exhausted(&self, path_str: &str, limit: u32, reset_at: i64) {
        let mut tmpl = ResponseTemplate::new(403)
            .set_body_json(serde_json::json!({ "message": "API rate limit exceeded" }));
        for (k, v) in rate_limit_headers(limit, 0, reset_at) {
            tmpl = tmpl.append_header(k.as_str(), v.as_str());
        }
        Mock::given(method("GET"))
            .and(path(path_str))
            .respond_with(tmpl)
            .mount(&self.server)
            .await;
    }

    /// Script the GitHub **App installation-token** endpoint
    /// (`POST /app/installations/{installation_id}/access_tokens`) — Task 314's
    /// App-auth + transparent-refresh double. Every POST returns `{token,
    /// expires_at}` with the supplied `token` + an RFC3339 `expires_at`
    /// `expires_in_secs` after `minted_at_secs`. Mount this once; each refresh
    /// the provider issues is one more POST the test can count via
    /// [`FakeGitHub::token_mint_count`].
    pub async fn mount_app_token(
        &self,
        installation_id: u64,
        token: &str,
        minted_at_secs: i64,
        expires_in_secs: i64,
    ) {
        let route = format!("/app/installations/{installation_id}/access_tokens");
        let expires_at = epoch_secs_to_rfc3339(minted_at_secs + expires_in_secs);
        Mock::given(method("POST"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "token": token,
                "expires_at": expires_at,
            })))
            .mount(&self.server)
            .await;
    }

    /// How many App-token POSTs the fake has served (Task 314's refresh
    /// assertion: a refresh after the synthetic clock passes expiry is one more
    /// mint). Counts only the `…/access_tokens` route.
    pub async fn token_mint_count(&self) -> usize {
        self.server
            .received_requests()
            .await
            .map(|reqs| {
                reqs.iter()
                    .filter(|r| {
                        r.method == wiremock::http::Method::POST
                            && r.url.path().ends_with("/access_tokens")
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    /// Build an App-installation [`GitHubProvider`] (Task 314) pointed at this
    /// fake's base. `private_key_pem` is a throwaway RSA key (the fake never
    /// verifies the JWT); `now` is the synthetic clock so the
    /// expiry/refresh path is deterministic.
    pub fn app_provider(
        &self,
        app_id: u64,
        installation_id: u64,
        private_key_pem: &[u8],
        now: crate::github::NowSecs,
    ) -> GitHubProvider {
        GitHubProvider::with_app_installation_and_base(
            app_id,
            installation_id,
            private_key_pem,
            &self.base_uri(),
            now,
        )
        .expect("build App GitHubProvider against wiremock base")
    }
}

/// Format epoch seconds as an RFC3339 UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`) —
/// the shape GitHub's token endpoint returns, the inverse of `github.rs`'s
/// `parse_rfc3339_secs`. Used by [`FakeGitHub::mount_app_token`].
pub fn epoch_secs_to_rfc3339(secs: i64) -> String {
    // days since epoch → civil date (Howard Hinnant's algorithm).
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hour, min, sec) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// A throwaway 2048-bit RSA private key (PKCS#8 PEM) for the Task-314 App-auth
/// tests. The fake token endpoint never verifies the JWT signature, so a static
/// test key is fine; this only needs to parse as a valid RSA key so
/// `EncodingKey::from_rsa_pem` succeeds. NOT a real credential.
pub const TEST_APP_PRIVATE_KEY_PEM: &[u8] = include_bytes!("../tests/fixtures/test_app_key.pem");

/// A wiremock-backed fake Linear GraphQL endpoint (Task 317 fills the query
/// fixtures + the client). The harness shape is frozen now.
pub struct FakeLinear {
    server: MockServer,
}

impl FakeLinear {
    /// Start a fresh fake Linear.
    pub async fn start() -> Self {
        Self {
            server: MockServer::start().await,
        }
    }

    /// The base URI (Linear's GraphQL endpoint is `/graphql`).
    pub fn base_uri(&self) -> String {
        self.server.uri()
    }

    /// The underlying server.
    pub fn server(&self) -> &MockServer {
        &self.server
    }

    /// Mount a `POST /graphql` → JSON response (status 200).
    pub async fn mount_graphql(&self, body: serde_json::Value) {
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Number of requests the fake has received so far. Lets the 1 h-cache test
    /// assert the second fetch served from cache made NO second HTTP call.
    pub async fn request_count(&self) -> usize {
        self.server
            .received_requests()
            .await
            .map(|r| r.len())
            .unwrap_or(0)
    }
}

/// A wiremock-backed fake Jira (Atlassian) REST endpoint (Task 317). Frozen
/// harness shape.
pub struct FakeJira {
    server: MockServer,
}

impl FakeJira {
    /// Start a fresh fake Jira.
    pub async fn start() -> Self {
        Self {
            server: MockServer::start().await,
        }
    }

    /// The base URI.
    pub fn base_uri(&self) -> String {
        self.server.uri()
    }

    /// The underlying server.
    pub fn server(&self) -> &MockServer {
        &self.server
    }

    /// Mount a `GET <path>` → JSON response (status 200).
    pub async fn mount_get_json(&self, path_str: &str, body: serde_json::Value) {
        Mock::given(method("GET"))
            .and(path(path_str))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Mount the **OAuth-refresh** flow for the Jira 401→refresh→retry test
    /// (Task 317): a request bearing `stale_token` gets a `401`; a request
    /// bearing `fresh_token` gets the `200` body. Lets a single test prove the
    /// client transparently refreshes and retries once.
    pub async fn mount_get_json_with_refresh(
        &self,
        path_str: &str,
        stale_token: &str,
        fresh_token: &str,
        body: serde_json::Value,
    ) {
        // Stale bearer → 401.
        Mock::given(method("GET"))
            .and(path(path_str))
            .and(header(
                "authorization",
                format!("Bearer {stale_token}").as_str(),
            ))
            .respond_with(ResponseTemplate::new(401))
            .mount(&self.server)
            .await;
        // Fresh bearer → 200 + body.
        Mock::given(method("GET"))
            .and(path(path_str))
            .and(header(
                "authorization",
                format!("Bearer {fresh_token}").as_str(),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    /// Number of requests received (cache / retry assertions).
    pub async fn request_count(&self) -> usize {
        self.server
            .received_requests()
            .await
            .map(|r| r.len())
            .unwrap_or(0)
    }
}

/// The synthetic `X-RateLimit-*` header set GitHub returns (`design/13 §3.9`).
/// Returned as `(name, value)` pairs so a test can attach them to any response.
/// Task 314 parses these to drive its rate-limit budgets without a live clock.
pub fn rate_limit_headers(limit: u32, remaining: u32, reset_at: i64) -> Vec<(String, String)> {
    vec![
        ("x-ratelimit-limit".to_string(), limit.to_string()),
        ("x-ratelimit-remaining".to_string(), remaining.to_string()),
        ("x-ratelimit-reset".to_string(), reset_at.to_string()),
    ]
}

/// A deterministic clock for the rate-limit / polling-cadence tests
/// (`design/13 §3.3`/§3.9, Task 314). Starts at a caller-supplied epoch-seconds
/// value; [`SyntheticClock::advance`] moves it forward; [`SyntheticClock::now`]
/// reads it. Thread-safe so a shared budget can read it from multiple tasks.
pub struct SyntheticClock {
    now_secs: AtomicI64,
}

impl SyntheticClock {
    /// Start the clock at `start_secs` (epoch seconds).
    pub fn new(start_secs: i64) -> Self {
        Self {
            now_secs: AtomicI64::new(start_secs),
        }
    }

    /// Current time, epoch seconds.
    pub fn now(&self) -> i64 {
        self.now_secs.load(Ordering::SeqCst)
    }

    /// Advance the clock by `secs` and return the new time.
    pub fn advance(&self, secs: i64) -> i64 {
        self.now_secs.fetch_add(secs, Ordering::SeqCst) + secs
    }
}

/// Load a recorded fixture from `crates/vcs/tests/fixtures/<name>` and parse it
/// as JSON. Panics with a clear message if the file is missing or malformed —
/// these are checked-in test assets, so a failure is a test-author bug.
pub fn fixture(name: &str) -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    let bytes =
        std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("parse fixture {} as JSON: {e}", path.display()))
}
