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
}

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
