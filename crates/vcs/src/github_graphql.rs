//! GitHub GraphQL queries + response projections for review-thread sync
//! (Task 316, `design/13 §3.6`).
//!
//! Review threads + `resolveReviewThread` are **GraphQL-only** on GitHub (the
//! REST API has no review-thread resolution concept). Following the Linear
//! client's precedent ([`crate::linear`]), this hand-rolls the query/mutation
//! strings + typed `serde` projections rather than pulling `graphql_client`'s
//! codegen for two operations — one query, one mutation, a handful of fields.
//! (`graphql_client` is pinned by Task 313 but buys nothing here; see the
//! `crates/vcs/Cargo.toml` note + the Linear client's "No `graphql_client`"
//! comment.)
//!
//! The query/mutation are POSTed to `<base>/graphql` through the same
//! `GitHubProvider::request_json` path the REST methods use, so the
//! `X-RateLimit-*` headers are captured into Task 314's rate-limit pool exactly
//! as for every other call (GraphQL bills the same per-token GitHub budget).
//!
//! Threads are NEVER persisted (`design/13 §3.6`/R-3 — GitHub is canonical);
//! the cache lives in [`crate::dispatch::VcsState::threads_cache`].

use serde::Deserialize;

use crate::provider::{ReviewThread, ThreadId};

/// The GraphQL endpoint path (POSTed against the provider's configured base).
pub const GRAPHQL_PATH: &str = "/graphql";

/// One query: fetch a PR's review threads by `owner`/`name`/`number`, with each
/// thread's id, resolved flag, anchor path, and comment bodies (oldest first).
///
/// The `first:` page sizes (100 threads, 100 comments/thread) cover the
/// overwhelming majority of PRs in one round-trip; deeper pagination is a
/// documented follow-on (`design/13 §3.6` is "one query, full structure" — the
/// page sizes are the practical ceiling, not a correctness bug).
pub const REVIEW_THREADS_QUERY: &str = "\
query($owner:String!,$name:String!,$number:Int!){\
repository(owner:$owner,name:$name){\
pullRequest(number:$number){\
reviewThreads(first:100){\
nodes{id isResolved path comments(first:100){nodes{author{login} body}}}}}}}";

/// The mutation: mark a review thread resolved by its node id. Returns the
/// thread's new `isResolved` so we can confirm the server applied it.
pub const RESOLVE_THREAD_MUTATION: &str = "\
mutation($threadId:ID!){\
resolveReviewThread(input:{threadId:$threadId}){\
thread{id isResolved}}}";

/// Build the JSON request body for [`REVIEW_THREADS_QUERY`].
pub fn review_threads_body(owner: &str, name: &str, number: i64) -> serde_json::Value {
    serde_json::json!({
        "query": REVIEW_THREADS_QUERY,
        "variables": { "owner": owner, "name": name, "number": number },
    })
}

/// Build the JSON request body for [`RESOLVE_THREAD_MUTATION`].
pub fn resolve_thread_body(thread_id: &str) -> serde_json::Value {
    serde_json::json!({
        "query": RESOLVE_THREAD_MUTATION,
        "variables": { "threadId": thread_id },
    })
}

// --- Response projections (local; hand-rolled, mirrors `crate::linear`) ---

/// A GraphQL `errors` entry. GraphQL returns HTTP 200 with an `errors` array on
/// a logical failure, so callers MUST check it even on a 2xx.
#[derive(Debug, Deserialize)]
pub struct GraphQlError {
    #[serde(default)]
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct ReviewThreadsResponse {
    #[serde(default)]
    pub data: Option<ReviewThreadsData>,
    #[serde(default)]
    pub errors: Option<Vec<GraphQlError>>,
}

#[derive(Debug, Deserialize)]
pub struct ReviewThreadsData {
    #[serde(default)]
    pub repository: Option<GqlRepository>,
}

#[derive(Debug, Deserialize)]
pub struct GqlRepository {
    #[serde(default, rename = "pullRequest")]
    pub pull_request: Option<GqlPullRequest>,
}

#[derive(Debug, Deserialize)]
pub struct GqlPullRequest {
    #[serde(default, rename = "reviewThreads")]
    pub review_threads: Option<GqlReviewThreadConnection>,
}

#[derive(Debug, Deserialize)]
pub struct GqlReviewThreadConnection {
    #[serde(default)]
    pub nodes: Vec<GqlReviewThread>,
}

#[derive(Debug, Deserialize)]
pub struct GqlReviewThread {
    #[serde(default)]
    pub id: String,
    #[serde(default, rename = "isResolved")]
    pub is_resolved: bool,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub comments: Option<GqlCommentConnection>,
}

#[derive(Debug, Deserialize)]
pub struct GqlCommentConnection {
    #[serde(default)]
    pub nodes: Vec<GqlComment>,
}

#[derive(Debug, Deserialize)]
pub struct GqlComment {
    #[serde(default)]
    pub body: String,
}

impl GqlReviewThread {
    /// Project the GraphQL node into the FROZEN [`ReviewThread`] value type.
    pub fn into_review_thread(self) -> ReviewThread {
        ReviewThread {
            id: ThreadId(self.id),
            resolved: self.is_resolved,
            // GitHub returns `path: null` for PR-level (non-file) threads; the
            // value type carries `Option<String>`.
            path: self.path.filter(|p| !p.is_empty()),
            comments: self
                .comments
                .map(|c| c.nodes.into_iter().map(|n| n.body).collect())
                .unwrap_or_default(),
        }
    }
}

/// The `resolveReviewThread` mutation response (we only need the new
/// `isResolved` to confirm the server applied the change).
#[derive(Debug, Deserialize)]
pub struct ResolveThreadResponse {
    #[serde(default)]
    pub data: Option<ResolveThreadData>,
    #[serde(default)]
    pub errors: Option<Vec<GraphQlError>>,
}

#[derive(Debug, Deserialize)]
pub struct ResolveThreadData {
    #[serde(default, rename = "resolveReviewThread")]
    pub resolve_review_thread: Option<ResolvePayload>,
}

#[derive(Debug, Deserialize)]
pub struct ResolvePayload {
    #[serde(default)]
    pub thread: Option<ResolvedThread>,
}

#[derive(Debug, Deserialize)]
pub struct ResolvedThread {
    #[serde(default, rename = "isResolved")]
    pub is_resolved: bool,
}

/// Split an `owner/repo` full name into its two segments. Returns `None` when
/// the string is not exactly `owner/repo`.
pub fn split_repo_full_name(full: &str) -> Option<(&str, &str)> {
    let (owner, name) = full.split_once('/')?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    Some((owner, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_full_name() {
        assert_eq!(
            split_repo_full_name("acme/widget"),
            Some(("acme", "widget"))
        );
        assert_eq!(split_repo_full_name("acme"), None);
        assert_eq!(split_repo_full_name("a/b/c"), None);
    }

    #[test]
    fn projects_thread_with_null_path() {
        let node = GqlReviewThread {
            id: "T_1".to_string(),
            is_resolved: true,
            path: None,
            comments: Some(GqlCommentConnection {
                nodes: vec![GqlComment {
                    body: "looks good".to_string(),
                }],
            }),
        };
        let t = node.into_review_thread();
        assert_eq!(t.id, ThreadId("T_1".to_string()));
        assert!(t.resolved);
        assert_eq!(t.path, None);
        assert_eq!(t.comments, vec!["looks good".to_string()]);
    }
}
