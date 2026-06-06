//! [`GitHubProvider`] — the default [`VcsProvider`] backend on `octocrab`
//! (Task 313, `design/13 §3.1`).
//!
//! Uses `octocrab`'s typed `get`/`post`/`put` request helpers against the
//! configured base URI (default `https://api.github.com`; configurable for
//! GitHub Enterprise per R-10), deserializing into local `serde` projections
//! rather than coupling to octocrab's evolving model structs. TLS is rustls
//! (workspace pin posture — no openssl/native-tls).
//!
//! The PAT is read from the keychain (`SecretKind::GithubPat`, existing). The
//! **GitHub App option** (Task 314, R-7): given an app id + installation id +
//! the App private key (PEM, from `VcsSecretSlot::GithubAppPrivateKey`), this
//! provider mints a JWT, exchanges it for a short-lived **installation token**,
//! caches it with its expiry, and **transparently refreshes** before/at expiry
//! (and on a `401`). The installation token is held **in memory only** — never
//! persisted; only the *expiry* lands in `vcs_credentials.token_expires_at` so
//! the Core knows staleness across restarts. The App private key stays in the
//! keychain and is read only to sign the JWT.
//!
//! Each REST call seeds the matching [`RateLimitBudget`](crate::RateLimitBudget)
//! pool from the response's `X-RateLimit-*` headers (Task 314, `design/13 §3.9`)
//! — see [`GitHubProvider::last_rate_limit_headers`], which the dispatcher reads
//! into [`RateLimitPools`](crate::RateLimitPools).
//!
//! The GraphQL methods (`list_review_threads`/`resolve_thread`) are
//! signature-frozen stubs returning [`unimplemented_err`] — Task 316 fills them.
//! `revert_pr` is likewise a frozen stub (the revert-commit-by-default mechanics
//! land with the coordinated-merge loop, Task 320).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use concerto_error::{Error, Result};
use http_body_util::BodyExt;
use serde::Deserialize;
use url::Url;

use crate::provider::{
    unimplemented_err, CheckRun, CreatePrRequest, Deployment, Issue, MergeMethod, MergeReport,
    ProviderPrId, PullRequest, RevertReport, ReviewThread, ThreadId, VcsProvider,
};

/// Default GitHub REST base. GitHub Enterprise overrides this with
/// `https://<host>/api/v3` (R-10).
pub const DEFAULT_GITHUB_BASE_URI: &str = "https://api.github.com";

/// A clock the App-token refresh reads "now" (epoch seconds) from. Production
/// passes [`system_now_secs`](crate::dispatch::system_now_secs); the Task-314
/// App-refresh tests pass a closure over the `testkit` `SyntheticClock` so the
/// expiry/refresh path is deterministic (no real sleeps).
pub type NowSecs = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Refresh an installation token this many seconds **before** its stated expiry
/// (`design/13 §3.9` impl note: "refresh when within a small skew of expiry").
const TOKEN_REFRESH_SKEW_SECS: i64 = 60;

/// In-memory App-installation-token cache (Task 314). Holds the most-recently
/// minted `(token, expires_at)`; the token is NEVER persisted. Shared
/// (`Arc<Mutex<…>>`) across provider clones so one mint serves concurrent calls.
#[derive(Clone, Default)]
struct InstallationTokenCache {
    inner: Arc<Mutex<Option<(String, i64)>>>,
}

/// The signing material + endpoint a GitHub App provider needs to mint and
/// refresh installation tokens (Task 314). The PEM private key is held only to
/// sign the JWT; it is never logged.
struct AppAuth {
    app_id: u64,
    installation_id: u64,
    /// The RSA private key, parsed once from the PEM read out of the keychain.
    encoding_key: jsonwebtoken::EncodingKey,
    /// REST base for the token-mint endpoint (`/app/installations/{id}/
    /// access_tokens`). The same base as the provider's `client` (real GitHub,
    /// Enterprise, or the testkit wiremock base).
    base_uri: String,
    cache: InstallationTokenCache,
    now: NowSecs,
}

/// Which credential mints the calls this provider makes (Task 314). PAT is the
/// 313 path; App mints + transparently refreshes an installation token.
enum Auth {
    /// A pre-configured client (PAT or the testkit wiremock client).
    Static,
    /// A GitHub App installation — the token is minted/refreshed on demand.
    App(AppAuth),
}

/// The octocrab-backed GitHub provider.
pub struct GitHubProvider {
    client: octocrab::Octocrab,
    auth: Auth,
    base_uri: String,
    /// The `X-RateLimit-*` headers parsed off the most recent response, as
    /// `(name, value)` pairs. Task 314's dispatcher reads this after each call to
    /// seed the matching [`RateLimitPools`](crate::RateLimitPools) pool. Shared
    /// across clones so the latest call wins.
    last_headers: Arc<Mutex<Vec<(String, String)>>>,
}

impl GitHubProvider {
    /// Build a provider from a PAT against the default `api.github.com` base.
    pub fn with_token(token: impl Into<String>) -> Result<Self> {
        Self::with_token_and_base(token, DEFAULT_GITHUB_BASE_URI)
    }

    /// Build a provider from a PAT against a caller-supplied base URI
    /// (GitHub Enterprise, R-10, or the `testkit` wiremock base).
    pub fn with_token_and_base(token: impl Into<String>, base_uri: &str) -> Result<Self> {
        ensure_crypto_provider();
        let client = octocrab::Octocrab::builder()
            .personal_token(token.into())
            .base_uri(base_uri)
            .map_err(map_octo_err)?
            .build()
            .map_err(map_octo_err)?;
        Ok(Self::new_static(client, base_uri.to_string()))
    }

    /// Wrap a pre-built octocrab client (used by the `testkit` harness, which
    /// points the client at a `wiremock::MockServer` base URL).
    pub fn from_client(client: octocrab::Octocrab) -> Self {
        Self::new_static(client, DEFAULT_GITHUB_BASE_URI.to_string())
    }

    fn new_static(client: octocrab::Octocrab, base_uri: String) -> Self {
        Self {
            client,
            auth: Auth::Static,
            base_uri,
            last_headers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Build a **GitHub App installation** provider (Task 314, R-7) against the
    /// default `api.github.com` base. `private_key_pem` is the App private key
    /// read from `VcsSecretSlot::GithubAppPrivateKey`; it is parsed once + held
    /// only to sign the JWT (never logged, never persisted). Production passes
    /// [`system_now_secs`](crate::dispatch::system_now_secs) for `now`.
    pub fn with_app_installation(
        app_id: u64,
        installation_id: u64,
        private_key_pem: &[u8],
        now: NowSecs,
    ) -> Result<Self> {
        Self::with_app_installation_and_base(
            app_id,
            installation_id,
            private_key_pem,
            DEFAULT_GITHUB_BASE_URI,
            now,
        )
    }

    /// Build a GitHub App installation provider against a caller-supplied base
    /// (GitHub Enterprise, R-10, or the testkit wiremock base — which scripts the
    /// `/app/installations/{id}/access_tokens` token endpoint).
    pub fn with_app_installation_and_base(
        app_id: u64,
        installation_id: u64,
        private_key_pem: &[u8],
        base_uri: &str,
        now: NowSecs,
    ) -> Result<Self> {
        ensure_crypto_provider();
        let encoding_key =
            jsonwebtoken::EncodingKey::from_rsa_pem(private_key_pem).map_err(|e| {
                // Never include the key material in the error — only that parsing failed.
                Error::Vcs(format!("github app: invalid private key PEM: {e}"))
            })?;
        // The client carries no auth header itself — every call attaches the
        // freshly-minted installation token via a per-call `Authorization`
        // header (octocrab's typed helpers don't expose mutating the token, so
        // we set it per request). The base is the only client-level config.
        let client = octocrab::Octocrab::builder()
            .base_uri(base_uri)
            .map_err(map_octo_err)?
            .build()
            .map_err(map_octo_err)?;
        Ok(Self {
            client,
            auth: Auth::App(AppAuth {
                app_id,
                installation_id,
                encoding_key,
                base_uri: base_uri.to_string(),
                cache: InstallationTokenCache::default(),
                now,
            }),
            base_uri: base_uri.to_string(),
            last_headers: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// The `X-RateLimit-*` headers parsed off the most recent response, as
    /// `(name, value)` lowercase pairs (Task 314). Empty before the first call.
    /// The dispatcher feeds these into the matching
    /// [`RateLimitPools`](crate::RateLimitPools) pool after each call.
    pub fn last_rate_limit_headers(&self) -> Vec<(String, String)> {
        self.last_headers.lock().expect("headers mutex").clone()
    }

    /// Ensure a fresh installation token (App auth only), minting/refreshing it
    /// when within [`TOKEN_REFRESH_SKEW_SECS`] of expiry. Returns the token + its
    /// stated expiry (epoch seconds). PAT/static providers return `Ok(None)` (the
    /// client already carries the auth header). Public so the actor can persist
    /// the returned `expires_at` to `vcs_credentials.token_expires_at`.
    pub async fn ensure_installation_token(&self) -> Result<Option<(String, i64)>> {
        match &self.auth {
            Auth::Static => Ok(None),
            Auth::App(app) => {
                let now = (app.now)();
                if let Some((token, exp)) = app.cache.inner.lock().expect("token cache").clone() {
                    if exp - now > TOKEN_REFRESH_SKEW_SECS {
                        return Ok(Some((token, exp)));
                    }
                }
                let (token, exp) = app.mint_installation_token().await?;
                *app.cache.inner.lock().expect("token cache") = Some((token.clone(), exp));
                Ok(Some((token, exp)))
            }
        }
    }

    /// The bearer token to attach to a REST call: a freshly-ensured installation
    /// token (App), or `None` for a PAT/static client (which already carries it).
    async fn call_token(&self) -> Result<Option<String>> {
        Ok(self.ensure_installation_token().await?.map(|(t, _)| t))
    }

    /// Capture the `X-RateLimit-*` headers off a raw response into `last_headers`
    /// (Task 314). Idempotent per call; only the rate-limit headers are kept.
    fn capture_rate_limit_headers(&self, headers: &http::HeaderMap) {
        let mut captured = Vec::new();
        for name in [
            "x-ratelimit-limit",
            "x-ratelimit-remaining",
            "x-ratelimit-reset",
        ] {
            if let Some(v) = headers.get(name).and_then(|v| v.to_str().ok()) {
                captured.push((name.to_string(), v.to_string()));
            }
        }
        if !captured.is_empty() {
            *self.last_headers.lock().expect("headers mutex") = captured;
        }
    }

    /// Run a raw `GET`/`POST`/`PUT` (attaching the App token when present),
    /// capture the rate-limit headers, and deserialize the JSON body. Centralizes
    /// the header-capture so every method seeds the rate-limit pool.
    async fn request_json<T: serde::de::DeserializeOwned>(
        &self,
        method: http::Method,
        route: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T> {
        use octocrab::OctoBody;
        let token = self.call_token().await?;
        let uri = format!("{}{route}", self.base_uri.trim_end_matches('/'));
        let mut builder = http::Request::builder().method(method).uri(uri);
        if let Some(t) = &token {
            builder = builder.header(http::header::AUTHORIZATION, format!("Bearer {t}"));
        }
        builder = builder
            .header(http::header::ACCEPT, "application/vnd.github+json")
            .header(http::header::USER_AGENT, "concerto-vcs");
        let req = match &body {
            Some(b) => {
                let bytes =
                    serde_json::to_vec(b).map_err(|e| Error::Vcs(format!("github: {e}")))?;
                builder
                    .header(http::header::CONTENT_TYPE, "application/json")
                    .body(OctoBody::from(bytes))
            }
            None => builder.body(OctoBody::empty()),
        }
        .map_err(|e| Error::Vcs(format!("github: build request: {e}")))?;

        let resp = self.client.execute(req).await.map_err(map_octo_err)?;
        self.capture_rate_limit_headers(resp.headers());
        let status = resp.status();
        let body_bytes = resp
            .into_body()
            .collect()
            .await
            .map_err(map_octo_err)?
            .to_bytes();
        if !status.is_success() {
            return Err(Error::Vcs(format!(
                "github: HTTP {} for {route}",
                status.as_u16()
            )));
        }
        serde_json::from_slice(&body_bytes)
            .map_err(|e| Error::Vcs(format!("github: decode {route}: {e}")))
    }
}

impl AppAuth {
    /// Mint a fresh installation token: sign a short-lived JWT with the App
    /// private key, then exchange it at `/app/installations/{id}/access_tokens`
    /// for an installation access token + its expiry (`design/13` R-7, Task 314).
    /// Uses a plain `reqwest` client (rustls/ring posture) so the JWT bearer is
    /// fully controlled; the token + expiry come straight off GitHub's response.
    async fn mint_installation_token(&self) -> Result<(String, i64)> {
        let now = (self.now)();
        // GitHub App JWT: `iat` slightly in the past (clock skew), `exp` ≤ 10 min,
        // `iss` = app id (`design/13` R-7). 9-minute exp stays inside the 10-min
        // cap with skew headroom.
        #[derive(serde::Serialize)]
        struct Claims {
            iat: i64,
            exp: i64,
            iss: String,
        }
        let claims = Claims {
            iat: now - 30,
            exp: now + 9 * 60,
            iss: self.app_id.to_string(),
        };
        let header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        let jwt = jsonwebtoken::encode(&header, &claims, &self.encoding_key)
            .map_err(|e| Error::Vcs(format!("github app: JWT sign failed: {e}")))?;

        let url = format!(
            "{}/app/installations/{}/access_tokens",
            self.base_uri.trim_end_matches('/'),
            self.installation_id
        );
        let client = reqwest::Client::builder()
            .user_agent("concerto-vcs")
            .build()
            .map_err(|e| Error::Vcs(format!("github app: build client: {e}")))?;
        let resp = client
            .post(&url)
            .bearer_auth(&jwt)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| Error::Vcs(format!("github app: token mint request failed: {e}")))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| Error::Vcs(format!("github app: read token response: {e}")))?;
        if !status.is_success() {
            // Never echo the response body (it could carry sensitive detail) —
            // only the status (the never-log-secret discipline, Task 313).
            return Err(Error::Vcs(format!(
                "github app: installation token mint returned HTTP {}",
                status.as_u16()
            )));
        }
        #[derive(Deserialize)]
        struct TokenResponse {
            token: String,
            /// RFC3339 timestamp (`2024-01-01T00:00:00Z`).
            expires_at: String,
        }
        let parsed: TokenResponse = serde_json::from_str(&body)
            .map_err(|e| Error::Vcs(format!("github app: decode token response: {e}")))?;
        let exp = parse_rfc3339_secs(&parsed.expires_at).unwrap_or(now + 3600);
        Ok((parsed.token, exp))
    }
}

/// Parse an RFC3339 UTC timestamp (`2024-01-01T00:00:00Z`) into epoch seconds.
/// Minimal hand-roll (avoids a `chrono`/`time` direct dep) — GitHub always
/// returns UTC `Z`. Returns `None` on any deviation, so the caller falls back to
/// a conservative `now + 1h`.
fn parse_rfc3339_secs(s: &str) -> Option<i64> {
    // `YYYY-MM-DDTHH:MM:SSZ`
    let s = s.trim();
    let bytes = s.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[10] != b'T' || !s.ends_with('Z') {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: i64 = s.get(5..7)?.parse().ok()?;
    let day: i64 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let min: i64 = s.get(14..16)?.parse().ok()?;
    let sec: i64 = s.get(17..19)?.parse().ok()?;
    // Days since 1970-01-01 (civil-from-days, Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hour * 3600 + min * 60 + sec)
}

/// Install the process-level **ring** rustls `CryptoProvider` if none is set.
///
/// The workspace links both rustls crypto backends (iroh's hickory tree pulls
/// `aws-lc-rs`; octocrab's `rustls-ring` pulls `ring`), so rustls cannot
/// auto-select a default and `hyper-rustls` panics on the first TLS handshake
/// under `cargo test --workspace` feature unification. Installing `ring`
/// explicitly (idempotent — `install_default` returns `Err` if one is already
/// set, which we ignore) mirrors the relay crate's pattern and keeps the
/// no-openssl / pure-Rust / Windows-lane posture (`ring`, not `aws-lc-rs`).
fn ensure_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn map_octo_err(e: octocrab::Error) -> Error {
    Error::Vcs(format!("github: {e}"))
}

// --- REST response projections (local; decoupled from octocrab's models) ---

#[derive(Debug, Deserialize)]
struct GhRef {
    #[serde(rename = "ref")]
    ref_name: String,
    sha: String,
}

#[derive(Debug, Deserialize)]
struct GhPull {
    number: i64,
    node_id: Option<String>,
    title: String,
    #[serde(default)]
    body: Option<String>,
    state: String,
    #[serde(default)]
    draft: bool,
    html_url: String,
    head: GhRef,
    base: GhRef,
    #[serde(default)]
    merged: bool,
}

impl GhPull {
    fn into_pull_request(self, repo_full_name: &str) -> PullRequest {
        // GitHub reports `state` as open/closed; a closed+merged PR is "merged",
        // an open draft is "draft" — normalize to the trait's vocabulary.
        let state = if self.merged {
            "merged".to_string()
        } else if self.state == "open" && self.draft {
            "draft".to_string()
        } else {
            self.state.to_lowercase()
        };
        PullRequest {
            id: ProviderPrId {
                repo_full_name: repo_full_name.to_string(),
                number: self.number,
                node_id: self.node_id,
            },
            title: self.title,
            body: self.body.unwrap_or_default(),
            state,
            url: self.html_url,
            base_ref: self.base.ref_name,
            head_ref: self.head.ref_name,
            head_sha: self.head.sha,
        }
    }
}

#[derive(Debug, Deserialize)]
struct GhCheckRun {
    name: String,
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    details_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhCheckRunsResponse {
    #[serde(default)]
    check_runs: Vec<GhCheckRun>,
}

#[derive(Debug, Deserialize)]
struct GhMergeResult {
    #[serde(default)]
    merged: bool,
    #[serde(default)]
    sha: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhDeployment {
    id: u64,
    #[serde(default)]
    environment: String,
    #[serde(default, rename = "ref")]
    ref_: String,
}

#[derive(Debug, Deserialize)]
struct GhLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GhIssue {
    number: i64,
    title: String,
    #[serde(default)]
    body: Option<String>,
    state: String,
    html_url: String,
    #[serde(default)]
    labels: Vec<GhLabel>,
    /// Present on PR objects; lets us reject PR URLs routed to issue fetch.
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
}

/// Parse `owner/repo` + the trailing `issues/<n>` number from a GitHub issue
/// URL. Returns `None` for non-issue paths.
fn parse_github_issue_url(url: &Url) -> Option<(String, i64)> {
    // Path: /owner/repo/issues/<number>  (also /pull/<n> which we reject).
    let segments: Vec<&str> = url.path().trim_matches('/').split('/').collect();
    if segments.len() >= 4 && segments[2] == "issues" {
        let number = segments[3].parse::<i64>().ok()?;
        return Some((format!("{}/{}", segments[0], segments[1]), number));
    }
    None
}

#[async_trait]
impl VcsProvider for GitHubProvider {
    async fn create_pr(&self, req: CreatePrRequest) -> Result<PullRequest> {
        let route = format!("/repos/{}/pulls", req.repo_full_name);
        let body = serde_json::json!({
            "title": req.title,
            "head": req.head,
            "base": req.base,
            "body": req.body,
            "draft": req.draft,
        });
        let pull: GhPull = self
            .request_json(http::Method::POST, &route, Some(body))
            .await?;
        Ok(pull.into_pull_request(&req.repo_full_name))
    }

    async fn get_pr(&self, id: ProviderPrId) -> Result<PullRequest> {
        let route = format!("/repos/{}/pulls/{}", id.repo_full_name, id.number);
        let pull: GhPull = self.request_json(http::Method::GET, &route, None).await?;
        Ok(pull.into_pull_request(&id.repo_full_name))
    }

    async fn list_check_runs(&self, repo: &str, sha: &str) -> Result<Vec<CheckRun>> {
        let route = format!("/repos/{repo}/commits/{sha}/check-runs");
        let resp: GhCheckRunsResponse = self.request_json(http::Method::GET, &route, None).await?;
        Ok(resp
            .check_runs
            .into_iter()
            .map(|r| CheckRun {
                name: r.name,
                status: r.status,
                conclusion: r.conclusion.unwrap_or_default(),
                details_url: r.details_url.unwrap_or_default(),
            })
            .collect())
    }

    async fn merge_pr(&self, id: ProviderPrId, method: MergeMethod) -> Result<MergeReport> {
        let route = format!("/repos/{}/pulls/{}/merge", id.repo_full_name, id.number);
        let body = serde_json::json!({ "merge_method": method.as_str() });
        let result: GhMergeResult = self
            .request_json(http::Method::PUT, &route, Some(body))
            .await?;
        Ok(MergeReport {
            merged: result.merged,
            merge_commit_sha: result.sha,
            message: result.message.unwrap_or_default(),
        })
    }

    async fn revert_pr(&self, _id: ProviderPrId) -> Result<RevertReport> {
        // Signature frozen; the revert-commit-by-default mechanics (R-5) land
        // with the coordinated-revert loop (Task 320).
        Err(unimplemented_err(
            "GitHubProvider::revert_pr (filled by Task 320)",
        ))
    }

    async fn list_review_threads(&self, _id: ProviderPrId) -> Result<Vec<ReviewThread>> {
        // GraphQL — signature frozen; Task 316 supplies the query.
        Err(unimplemented_err(
            "GitHubProvider::list_review_threads (GraphQL; filled by Task 316)",
        ))
    }

    async fn resolve_thread(&self, _id: ThreadId) -> Result<()> {
        Err(unimplemented_err(
            "GitHubProvider::resolve_thread (GraphQL; filled by Task 316)",
        ))
    }

    async fn list_deployments(&self, repo: &str, ref_: &str) -> Result<Vec<Deployment>> {
        let route = format!("/repos/{repo}/deployments?ref={ref_}");
        let deployments: Vec<GhDeployment> =
            self.request_json(http::Method::GET, &route, None).await?;
        // The latest deployment status requires a second call per deployment
        // (Task 316 aggregates them); this task lists deployments with an empty
        // `state` placeholder. Signature + listing frozen now.
        Ok(deployments
            .into_iter()
            .map(|d| Deployment {
                id: d.id.to_string(),
                environment: d.environment,
                state: String::new(),
                ref_: d.ref_,
            })
            .collect())
    }

    async fn fetch_issue(&self, url: &Url) -> Result<Option<Issue>> {
        let (repo, number) = match parse_github_issue_url(url) {
            Some(parsed) => parsed,
            None => return Err(Error::Validation(format!("not a GitHub issue URL: {url}"))),
        };
        let route = format!("/repos/{repo}/issues/{number}");
        let issue: GhIssue = self.request_json(http::Method::GET, &route, None).await?;
        if issue.pull_request.is_some() {
            return Err(Error::Validation(format!(
                "URL {url} is a pull request, not an issue"
            )));
        }
        Ok(Some(Issue {
            number: issue.number,
            title: issue.title,
            body: issue.body.unwrap_or_default(),
            state: issue.state.to_lowercase(),
            url: issue.html_url,
            labels: issue.labels.into_iter().map(|l| l.name).collect(),
            // GitHub issues key on `number`; the provider-native string id is
            // a Linear/Jira concept (Task 317).
            external_id: String::new(),
        }))
    }
}
