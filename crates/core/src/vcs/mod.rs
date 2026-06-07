//! VCS Provider Integration (Task 45, `design/13`; the crate moved to
//! `crates/vcs` by Task 313).
//!
//! Task 313 extracted the whole VCS surface into the dedicated `concerto-vcs`
//! crate (the `VcsProvider` trait + octocrab `GitHubProvider` + the `gh`-CLI
//! fallback + the per-call `choose_backend` dispatch + the `fetch_issue` URL
//! router + the wiremock `testkit` harness). This module is now a thin
//! **wiring shim**: it re-exports the handle types the Core's `boot.rs` and the
//! `Vcs` gRPC handler already use ([`VcsConfig`], [`VcsHandle`], [`gh_cli`]) so
//! those call sites compile unchanged, and it keeps the supervised
//! [`VcsProviderActor`] here — the actor needs the Core's
//! [`crate::supervisor::Actor`] trait, which `concerto-vcs` (a leaf crate that
//! must not depend on the Core) cannot implement.
//!
//! The V0.1 `Vcs` gRPC service behavior is unchanged: [`VcsHandle`] still shells
//! out to `gh` for its Task-45 method set; the new octocrab/trait/dispatch
//! machinery is the *internal* surface the later VCS tasks build on.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use concerto_error::Result;
use concerto_keychain::{SecretKind, Secrets, VcsSecretSlot};
use concerto_persist::Persistence;
use concerto_transport::{WebhookAck, WebhookEnvelope, WebhookSink};
use concerto_vcs::provider::VcsProvider;
use concerto_vcs::webhook::{
    IngestOutcome, WebhookPayload, WebhookProviderSource, WebhookSecretSource,
};

// Re-export the moved surface so `crate::vcs::{gh_cli, VcsConfig, VcsHandle}`
// keeps resolving for `boot.rs` + `handlers/vcs.rs`.
pub use concerto_vcs::{gh_cli, VcsConfig, VcsHandle};

use crate::supervisor::{Actor, ActorContext};

/// Supervised actor that owns the [`VcsHandle`] (Task 45). `run` parks on
/// shutdown; the supervisor's factory clones the handle on each restart so the
/// cached `gh` path survives a wrapper panic.
///
/// Stays in `concerto-core` (not `concerto-vcs`) because it implements the
/// Core's [`Actor`] trait — the `concerto-vcs` leaf crate must not depend on the
/// Core. It is a thin wrapper around the moved [`VcsHandle`].
pub struct VcsProviderActor {
    handle: VcsHandle,
}

impl VcsProviderActor {
    /// Build a fresh actor with a new handle.
    pub fn new(persistence: Arc<Persistence>) -> Self {
        Self {
            handle: VcsHandle::new(persistence),
        }
    }

    /// Cheap clone of the shared handle.
    pub fn handle(&self) -> VcsHandle {
        self.handle.clone()
    }
}

#[async_trait]
impl Actor for VcsProviderActor {
    const NAME: &'static str = "vcs-provider";
    type Config = VcsConfig;

    async fn run(self, ctx: ActorContext<Self::Config>) -> Result<()> {
        tracing::info!("VCS provider ready (gh CLI backend)");
        ctx.shutdown.cancelled().await;
        tracing::debug!("VCS provider actor shutting down");
        Ok(())
    }
}

// ===========================================================================
// Task 315 — inbound-webhook Core wiring
// ===========================================================================
//
// The transport demuxes a `0x04` Webhook stream and invokes a [`WebhookSink`]
// the Core supplies at `serve_iroh`. The Core's sink ([`CoreWebhookSink`])
// resolves the targeted [`concerto_persist::RepositoryId`] from the webhook body's
// `repository.full_name`, builds the proto/transport-free `WebhookPayload`, and
// drives `VcsHandle::ingest_webhook` (idempotency → constant-time HMAC →
// parse → targeted-invalidate). The handle reads the per-repo HMAC secret +
// the re-fetch provider through two keychain-backed seams wired here, so
// `crates/vcs` stays a leaf crate that never depends on `concerto-keychain`.

/// Keychain-backed [`WebhookSecretSource`] (`VcsSecretSlot::WebhookSecret`, D4):
/// reads the per-repo HMAC secret keyed by `scope_id = repo_id`. The secret
/// material lives ONLY in the keychain; this seam exposes the raw bytes to the
/// HMAC verify and nowhere else.
struct KeychainWebhookSecretSource {
    secrets: Secrets,
}

#[async_trait]
impl WebhookSecretSource for KeychainWebhookSecretSource {
    async fn webhook_secret(&self, repo_id: &str) -> Result<Option<Vec<u8>>> {
        match self
            .secrets
            .get_vcs_secret(repo_id, VcsSecretSlot::WebhookSecret)
            .await
        {
            // GitHub webhook secrets are arbitrary UTF-8 strings; HMAC is keyed
            // by the secret's raw bytes.
            Ok(Some(v)) => Ok(Some(v.expose().as_bytes().to_vec())),
            Ok(None) => Ok(None),
            Err(e) => Err(concerto_error::Error::Internal(format!(
                "loading webhook secret for repo {repo_id}: {e}"
            ))),
        }
    }
}

/// Keychain-backed [`WebhookProviderSource`] (`design/13 §6.3`): builds an
/// octocrab `GitHubProvider` from the GitHub PAT (`SecretKind::GithubPat`) so the
/// targeted-invalidation path can eagerly re-fetch + emit. Returns `None` when no
/// PAT is configured — the cache rows are still dropped, so the next poll/read
/// refreshes (the webhook stays a strict accelerator; the poll path never depends
/// on it).
struct KeychainWebhookProviderSource {
    secrets: Secrets,
    handle: VcsHandle,
}

#[async_trait]
impl WebhookProviderSource for KeychainWebhookProviderSource {
    async fn provider_for(&self, _repo_full_name: &str) -> Result<Option<Arc<dyn VcsProvider>>> {
        let token = match self.secrets.get(SecretKind::GithubPat).await {
            Ok(Some(v)) => v,
            Ok(None) => return Ok(None), // no PAT → drop is a no-op.
            Err(e) => {
                return Err(concerto_error::Error::Internal(format!(
                    "loading GitHub PAT for webhook re-fetch: {e}"
                )))
            }
        };
        Ok(Some(self.handle.github_provider(token.expose()).await?))
    }
}

/// Keychain-backed [`concerto_vcs::WriteBackTokens`] (Task 320.5): resolves the
/// Linear/Jira access token for the post-coordinated-merge issue write-back,
/// keyed by the most-recently-connected `vcs_credentials` account for the
/// provider (the same single-account resolution the `FetchIssueByUrl` handler
/// uses). Tokens live ONLY in the keychain; this seam exposes one to the
/// write-back call and nowhere else. `Ok(None)` ⇒ no credential connected for
/// the provider (the write-back records a `skipped`/`failed` outcome — it never
/// fails the merge).
struct KeychainWriteBackTokens {
    secrets: Secrets,
    persistence: Arc<Persistence>,
}

#[async_trait]
impl concerto_vcs::WriteBackTokens for KeychainWriteBackTokens {
    async fn token(
        &self,
        provider: concerto_vcs::IssueProvider,
    ) -> Result<Option<concerto_keychain::SecretValue>> {
        let (provider_str, slot) = match provider {
            concerto_vcs::IssueProvider::Linear => ("linear", VcsSecretSlot::LinearAccessToken),
            concerto_vcs::IssueProvider::Jira => ("jira", VcsSecretSlot::JiraAccessToken),
        };
        let creds = concerto_persist::vcs_credentials::list_by_provider(
            self.persistence.readers(),
            provider_str,
        )
        .await?;
        // Most-recently-updated row = the connected account (single-account V1.0).
        let scope = creds
            .into_iter()
            .max_by_key(|c| c.updated_at)
            .map(|c| c.scope_id);
        match scope {
            Some(scope_id) => Ok(self.secrets.get_vcs_secret(&scope_id, slot).await?),
            None => Ok(None),
        }
    }
}

/// Build the LIVE Linear/Jira issue write-back (Task 320.5) the Workarea
/// Manager calls at the end of a coordinated-merge success path. Reads tokens
/// through the keychain ([`KeychainWriteBackTokens`]); mints nothing. Wired at
/// boot via `WorkareaManager::with_issue_write_back`.
pub fn build_issue_write_back(
    persistence: Arc<Persistence>,
) -> Result<Arc<dyn concerto_vcs::IssueWriteBack>> {
    let tokens = Arc::new(KeychainWriteBackTokens {
        secrets: Secrets::new(),
        persistence,
    });
    Ok(Arc::new(concerto_vcs::LinearJiraWriteBack::new(tokens)?))
}

/// The Core's [`WebhookSink`] (Task 315): the seam the transport invokes for every
/// demuxed `0x04` Webhook stream. Maps the on-wire [`WebhookEnvelope`] onto a
/// repo + [`WebhookPayload`] and drives `VcsHandle::ingest_webhook`, then maps the
/// [`IngestOutcome`] to the transport [`WebhookAck`] byte the relay chains back to
/// GitHub as an HTTP status.
struct CoreWebhookSink {
    handle: VcsHandle,
}

impl CoreWebhookSink {
    /// Resolve the targeted repo, verify + process. Kept separate so the trait
    /// `ingest` stays a thin boxed-future wrapper. Returns the [`IngestOutcome`]
    /// the caller maps to a [`WebhookAck`].
    async fn process(&self, envelope: WebhookEnvelope) -> IngestOutcome {
        // The relay's body is GitHub's signed JSON; the `repository.full_name`
        // identifies which tracked repo (and thus which per-repo HMAC secret +
        // cache rows) the delivery targets. We read it BEFORE HMAC only to route;
        // the authenticity floor is still the per-repo HMAC `ingest_webhook` runs.
        let full_name = match serde_json::from_slice::<serde_json::Value>(&envelope.body)
            .ok()
            .and_then(|v| {
                v.get("repository")
                    .and_then(|r| r.get("full_name"))
                    .and_then(|n| n.as_str())
                    .map(str::to_string)
            }) {
            Some(name) => name,
            None => {
                // No `repository.full_name` to route on (e.g. a `ping` with no
                // repo, or a malformed body). We cannot key a secret → reject
                // (4xx) with no sender-visible reason (`design/13 §8`).
                tracing::warn!("webhook: body carries no repository.full_name; dropping (4xx)");
                return IngestOutcome::Reject;
            }
        };

        let repo = match self.handle.resolve_repo_id_by_full_name(&full_name).await {
            Ok(Some(repo)) => repo,
            Ok(None) => {
                // The webhook targets a repo this Core does not manage. No secret
                // is keyed → reject (4xx), no sender-visible reason.
                tracing::warn!(repo = %full_name, "webhook: untracked repository; dropping (4xx)");
                return IngestOutcome::Reject;
            }
            Err(e) => {
                tracing::warn!(error = %e, repo = %full_name, "webhook: repo resolve failed (5xx)");
                return IngestOutcome::Error;
            }
        };

        let payload = WebhookPayload {
            delivery_id: envelope.delivery_id,
            signature_256: envelope.signature_256,
            event_type: envelope.event_type,
            body: envelope.body,
        };
        match self.handle.ingest_webhook(&repo, payload).await {
            Ok(outcome) => outcome,
            Err(e) => {
                tracing::warn!(error = %e, repo = %repo, "webhook: ingest errored (5xx)");
                IngestOutcome::Error
            }
        }
    }
}

impl WebhookSink for CoreWebhookSink {
    fn ingest(
        &self,
        envelope: WebhookEnvelope,
    ) -> Pin<Box<dyn std::future::Future<Output = WebhookAck> + Send>> {
        // `Arc<dyn WebhookSink>` is held by the transport; clone the cheap
        // `VcsHandle` into the future so it owns no borrow of `self`.
        let handle = self.handle.clone();
        Box::pin(async move {
            let sink = CoreWebhookSink { handle };
            match sink.process(envelope).await {
                IngestOutcome::Accepted => WebhookAck::Accepted,
                IngestOutcome::Reject => WebhookAck::Reject,
                IngestOutcome::Error => WebhookAck::Error,
            }
        })
    }
}

/// Build the Core's inbound-webhook [`WebhookSink`] (Task 315) over a `VcsHandle`
/// freshly equipped with the keychain-backed secret + provider seams. The caller
/// (`boot.rs`) installs the returned sink on the transport via
/// [`concerto_transport::IrohTransport::set_webhook_sink`] before `serve_iroh`.
/// Each call constructs its own `Secrets` handle (cheap; just the service name).
pub fn build_webhook_sink(vcs_handle: VcsHandle) -> Arc<dyn WebhookSink> {
    let secret_source = Arc::new(KeychainWebhookSecretSource {
        secrets: Secrets::new(),
    });
    let provider_source = Arc::new(KeychainWebhookProviderSource {
        secrets: Secrets::new(),
        handle: vcs_handle.clone(),
    });
    let handle = vcs_handle.with_webhook_sources(secret_source, provider_source);
    Arc::new(CoreWebhookSink { handle })
}

// ===========================================================================
// Task 318 — `CheckRunsSource` for `VcsHandle` (the Scheduler's poll source)
// ===========================================================================
//
// The Scheduler's `wait_for_check_runs` (Task 318) polls a `CheckRunsSource`
// trait, not the concrete `VcsHandle`. The trait is defined in `concerto-core`
// (it is the Scheduler's seam); `VcsHandle` lives in the `concerto-vcs` leaf
// crate. Implementing a local trait for the foreign handle is allowed by the
// orphan rule and keeps `concerto-vcs` free of any Scheduler dependency. The
// production poll delegates to the Task-45 `get_check_runs` + maps each
// `gh_cli::CheckRun` → the transport-free `CheckRunSnapshot`; the webhook
// fast-path subscribes to the `ChecksAggregator` broadcast (Task 316's
// `checks.<wa>.<repo>` emits, fed by Task 315's receiver), filtered to the
// target `repository_id`.

// The Scheduler module (and thus `wait_checks`) is `#![cfg(unix)]` — agent-host
// PTY is unix-only in V1.0 (Windows ConPTY scheduler is Task 702 / Phase 7). So
// this impl, which references `crate::scheduler::wait_checks`, is unix-gated to
// match; on Windows the whole `wait_for_check_runs` path is absent.
#[cfg(unix)]
#[async_trait]
impl crate::scheduler::wait_checks::CheckRunsSource for VcsHandle {
    async fn check_runs(
        &self,
        repo: &concerto_persist::RepositoryId,
        sha: &str,
    ) -> Result<Vec<crate::scheduler::wait_checks::CheckRunSnapshot>> {
        let runs = self.get_check_runs(repo, sha).await?;
        Ok(runs
            .into_iter()
            .map(|r| crate::scheduler::wait_checks::CheckRunSnapshot {
                name: r.name,
                status: r.status,
                conclusion: r.conclusion,
            })
            .collect())
    }

    fn webhook_wake(
        &self,
        repo: &concerto_persist::RepositoryId,
    ) -> Option<crate::scheduler::wait_checks::WebhookWake> {
        // Advisory only: a `checks.<wa>.<repo>` event for this repo cancels the
        // current backoff sleep so the loop re-polls immediately. The
        // authoritative state always comes from the re-poll (`check_runs`), so a
        // missed/absent webhook only costs a backoff step — never correctness.
        Some(crate::scheduler::wait_checks::WebhookWake::new(
            self.checks().subscribe(),
            repo.0.clone(),
        ))
    }
}
