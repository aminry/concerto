//! Review-thread / check-run / deployment aggregation + the
//! `checks.<workarea_id>.<repository_id>` opaque-frame event emission
//! (Task 316, `design/13 §3.6`/§4/§5.3/§6.3).
//!
//! This is the in-memory freshness layer over the [`VcsProvider`] GraphQL/REST
//! methods. It owns:
//!
//! - **The caches** (`design/13 §4`): `threads_cache` keyed by PR (refresh on
//!   workarea open), `check_cache` keyed `(repo, sha)` with a **30s TTL**, and a
//!   per-`(repo, ref)` deployment cache. NONE are persisted to SQLite — GitHub
//!   is canonical (`design/13 §3.6`/R-3).
//! - **Event emission** of `pr.thread_updated` / `pr.check_run_updated` /
//!   `pr.deployment_updated` on the `checks.<wa>.<repo>` subject (`design/13
//!   §5.3`), carrying an **opaque JSON frame** (FROZEN format — see
//!   [`build_frame`]) that the streams layer rides on the non-oneof
//!   `Event.checks_opaque = 17` field (PHASE3_PLANNING §2). Events fire only on
//!   a *change* (the new value differs from the cached one), so a webhook + a
//!   poll that observe the same state do not double-emit.
//! - **The three §6.3 invalidation paths**: webhook-targeted (the
//!   [`ChecksAggregator::invalidate_*`] hooks Task 315 calls on receipt),
//!   TTL-lazy (the 30s `check_cache` staleness check on read), and user
//!   force-refresh ([`ChecksAggregator::refresh_workarea`]).
//!
//! The aggregator is provider-agnostic: it is handed an `Arc<dyn VcsProvider>`
//! per call, so the Core wires the dispatched (rate-limit-pool-aware) provider
//! and the `testkit` wires a `FakeGitHub`-backed `GitHubProvider`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use concerto_error::Result;
use serde::Serialize;
use tokio::sync::broadcast;

use crate::dispatch::{system_now_secs, NowSecs};
use crate::provider::{CheckRun, Deployment, ProviderPrId, ReviewThread, ThreadId, VcsProvider};

/// The check-run cache TTL (`design/13 §4`): 30 seconds.
pub const CHECK_CACHE_TTL_SECS: i64 = 30;

/// Broadcast capacity for VCS events → the streams pump. Generous relative to
/// the per-subject ring so a healthy subscriber never lags (mirrors the
/// streams layer's own `LIVE_BROADCAST_CAP`).
const VCS_EVENT_CHANNEL_CAP: usize = 1024;

/// One `checks.<workarea_id>.<repository_id>` event: the routing scope + the
/// opaque JSON frame the streams layer puts on `Event.checks_opaque = 17`.
///
/// The aggregator broadcasts these; [`crate::checks::ChecksAggregator::subscribe`]
/// hands a receiver to the Core's `StreamsHandler`, which filters by
/// `(workarea_id, repository_id)` and wraps `frame` into the wire `Event`.
#[derive(Debug, Clone)]
pub struct VcsEvent {
    pub workarea_id: String,
    pub repository_id: String,
    /// The opaque frame (deterministic JSON; see [`build_frame`]).
    pub frame: Vec<u8>,
}

/// The discriminator on the opaque frame (`design/13 §5.3` event kinds). FROZEN
/// string values — Task 324 switches on these.
pub const KIND_THREAD_UPDATED: &str = "thread_updated";
pub const KIND_CHECK_RUN_UPDATED: &str = "check_run_updated";
pub const KIND_DEPLOYMENT_UPDATED: &str = "deployment_updated";

// --- Frame entity projections (the FROZEN opaque-frame `entity` shapes) ---

#[derive(Serialize)]
struct FrameThread<'a> {
    id: &'a str,
    resolved: bool,
    path: Option<&'a str>,
    comments: &'a [String],
}

#[derive(Serialize)]
struct FrameCheckRun<'a> {
    name: &'a str,
    status: &'a str,
    conclusion: &'a str,
    details_url: &'a str,
}

#[derive(Serialize)]
struct FrameChecks<'a> {
    sha: &'a str,
    runs: Vec<FrameCheckRun<'a>>,
}

#[derive(Serialize)]
struct FrameDeployment<'a> {
    id: &'a str,
    environment: &'a str,
    state: &'a str,
    #[serde(rename = "ref")]
    ref_: &'a str,
}

#[derive(Serialize)]
struct FrameDeployments<'a> {
    #[serde(rename = "ref")]
    ref_: &'a str,
    deployments: Vec<FrameDeployment<'a>>,
}

/// Build the FROZEN opaque frame for a `checks.<wa>.<repo>` event:
/// `{ kind, workarea_id, repository_id, entity }`. Serialization is
/// deterministic (serde struct field order is stable), so the same change
/// always produces the same bytes. Documented on `streams.proto`'s
/// `Event.checks_opaque`.
fn build_frame<E: Serialize>(
    kind: &str,
    workarea_id: &str,
    repository_id: &str,
    entity: E,
) -> Vec<u8> {
    let frame = serde_json::json!({
        "kind": kind,
        "workarea_id": workarea_id,
        "repository_id": repository_id,
        "entity": entity,
    });
    // `to_vec` on a `serde_json::Value` never fails (the value is already valid).
    serde_json::to_vec(&frame).unwrap_or_default()
}

/// Build the `thread_updated` frame for a single resolved/changed thread.
pub fn thread_frame(workarea_id: &str, repository_id: &str, thread: &ReviewThread) -> Vec<u8> {
    build_frame(
        KIND_THREAD_UPDATED,
        workarea_id,
        repository_id,
        FrameThread {
            id: &thread.id.0,
            resolved: thread.resolved,
            path: thread.path.as_deref(),
            comments: &thread.comments,
        },
    )
}

/// Build the `check_run_updated` frame for a `(sha, runs)` set.
pub fn check_run_frame(
    workarea_id: &str,
    repository_id: &str,
    sha: &str,
    runs: &[CheckRun],
) -> Vec<u8> {
    build_frame(
        KIND_CHECK_RUN_UPDATED,
        workarea_id,
        repository_id,
        FrameChecks {
            sha,
            runs: runs
                .iter()
                .map(|r| FrameCheckRun {
                    name: &r.name,
                    status: &r.status,
                    conclusion: &r.conclusion,
                    details_url: &r.details_url,
                })
                .collect(),
        },
    )
}

/// Build the `deployment_updated` frame for a `(ref, deployments)` set.
pub fn deployment_frame(
    workarea_id: &str,
    repository_id: &str,
    ref_: &str,
    deployments: &[Deployment],
) -> Vec<u8> {
    build_frame(
        KIND_DEPLOYMENT_UPDATED,
        workarea_id,
        repository_id,
        FrameDeployments {
            ref_,
            deployments: deployments
                .iter()
                .map(|d| FrameDeployment {
                    id: &d.id,
                    environment: &d.environment,
                    state: &d.state,
                    ref_: &d.ref_,
                })
                .collect(),
        },
    )
}

/// A cached check-run set + the time it was fetched (for the 30s TTL).
struct CachedChecks {
    fetched_at: i64,
    runs: Vec<CheckRun>,
}

/// The in-memory VCS aggregation caches + the event broadcaster (`design/13
/// §4`). Cheap-clone (`Arc`-wrapped interior) so the Core's handle + the
/// gRPC handler share one aggregator. NONE of these caches are persisted.
#[derive(Clone)]
pub struct ChecksAggregator {
    inner: Arc<Inner>,
}

struct Inner {
    /// `threads_cache` keyed by PR (`repo_full_name#number`) — refreshed on
    /// workarea open (`design/13 §4`). `Vec<ReviewThread>`.
    threads: Mutex<HashMap<String, Vec<ReviewThread>>>,
    /// `check_cache` keyed `(repo, sha)` with a 30s TTL (`design/13 §4`).
    checks: Mutex<HashMap<(String, String), CachedChecks>>,
    /// Deployment cache keyed `(repo, ref)`.
    deployments: Mutex<HashMap<(String, String), Vec<Deployment>>>,
    /// The event broadcaster.
    events: broadcast::Sender<VcsEvent>,
    /// Injectable clock (the production wall clock; `testkit` synthetic clock in
    /// tests) so the 30s-TTL refetch path is deterministic.
    now: NowSecs,
}

/// The PR cache key (`repo_full_name#number`) for `threads_cache`.
fn pr_key(id: &ProviderPrId) -> String {
    format!("{}#{}", id.repo_full_name, id.number)
}

impl ChecksAggregator {
    /// Build an aggregator on the wall clock (the production path).
    pub fn new() -> Self {
        Self::with_clock(Arc::new(system_now_secs))
    }

    /// Build an aggregator with an injectable clock (the `testkit` synthetic
    /// clock drives the 30s-TTL refetch deterministically).
    pub fn with_clock(now: NowSecs) -> Self {
        let (events, _rx) = broadcast::channel(VCS_EVENT_CHANNEL_CAP);
        Self {
            inner: Arc::new(Inner {
                threads: Mutex::new(HashMap::new()),
                checks: Mutex::new(HashMap::new()),
                deployments: Mutex::new(HashMap::new()),
                events,
                now,
            }),
        }
    }

    /// Subscribe to the VCS event stream (the Core's `StreamsHandler` filters
    /// by `(workarea_id, repository_id)` and wraps each into a wire `Event`).
    pub fn subscribe(&self) -> broadcast::Receiver<VcsEvent> {
        self.inner.events.subscribe()
    }

    /// The broadcast sender, for the Core to clone into the streams handler.
    pub fn sender(&self) -> broadcast::Sender<VcsEvent> {
        self.inner.events.clone()
    }

    fn emit(&self, workarea_id: &str, repository_id: &str, frame: Vec<u8>) {
        // A send error means no subscribers are attached — that is fine (a
        // co-located Core with the Checks panel closed). Drop silently.
        let _ = self.inner.events.send(VcsEvent {
            workarea_id: workarea_id.to_string(),
            repository_id: repository_id.to_string(),
            frame,
        });
    }

    // ----- Review threads (`design/13 §3.6`) -----

    /// Fetch a PR's review threads via the provider's GraphQL query, replace the
    /// cached set, and emit a `pr.thread_updated` event per thread whose
    /// resolved-flag/comments changed (or all on first fetch / force-refresh).
    /// This is the **refresh-on-workarea-open** path + the force-refresh path.
    /// Returns the fresh threads (also what the gRPC handler returns).
    pub async fn list_review_threads(
        &self,
        provider: &Arc<dyn VcsProvider>,
        workarea_id: &str,
        repository_id: &str,
        pr: ProviderPrId,
    ) -> Result<Vec<ReviewThread>> {
        let threads = provider.list_review_threads(pr.clone()).await?;
        let key = pr_key(&pr);
        let prev = self
            .inner
            .threads
            .lock()
            .expect("threads")
            .get(&key)
            .cloned();
        // Emit for each thread that is new or changed since the last cache.
        for thread in &threads {
            let changed = match &prev {
                Some(prev_threads) => prev_threads
                    .iter()
                    .find(|t| t.id == thread.id)
                    .map(|t| t != thread)
                    .unwrap_or(true),
                None => true,
            };
            if changed {
                self.emit(
                    workarea_id,
                    repository_id,
                    thread_frame(workarea_id, repository_id, thread),
                );
            }
        }
        self.inner
            .threads
            .lock()
            .expect("threads")
            .insert(key, threads.clone());
        Ok(threads)
    }

    /// Resolve a thread via the provider's GraphQL mutation; on success flip the
    /// cached thread's resolved flag + emit `pr.thread_updated`. `pr` locates
    /// the cached thread set (the mutation itself keys only on `thread_id`).
    pub async fn resolve_thread(
        &self,
        provider: &Arc<dyn VcsProvider>,
        workarea_id: &str,
        repository_id: &str,
        pr: &ProviderPrId,
        thread_id: ThreadId,
    ) -> Result<()> {
        provider.resolve_thread(thread_id.clone()).await?;
        let key = pr_key(pr);
        // Update the cached thread's resolved flag + grab a clone to frame.
        let updated = {
            let mut threads = self.inner.threads.lock().expect("threads");
            threads.get_mut(&key).and_then(|set| {
                set.iter_mut().find(|t| t.id == thread_id).map(|t| {
                    t.resolved = true;
                    t.clone()
                })
            })
        };
        // Emit the change. If the thread was not in the cache (resolve without a
        // prior list), emit a minimal resolved frame so the UI still updates.
        let frame = match &updated {
            Some(thread) => thread_frame(workarea_id, repository_id, thread),
            None => thread_frame(
                workarea_id,
                repository_id,
                &ReviewThread {
                    id: thread_id,
                    resolved: true,
                    path: None,
                    comments: Vec::new(),
                },
            ),
        };
        self.emit(workarea_id, repository_id, frame);
        Ok(())
    }

    /// Read the cached threads for a PR without a fetch (the gRPC handler's
    /// cache-first read; `None` when never fetched). Used by "Send to agent".
    pub fn cached_threads(&self, pr: &ProviderPrId) -> Option<Vec<ReviewThread>> {
        self.inner
            .threads
            .lock()
            .expect("threads")
            .get(&pr_key(pr))
            .cloned()
    }

    // ----- Check runs (`design/13 §4`, TTL 30s) -----

    /// Get check runs for `(repo, sha)`, serving the cache when the entry is
    /// fresher than the 30s TTL (the §6.3 **TTL-lazy** path) and re-fetching
    /// otherwise. On a fetch whose result differs from the cache, emit
    /// `pr.check_run_updated`. `force` (the §6.3 force-refresh path) always
    /// re-fetches.
    pub async fn check_runs(
        &self,
        provider: &Arc<dyn VcsProvider>,
        workarea_id: &str,
        repository_id: &str,
        repo_full_name: &str,
        sha: &str,
        force: bool,
    ) -> Result<Vec<CheckRun>> {
        let cache_key = (repo_full_name.to_string(), sha.to_string());
        let now = (self.inner.now)();
        if !force {
            if let Some(cached) = self.inner.checks.lock().expect("checks").get(&cache_key) {
                if now - cached.fetched_at < CHECK_CACHE_TTL_SECS {
                    return Ok(cached.runs.clone());
                }
            }
        }
        let runs = provider.list_check_runs(repo_full_name, sha).await?;
        let changed = self
            .inner
            .checks
            .lock()
            .expect("checks")
            .get(&cache_key)
            .map(|c| c.runs != runs)
            .unwrap_or(true);
        self.inner.checks.lock().expect("checks").insert(
            cache_key,
            CachedChecks {
                fetched_at: now,
                runs: runs.clone(),
            },
        );
        if changed {
            self.emit(
                workarea_id,
                repository_id,
                check_run_frame(workarea_id, repository_id, sha, &runs),
            );
        }
        Ok(runs)
    }

    // ----- Deployments (`design/13 §3.8`) -----

    /// List deployments for `(repo, ref)` + aggregate their statuses (via the
    /// provider), caching the result and emitting `pr.deployment_updated` on a
    /// change.
    pub async fn list_deployments(
        &self,
        provider: &Arc<dyn VcsProvider>,
        workarea_id: &str,
        repository_id: &str,
        repo_full_name: &str,
        ref_: &str,
    ) -> Result<Vec<Deployment>> {
        let deployments = provider.list_deployments(repo_full_name, ref_).await?;
        let cache_key = (repo_full_name.to_string(), ref_.to_string());
        let changed = self
            .inner
            .deployments
            .lock()
            .expect("deployments")
            .get(&cache_key)
            .map(|d| d != &deployments)
            .unwrap_or(true);
        self.inner
            .deployments
            .lock()
            .expect("deployments")
            .insert(cache_key, deployments.clone());
        if changed {
            self.emit(
                workarea_id,
                repository_id,
                deployment_frame(workarea_id, repository_id, ref_, &deployments),
            );
        }
        Ok(deployments)
    }

    // ----- §6.3 invalidation paths -----

    /// **Webhook-targeted invalidation** (`design/13 §6.3`, the seam Task 315
    /// calls on a `pull_request_review_thread` webhook): drop just the affected
    /// PR's threads cache + re-fetch + emit. Until 315 lands this is poll-only;
    /// it is exercised by a fake invalidation hook in the Tier-2 tests.
    pub async fn invalidate_threads(
        &self,
        provider: &Arc<dyn VcsProvider>,
        workarea_id: &str,
        repository_id: &str,
        pr: ProviderPrId,
    ) -> Result<Vec<ReviewThread>> {
        self.inner
            .threads
            .lock()
            .expect("threads")
            .remove(&pr_key(&pr));
        self.list_review_threads(provider, workarea_id, repository_id, pr)
            .await
    }

    /// **Webhook-targeted invalidation** for a `check_run` webhook: drop the
    /// `(repo, sha)` check cache entry + force a re-fetch + emit.
    pub async fn invalidate_check_runs(
        &self,
        provider: &Arc<dyn VcsProvider>,
        workarea_id: &str,
        repository_id: &str,
        repo_full_name: &str,
        sha: &str,
    ) -> Result<Vec<CheckRun>> {
        self.inner
            .checks
            .lock()
            .expect("checks")
            .remove(&(repo_full_name.to_string(), sha.to_string()));
        self.check_runs(
            provider,
            workarea_id,
            repository_id,
            repo_full_name,
            sha,
            true,
        )
        .await
    }

    /// **Webhook-targeted invalidation** for a `deployment_status` webhook: drop
    /// the `(repo, ref)` cache + re-fetch + emit.
    pub async fn invalidate_deployments(
        &self,
        provider: &Arc<dyn VcsProvider>,
        workarea_id: &str,
        repository_id: &str,
        repo_full_name: &str,
        ref_: &str,
    ) -> Result<Vec<Deployment>> {
        self.inner
            .deployments
            .lock()
            .expect("deployments")
            .remove(&(repo_full_name.to_string(), ref_.to_string()));
        self.list_deployments(provider, workarea_id, repository_id, repo_full_name, ref_)
            .await
    }

    /// **User force-refresh** (`design/13 §6.3`, pull-to-refresh): re-fetch
    /// threads + checks + deployments for one PR in the open workarea and emit
    /// every change. `head_sha`/`base_ref` come from the PR row. Drops the
    /// per-PR caches first so every entity is re-fetched and re-emitted.
    pub async fn refresh_workarea_pr(
        &self,
        provider: &Arc<dyn VcsProvider>,
        workarea_id: &str,
        repository_id: &str,
        pr: ProviderPrId,
        head_sha: &str,
    ) -> Result<()> {
        let repo = pr.repo_full_name.clone();
        // Drop the caches so the re-fetch always emits.
        self.inner
            .threads
            .lock()
            .expect("threads")
            .remove(&pr_key(&pr));
        self.inner
            .checks
            .lock()
            .expect("checks")
            .remove(&(repo.clone(), head_sha.to_string()));
        self.inner
            .deployments
            .lock()
            .expect("deployments")
            .remove(&(repo.clone(), head_sha.to_string()));
        self.list_review_threads(provider, workarea_id, repository_id, pr.clone())
            .await?;
        self.check_runs(provider, workarea_id, repository_id, &repo, head_sha, true)
            .await?;
        self.list_deployments(provider, workarea_id, repository_id, &repo, head_sha)
            .await?;
        Ok(())
    }
}

impl Default for ChecksAggregator {
    fn default() -> Self {
        Self::new()
    }
}
