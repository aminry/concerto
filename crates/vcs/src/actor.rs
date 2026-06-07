//! Cloneable [`VcsHandle`] + [`VcsConfig`] (Task 45, moved to `crates/vcs` by
//! Task 313).
//!
//! The V0.1 `VcsHandle` lived in `crates/core/src/vcs/actor.rs` and shelled out
//! to `gh`. Task 313 moves the handle here so the new `crates/vcs` crate owns
//! the whole VCS surface; the supervised `VcsProviderActor` (which needs the
//! Core's `supervisor::Actor` trait) stays in `crates/core/src/vcs/` and wraps
//! this handle — so `boot.rs` + the `Vcs` gRPC handler compile unchanged.
//!
//! ## Frozen surface (Task 45 — preserved verbatim)
//!
//! The Task-45 `VcsHandle` method signatures (`create_pr`/`get_pr`/`list_prs`/
//! `merge_pr`/`get_check_runs`/`fetch_issue` keyed by `RepositoryId`) are
//! FROZEN and reused by the existing gRPC handler — extend, never break. The
//! handle keeps shelling out to `gh` (the V0.1 behavior) so the V0.1 `Vcs` gRPC
//! path is byte-for-byte unchanged. The new octocrab `GitHubProvider` +
//! `choose_backend` dispatch + the `fetch_issue(url)` router are the *internal*
//! trait surface this task adds; wiring the handle to dispatch through the
//! trait is a follow-on (the gRPC proto is untouched this task).
//!
//! Task 313 adds [`VcsHandle::fetch_issue_url`] — the top-level URL-host router
//! (`design/13 §6.1`): github.com → the GitHub issue fetch; linear.app /
//! *.atlassian.net → the Task-317 seam (`Unimplemented`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use concerto_error::{Error, Result};
use concerto_persist::{
    NewPullRequest, Persistence, PullRequest, PullRequestId, Repository, RepositoryId, WorkareaId,
};
use url::Url;

use concerto_keychain::SecretValue;

use crate::checks::ChecksAggregator;
use crate::dispatch::{external_tracker_blocked, route_issue_host, IssueCache, IssueHost};
use crate::gh_cli;
use crate::github::GitHubProvider;
use crate::jira::{JiraClient, RefreshToken};
use crate::linear::LinearClient;
use crate::provider::{Issue, VcsProvider};

/// Config for the supervised actor's `run` loop. V0.1 has no knobs.
#[derive(Clone, Debug, Default)]
pub struct VcsConfig;

/// Cheap-cloneable, shareable handle to the VCS provider.
///
/// Cloning is `O(Arc)`; the persistence handle and the resolved `gh` path are
/// shared across clones. The `gh` path is resolved lazily on first use.
#[derive(Clone)]
pub struct VcsHandle {
    persistence: Arc<Persistence>,
    gh_path: Arc<tokio::sync::OnceCell<PathBuf>>,
    /// Shared 1 h-TTL issue cache (`design/13 §3.7`/§4). Issue bodies live ONLY
    /// here — never in SQLite. Cloned with the handle so every clone shares one
    /// cache (Task 317).
    issue_cache: IssueCache,
    /// Shared review-thread / check-run / deployment aggregator + the
    /// `checks.<wa>.<repo>` event broadcaster (Task 316). Cloned with the handle
    /// so the gRPC handler + the streams handler (which reads
    /// [`VcsHandle::checks_sender`]) share one aggregator + one broadcast.
    /// In-memory only — never persisted (`design/13 §3.6`/R-3).
    checks: ChecksAggregator,
    /// The per-repo webhook-secret seam (`VcsSecretSlot::WebhookSecret`, D4) the
    /// `ingest_webhook` HMAC verify reads through (Task 315). `None` until the
    /// Core wires the keychain source at boot (or a test injects a fake); a
    /// `None` source ⇒ every webhook is dropped (no secret configured). Held as
    /// an `Option<Arc<dyn …>>` so the handle stays cheap-clone + keychain-free
    /// in the leaf crate.
    webhook_secret_source: Option<Arc<dyn crate::webhook::WebhookSecretSource>>,
    /// The per-repo provider seam (`design/13 §6.3`) the targeted-invalidation
    /// path uses to eagerly re-fetch + emit fresh state on a verified webhook
    /// (Task 315/316). `None` ⇒ the cache rows are still dropped (next read
    /// refreshes); the webhook stays a strict accelerator.
    webhook_provider_source: Option<Arc<dyn crate::webhook::WebhookProviderSource>>,
}

impl VcsHandle {
    /// Build a fresh handle. The `gh` path is NOT resolved here.
    pub fn new(persistence: Arc<Persistence>) -> Self {
        Self {
            persistence,
            gh_path: Arc::new(tokio::sync::OnceCell::new()),
            issue_cache: IssueCache::system(),
            checks: ChecksAggregator::new(),
            webhook_secret_source: None,
            webhook_provider_source: None,
        }
    }

    /// Build a handle with a caller-supplied issue cache (the `testkit`
    /// synthetic-clock cache in tests). Production uses [`VcsHandle::new`].
    pub fn with_issue_cache(persistence: Arc<Persistence>, issue_cache: IssueCache) -> Self {
        Self {
            persistence,
            gh_path: Arc::new(tokio::sync::OnceCell::new()),
            issue_cache,
            checks: ChecksAggregator::new(),
            webhook_secret_source: None,
            webhook_provider_source: None,
        }
    }

    /// Install the inbound-webhook seams (Task 315): the per-repo HMAC-secret
    /// source (`VcsSecretSlot::WebhookSecret`, D4) the verify reads through, and
    /// the provider source the targeted-invalidation re-fetch uses (`design/13
    /// §6.3`). The Core wires these at boot; the Tier-2 tests inject fakes.
    /// Returns the handle for chaining (the wiring site holds the result).
    pub fn with_webhook_sources(
        mut self,
        secret_source: Arc<dyn crate::webhook::WebhookSecretSource>,
        provider_source: Arc<dyn crate::webhook::WebhookProviderSource>,
    ) -> Self {
        self.webhook_secret_source = Some(secret_source);
        self.webhook_provider_source = Some(provider_source);
        self
    }

    /// The shared review-thread / check-run / deployment aggregator (Task 316).
    pub fn checks(&self) -> &ChecksAggregator {
        &self.checks
    }

    /// The `checks.<wa>.<repo>` event broadcast sender, for the Core to wire
    /// into the `StreamsHandler` (`with_vcs_events`).
    pub fn checks_sender(&self) -> tokio::sync::broadcast::Sender<crate::checks::VcsEvent> {
        self.checks.sender()
    }

    /// Borrow the shared read-only pool (the gRPC handler's repo→workarea lookup).
    pub fn persistence_readers(&self) -> &sqlx::SqlitePool {
        self.persistence.readers()
    }

    /// Exclusive write access to the SQLite writer (the `SetVcsCredential`
    /// handler upserts the non-secret `vcs_credentials` metadata row, Task 317).
    pub async fn persistence_writer(&self) -> concerto_persist::WriterGuard<'_> {
        self.persistence.writer().await
    }

    /// Resolve (and cache) the `gh` binary path.
    pub async fn gh(&self) -> Result<&std::path::Path> {
        let path = self
            .gh_path
            .get_or_try_init(|| async { gh_cli::resolve_gh_path() })
            .await?;
        Ok(path.as_path())
    }

    /// Probe authentication once. Returns [`Error::VcsNotAuthenticated`] on
    /// failure.
    pub async fn check_auth(&self) -> Result<()> {
        let gh = self.gh().await?;
        gh_cli::check_auth(gh).await
    }

    /// List PRs in `repository_id`'s upstream repo via `gh pr list`.
    pub async fn list_prs(&self, repository_id: &RepositoryId) -> Result<Vec<gh_cli::PrSummary>> {
        let gh = self.gh().await?;
        let repo = self.resolve_repo_full_name(repository_id).await?;
        gh_cli::list_prs(gh, &repo).await
    }

    /// Look up one PR by number, upsert the cache row, return the projection.
    pub async fn get_pr(
        &self,
        workarea_id: &WorkareaId,
        repository_id: &RepositoryId,
        pr_number: i64,
    ) -> Result<PullRequest> {
        let gh = self.gh().await?;
        let repo = self.resolve_repo_full_name(repository_id).await?;
        let detail = gh_cli::view_pr(gh, &repo, pr_number).await?;
        self.upsert_from_detail(workarea_id, repository_id, &detail)
            .await
    }

    /// Create a PR for `head_ref` against `base_ref` and persist the cache row.
    pub async fn create_pr(
        &self,
        workarea_id: &WorkareaId,
        repository_id: &RepositoryId,
        base_ref: &str,
        head_ref: &str,
        title: &str,
        body: &str,
    ) -> Result<PullRequest> {
        let gh = self.gh().await?;
        let repository = self.load_repository(repository_id).await?;
        let repo_name = repo_full_name_from_url(&repository.url).ok_or_else(|| {
            Error::Validation(format!(
                "repository {repository_id} url `{}` is not a parseable github.com URL",
                repository.url
            ))
        })?;
        let base = if base_ref.is_empty() {
            repository.default_branch.as_str()
        } else {
            base_ref
        };
        let number = gh_cli::create_pr(gh, &repo_name, head_ref, base, title, body).await?;
        let detail = gh_cli::view_pr(gh, &repo_name, number).await?;
        self.upsert_from_detail(workarea_id, repository_id, &detail)
            .await
    }

    /// Merge an existing PR. `method` ∈ `merge|squash|rebase`.
    pub async fn merge_pr(
        &self,
        repository_id: &RepositoryId,
        pr_number: i64,
        method: &str,
    ) -> Result<()> {
        let gh = self.gh().await?;
        let repo = self.resolve_repo_full_name(repository_id).await?;
        gh_cli::merge_pr(gh, &repo, pr_number, method).await
    }

    /// List check runs for a commit SHA on the given repo.
    pub async fn get_check_runs(
        &self,
        repository_id: &RepositoryId,
        sha: &str,
    ) -> Result<Vec<gh_cli::CheckRun>> {
        let gh = self.gh().await?;
        let repo = self.resolve_repo_full_name(repository_id).await?;
        gh_cli::get_check_runs(gh, &repo, sha).await
    }

    /// Fetch a GitHub issue via `gh issue view` (Task-45 frozen signature,
    /// keyed by `RepositoryId` + number).
    pub async fn fetch_issue(
        &self,
        repository_id: &RepositoryId,
        issue_number: i64,
    ) -> Result<gh_cli::IssueDetail> {
        let gh = self.gh().await?;
        let repo = self.resolve_repo_full_name(repository_id).await?;
        gh_cli::view_issue(gh, &repo, issue_number).await
    }

    /// Top-level `fetch_issue(url)` **router** (`design/13 §6.1` + §3.7). Parses
    /// the URL host and dispatches: `github.com`/Enterprise → the GitHub issue
    /// fetch (octocrab, Task 313); `linear.app` → the Linear GraphQL client;
    /// `*.atlassian.net` → the Jira REST client (Task 317). The result is cached
    /// for 1 h in memory (keyed by canonicalized URL); a still-fresh hit skips
    /// the HTTP call entirely. Issue bodies are NEVER persisted to SQLite.
    ///
    /// `creds` carries the per-arm credentials (read from the keychain by the
    /// caller) + the `enterprise_data_privacy` flag: a privacy-locked project
    /// refuses an external-tracker (Linear/Jira) fetch with the typed
    /// [`external_tracker_blocked`] error BEFORE any outbound call (the GitHub
    /// arm is the user's own repo host, not an external tracker, so it is not
    /// gated here).
    pub async fn fetch_issue_url(
        &self,
        url: &str,
        creds: &IssueFetchCreds<'_>,
    ) -> Result<Option<Issue>> {
        // 1 h-TTL cache hit → no HTTP call (privacy + latency).
        if let Some(cached) = self.issue_cache.get(url) {
            return Ok(Some(cached));
        }

        let parsed = Url::parse(url)
            .map_err(|e| Error::Validation(format!("invalid issue URL `{url}`: {e}")))?;
        let issue = match route_issue_host(&parsed)? {
            IssueHost::GitHub => {
                let token = creds.github_token.ok_or_else(|| {
                    Error::VcsNotAuthenticated(
                        "GitHub issue fetch needs a token (SecretKind::GithubPat)".to_string(),
                    )
                })?;
                let provider = GitHubProvider::with_token(token)?;
                provider.fetch_issue(&parsed).await?
            }
            IssueHost::Linear => {
                if creds.enterprise_data_privacy {
                    return Err(external_tracker_blocked("linear"));
                }
                let token = creds.linear_token.ok_or_else(|| {
                    Error::VcsNotAuthenticated(
                        "Linear issue fetch needs a token (VcsSecretSlot::LinearAccessToken); \
                         connect Linear in Settings"
                            .to_string(),
                    )
                })?;
                let client = match creds.linear_base {
                    Some(base) => LinearClient::with_base(base)?,
                    None => LinearClient::new()?,
                };
                Some(client.fetch(url, token).await?)
            }
            IssueHost::Jira => {
                if creds.enterprise_data_privacy {
                    return Err(external_tracker_blocked("jira"));
                }
                let token = creds.jira_token.ok_or_else(|| {
                    Error::VcsNotAuthenticated(
                        "Jira issue fetch needs a token (VcsSecretSlot::JiraAccessToken); \
                         connect Jira in Settings"
                            .to_string(),
                    )
                })?;
                // Jira's REST base is the Atlassian site host of the URL itself
                // (`https://<site>.atlassian.net`), unless the caller overrides
                // it (the testkit wiremock base).
                let base = match creds.jira_base {
                    Some(base) => base.to_string(),
                    None => jira_base_from_url(&parsed)?,
                };
                let client = JiraClient::with_base(&base)?;
                Some(client.fetch(url, token, creds.jira_refresh).await?)
            }
        };

        // Cache successful fetches (a `None`/absent issue is not cached).
        if let Some(ref issue) = issue {
            self.issue_cache.put(url, issue.clone());
        }
        Ok(issue)
    }

    /// Direct Linear issue fetch by id (`ENG-123`) or URL (`design/13 §5.1`
    /// `fetch_linear_issue`). Skips the host-routing step; still consults the 1 h
    /// cache + the `enterprise_data_privacy` gate. `linear_base` overrides the
    /// production endpoint (the testkit wiremock base) when `Some`.
    pub async fn fetch_linear_issue(
        &self,
        id_or_url: &str,
        token: &SecretValue,
        enterprise_data_privacy: bool,
        linear_base: Option<&str>,
    ) -> Result<Issue> {
        if enterprise_data_privacy {
            return Err(external_tracker_blocked("linear"));
        }
        if let Some(cached) = self.issue_cache.get(id_or_url) {
            return Ok(cached);
        }
        let client = match linear_base {
            Some(base) => LinearClient::with_base(base)?,
            None => LinearClient::new()?,
        };
        let issue = client.fetch(id_or_url, token).await?;
        self.issue_cache.put(id_or_url, issue.clone());
        Ok(issue)
    }

    /// Direct Jira issue fetch by key (`PROJ-45`) or URL (`design/13 §5.1`). The
    /// caller supplies the Atlassian site base + token + optional one-shot
    /// refresh. Consults the 1 h cache + the privacy gate.
    pub async fn fetch_jira_issue(
        &self,
        key_or_url: &str,
        base_uri: &str,
        token: &SecretValue,
        refresh: Option<&RefreshToken<'_>>,
        enterprise_data_privacy: bool,
    ) -> Result<Issue> {
        if enterprise_data_privacy {
            return Err(external_tracker_blocked("jira"));
        }
        if let Some(cached) = self.issue_cache.get(key_or_url) {
            return Ok(cached);
        }
        let client = JiraClient::with_base(base_uri)?;
        let issue = client.fetch(key_or_url, token, refresh).await?;
        self.issue_cache.put(key_or_url, issue.clone());
        Ok(issue)
    }

    // ---- Task 316: review-thread / check-run / deployment aggregation ----

    /// Build the octocrab [`GitHubProvider`] for `repo_full_name` from the
    /// keychain PAT (`SecretKind::GithubPat`) + return it as a trait object the
    /// [`ChecksAggregator`] drives. Every call goes through the provider's
    /// header-capturing `request_json` path, so it bills Task 314's rate-limit
    /// pool. (The full per-call `choose_backend` App-vs-PAT-vs-gh selection is
    /// the dispatcher's; the GraphQL review-thread + REST check/deploy paths are
    /// octocrab-only — `gh` has no thread-resolve — so this uses the PAT
    /// octocrab provider directly.)
    pub async fn github_provider(&self, token: &str) -> Result<Arc<dyn VcsProvider>> {
        Ok(Arc::new(GitHubProvider::with_token(token)?))
    }

    /// Resolve a repository row's `owner/repo` full name (public so the gRPC
    /// handler can scope GraphQL/REST calls).
    pub async fn repo_full_name(&self, id: &RepositoryId) -> Result<String> {
        self.resolve_repo_full_name(id).await
    }

    /// Look up the cached PR row for `(repository_id, pr_number)` — the handler
    /// needs its GraphQL node id (`external_id`, Task 319 when present) +
    /// `head_sha`. Returns `None` when no row is cached yet.
    pub async fn cached_pr_row(
        &self,
        repository_id: &RepositoryId,
        pr_number: i64,
    ) -> Result<Option<PullRequest>> {
        let pool = self.persistence.readers();
        let id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM pull_requests WHERE repository_id = ? AND pr_number = ? LIMIT 1",
        )
        .bind(&repository_id.0)
        .bind(pr_number)
        .fetch_optional(pool)
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
        match id {
            Some(id) => concerto_persist::pull_requests::get(pool, &PullRequestId(id)).await,
            None => Ok(None),
        }
    }

    // ---- Task 315: inbound-webhook ingest ----

    /// Ingest an inbound GitHub webhook for `repo` (`design/13 §5.1`, the FROZEN
    /// method). The pipeline order is FROZEN (`design/13 §6.2`):
    ///
    /// 1. **Idempotency first** — dedupe on `payload.delivery_id` via the
    ///    restart-surviving `webhook_deliveries` table (migration 0013). A replay
    ///    is dropped and acked `200` (so GitHub stops retrying); it never touches
    ///    the secret or the parser.
    /// 2. **HMAC verify** — recompute HMAC-SHA256 over the raw body with the
    ///    per-repo `VcsSecretSlot::WebhookSecret` and constant-time-compare
    ///    against `payload.signature_256`. A mismatch / missing-secret /
    ///    missing-signature is dropped + logged with NO sender-visible reason
    ///    (`design/13 §8`) → [`IngestOutcome::Reject`] (`4xx`).
    /// 3. **Parse** by `event_type`; an unknown/unparseable event is a no-op
    ///    `200` (forward-compat).
    /// 4. **Targeted cache invalidation** (`design/13 §6.3`) — drop just the
    ///    affected PR / check / deployment cache rows; best-effort eager
    ///    re-fetch + emit via the provider seam. A failure here NEVER breaks the
    ///    poll path: an authentic-but-uninvalidatable webhook still acks `200`.
    ///
    /// Returns the [`IngestOutcome`] the Core maps to the transport ack byte.
    /// Never errors out of band — every failure mode maps to a defined outcome
    /// (the FROZEN signature returns `Result`, but the body is total).
    pub async fn ingest_webhook(
        &self,
        repo: &RepositoryId,
        payload: crate::webhook::WebhookPayload,
    ) -> Result<crate::webhook::IngestOutcome> {
        use crate::webhook::{parse_event, verify_signature, IngestOutcome};

        // 1. Idempotency first — cheapest, drops replays without the secret.
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let newly_inserted = {
            let mut writer = self.persistence.writer().await;
            match concerto_persist::webhook_deliveries::insert_delivery_if_absent(
                &mut writer,
                &payload.delivery_id,
                &repo.0,
                now_ms,
            )
            .await
            {
                Ok(b) => b,
                Err(e) => {
                    // The idempotency DB write failed — a Core-internal error
                    // after an otherwise-valid frame. Ack 5xx; GitHub redelivers.
                    tracing::warn!(error = %e, repo = %repo, "webhook: idempotency insert failed");
                    return Ok(IngestOutcome::Error);
                }
            }
        };
        if !newly_inserted {
            // Replay (same delivery_id) — drop, ack 200 so GitHub stops retrying.
            tracing::debug!(
                delivery_id = %payload.delivery_id,
                repo = %repo,
                "webhook: replay (delivery-id already seen); dropping, ack 200"
            );
            return Ok(IngestOutcome::Accepted);
        }

        // 2. HMAC verify — constant-time, keyed by the per-repo webhook secret.
        let secret = match &self.webhook_secret_source {
            Some(src) => match src.webhook_secret(&repo.0).await {
                Ok(Some(s)) => s,
                Ok(None) => {
                    // No secret configured for this repo — the webhook is not
                    // set up. Drop + log; NO sender-visible reason (`design/13 §8`).
                    tracing::warn!(repo = %repo, "webhook: no secret configured; dropping (4xx)");
                    return Ok(IngestOutcome::Reject);
                }
                Err(e) => {
                    tracing::warn!(error = %e, repo = %repo, "webhook: secret load failed; dropping (5xx)");
                    return Ok(IngestOutcome::Error);
                }
            },
            None => {
                tracing::warn!(repo = %repo, "webhook: no secret source wired; dropping (4xx)");
                return Ok(IngestOutcome::Reject);
            }
        };
        if !verify_signature(&secret, &payload.body, &payload.signature_256) {
            // Mismatch / missing signature — drop + log, NO sender-visible reason.
            tracing::warn!(
                repo = %repo,
                event = %payload.event_type,
                "webhook: HMAC verification failed; dropping (4xx)"
            );
            return Ok(IngestOutcome::Reject);
        }

        // 3. Parse the event (a malformed/unknown body is a no-op 200).
        let parsed = parse_event(&payload.event_type, &payload.body);
        tracing::debug!(
            repo = %repo,
            event = %payload.event_type,
            delivery_id = %payload.delivery_id,
            "webhook: verified; ingesting"
        );

        // 4. Targeted cache invalidation (`design/13 §6.3`). Best-effort — any
        //    error here is logged and swallowed: the webhook is authentic + the
        //    delivery is recorded, so the accelerator simply no-op'd. The poll
        //    path is untouched.
        if let Err(e) = self.invalidate_for_event(repo, &parsed).await {
            tracing::warn!(error = %e, repo = %repo, "webhook: targeted invalidation failed (poll path unaffected)");
        }

        Ok(IngestOutcome::Accepted)
    }

    /// Apply the §6.3 targeted invalidation for a parsed event: locate the
    /// affected cache rows (by `(repo, sha)` / PR number / `(repo, ref)`) from the
    /// `pull_requests` cache, build a provider via the seam, and drive the
    /// [`ChecksAggregator`] invalidate path (drop + re-fetch + emit). An
    /// [`ParsedEvent::Unhandled`] event is a no-op.
    async fn invalidate_for_event(
        &self,
        repo: &RepositoryId,
        parsed: &crate::webhook::ParsedEvent,
    ) -> Result<()> {
        use crate::webhook::ParsedEvent;

        // No provider seam wired → drop is a no-op (next poll refreshes). The
        // delivery is recorded; the accelerator is just inactive.
        let Some(provider_src) = &self.webhook_provider_source else {
            return Ok(());
        };

        let repo_full = self.resolve_repo_full_name(repo).await?;
        let provider = match provider_src.provider_for(&repo_full).await? {
            Some(p) => p,
            None => return Ok(()), // no credential wired → drop is a no-op.
        };

        match parsed {
            ParsedEvent::CheckRun { sha } => {
                // Invalidate the check cache for every workarea that has a cached
                // PR at this head SHA in this repo (each workarea has its own
                // `checks.<wa>.<repo>` subject).
                for (workarea_id, _pr_number) in self.workareas_for_sha(repo, sha).await? {
                    self.checks
                        .invalidate_check_runs(&provider, &workarea_id, &repo.0, &repo_full, sha)
                        .await?;
                }
            }
            ParsedEvent::PullRequest { number } => {
                for workarea_id in self.workareas_for_pr(repo, *number).await? {
                    let pr = crate::provider::ProviderPrId::new(repo_full.clone(), *number);
                    self.checks
                        .invalidate_threads(&provider, &workarea_id, &repo.0, pr)
                        .await?;
                }
            }
            ParsedEvent::Deployment { ref_ } => {
                for (workarea_id, _pr_number) in self.workareas_for_ref(repo, ref_).await? {
                    self.checks
                        .invalidate_deployments(&provider, &workarea_id, &repo.0, &repo_full, ref_)
                        .await?;
                }
            }
            ParsedEvent::Unhandled => {}
        }
        Ok(())
    }

    /// Distinct `(workarea_id, pr_number)` for cached PRs in `repo` at `head_sha`.
    async fn workareas_for_sha(
        &self,
        repo: &RepositoryId,
        sha: &str,
    ) -> Result<Vec<(String, i64)>> {
        let rows = sqlx::query_as::<_, (String, i64)>(
            "SELECT DISTINCT workarea_id, pr_number FROM pull_requests
             WHERE repository_id = ? AND head_sha = ?",
        )
        .bind(&repo.0)
        .bind(sha)
        .fetch_all(self.persistence.readers())
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
        Ok(rows)
    }

    /// Distinct `workarea_id` for cached PRs in `repo` with `pr_number`.
    async fn workareas_for_pr(&self, repo: &RepositoryId, pr_number: i64) -> Result<Vec<String>> {
        let rows = sqlx::query_as::<_, (String,)>(
            "SELECT DISTINCT workarea_id FROM pull_requests
             WHERE repository_id = ? AND pr_number = ?",
        )
        .bind(&repo.0)
        .bind(pr_number)
        .fetch_all(self.persistence.readers())
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
        Ok(rows.into_iter().map(|(w,)| w).collect())
    }

    /// Distinct `(workarea_id, pr_number)` for cached PRs in `repo` whose head
    /// branch matches `ref_` (a deployment ref is typically a branch name).
    async fn workareas_for_ref(
        &self,
        repo: &RepositoryId,
        ref_: &str,
    ) -> Result<Vec<(String, i64)>> {
        let rows = sqlx::query_as::<_, (String, i64)>(
            "SELECT DISTINCT workarea_id, pr_number FROM pull_requests
             WHERE repository_id = ? AND head_ref = ?",
        )
        .bind(&repo.0)
        .bind(ref_)
        .fetch_all(self.persistence.readers())
        .await
        .map_err(|e| Error::Sqlx(Box::new(e)))?;
        Ok(rows)
    }

    // ---- internal helpers ----

    async fn load_repository(&self, id: &RepositoryId) -> Result<Repository> {
        concerto_persist::repositories::get(self.persistence.readers(), id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("repository {id} not found")))
    }

    async fn resolve_repo_full_name(&self, id: &RepositoryId) -> Result<String> {
        let row = self.load_repository(id).await?;
        repo_full_name_from_url(&row.url).ok_or_else(|| {
            Error::Validation(format!(
                "repository {id} url `{}` is not a parseable github.com URL",
                row.url
            ))
        })
    }

    /// Resolve the internal [`RepositoryId`] for a GitHub `owner/repo` full name
    /// (Task 315): scan the `repositories` rows, parse each row's clone URL with
    /// [`repo_full_name_from_url`], and return the first id whose full name
    /// matches (case-insensitively — GitHub treats owner/repo as
    /// case-insensitive). Returns `None` when no tracked repository matches (the
    /// webhook targets a repo this Core does not manage). Used by the Core's
    /// `WebhookSink` to map an inbound webhook body's `repository.full_name` onto
    /// the per-repo HMAC secret + cache rows.
    pub async fn resolve_repo_id_by_full_name(
        &self,
        full_name: &str,
    ) -> Result<Option<RepositoryId>> {
        let rows = sqlx::query_as::<_, (String, String)>("SELECT id, url FROM repositories")
            .fetch_all(self.persistence.readers())
            .await
            .map_err(|e| Error::Sqlx(Box::new(e)))?;
        let target = full_name.to_ascii_lowercase();
        for (id, url) in rows {
            if let Some(name) = repo_full_name_from_url(&url) {
                if name.to_ascii_lowercase() == target {
                    return Ok(Some(RepositoryId(id)));
                }
            }
        }
        Ok(None)
    }

    async fn upsert_from_detail(
        &self,
        workarea_id: &WorkareaId,
        repository_id: &RepositoryId,
        detail: &gh_cli::PrDetail,
    ) -> Result<PullRequest> {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let new_id = PullRequestId(uuid::Uuid::now_v7().to_string());
        // `owner/repo` for the GraphQL endpoint (Task 316). The `gh` CLI
        // detail does not carry the GraphQL node id, so `external_id` stays
        // `''` on this path — octocrab create populates it (Task 313/316).
        let repository_full_name = self
            .resolve_repo_full_name(repository_id)
            .await
            .unwrap_or_default();
        let id = {
            // Take the writer lock ONCE so the insertion-order `MAX` and the
            // upsert are atomic (Task 319 D7). `merge_order` only takes effect
            // on the first insert; a re-sync preserves the user's reorder.
            let mut writer = self.persistence.writer().await;
            let merge_order =
                concerto_persist::pull_requests::next_merge_order(&mut writer, workarea_id).await?;
            let row = NewPullRequest {
                id: new_id,
                workarea_id: workarea_id.clone(),
                repository_id: repository_id.clone(),
                provider: "github".to_string(),
                pr_number: detail.number,
                base_ref: detail.base_ref_name.clone(),
                head_ref: detail.head_ref_name.clone(),
                state: detail.state.to_lowercase(),
                title: detail.title.clone(),
                body: detail.body.clone(),
                url: detail.url.clone(),
                head_sha: detail.head_ref_oid.clone(),
                merge_order,
                external_id: String::new(),
                repository_full_name,
                created_at: now_ms,
                updated_at: now_ms,
            };
            concerto_persist::pull_requests::upsert(&mut writer, row).await?
        };
        concerto_persist::pull_requests::get(self.persistence.readers(), &id)
            .await?
            .ok_or_else(|| Error::Internal(format!("pull_request {id} missing after upsert")))
    }
}

/// Per-arm credentials + privacy flag for [`VcsHandle::fetch_issue_url`]
/// (Task 317). The caller (the gRPC handler) reads each token from the keychain
/// (GitHub PAT via `SecretKind::GithubPat`; Linear/Jira via the
/// `VcsSecretSlot::{Linear,Jira}AccessToken` slots) and resolves the project's
/// `enterprise_data_privacy` setting before constructing this. All fields are
/// borrowed so no secret is copied into the carrier.
///
/// Only the arm matching the URL host is consulted, so an unused arm may be
/// `None`. `linear_base`/`jira_base` override the production API base (the
/// `testkit` wiremock base in tests; `None` → the real endpoint / the URL's
/// Atlassian site for Jira).
#[derive(Default)]
pub struct IssueFetchCreds<'a> {
    /// GitHub PAT (the GitHub arm; not gated by `enterprise_data_privacy` —
    /// it is the user's own repo host, not an external tracker).
    pub github_token: Option<&'a str>,
    /// Linear OAuth access token or personal API key.
    pub linear_token: Option<&'a SecretValue>,
    /// Jira (Atlassian) OAuth access token.
    pub jira_token: Option<&'a SecretValue>,
    /// One-shot Jira OAuth refresh callback (invoked once on a 401).
    pub jira_refresh: Option<&'a RefreshToken<'a>>,
    /// Override the Linear API base (testkit). `None` → production.
    pub linear_base: Option<&'a str>,
    /// Override the Jira API base (testkit). `None` → the URL's Atlassian site.
    pub jira_base: Option<&'a str>,
    /// When `true`, refuse Linear/Jira (external-tracker) fetches with the typed
    /// `vcs.external_tracker_blocked` error (the `design/13 §3.7` privacy floor).
    pub enterprise_data_privacy: bool,
}

/// Derive the Jira REST base (`https://<site>.atlassian.net`) from a Jira issue
/// URL, dropping the path. Used when the caller does not override the base.
fn jira_base_from_url(url: &Url) -> Result<String> {
    let host = url
        .host_str()
        .ok_or_else(|| Error::Validation(format!("jira: URL has no host: {url}")))?;
    let scheme = url.scheme();
    Ok(format!("{scheme}://{host}"))
}

/// Extract `owner/repo` from a GitHub URL of any common shape
/// (`https://github.com/owner/repo.git`, `git@github.com:owner/repo`,
/// `https://github.com/owner/repo`). Moved verbatim from the V0.1 actor.
pub fn repo_full_name_from_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let stripped = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    // SSH form: `git@github.com:owner/repo`
    if let Some(after_colon) = stripped.strip_prefix("git@github.com:") {
        if after_colon.matches('/').count() == 1 {
            return Some(after_colon.to_string());
        }
    }
    // HTTPS form: `https://github.com/owner/repo` (also handles http://)
    for prefix in ["https://github.com/", "http://github.com/"] {
        if let Some(rest) = stripped.strip_prefix(prefix) {
            let head: String = rest
                .split(['/', '#', '?'])
                .take(2)
                .collect::<Vec<_>>()
                .join("/");
            if head.matches('/').count() == 1 && !head.is_empty() {
                return Some(head);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_https_url() {
        assert_eq!(
            repo_full_name_from_url("https://github.com/owner/repo.git"),
            Some("owner/repo".to_string())
        );
        assert_eq!(
            repo_full_name_from_url("https://github.com/owner/repo"),
            Some("owner/repo".to_string())
        );
    }

    #[test]
    fn parses_ssh_url() {
        assert_eq!(
            repo_full_name_from_url("git@github.com:owner/repo.git"),
            Some("owner/repo".to_string())
        );
    }

    #[test]
    fn rejects_non_github_url() {
        assert_eq!(
            repo_full_name_from_url("https://gitlab.com/owner/repo"),
            None
        );
    }
}
