//! The issue **write-back** trait seam (Task 317, locked decision D5).
//!
//! `design/13 §12 R-9` (and the PRD) wants Linear/Jira issue status transitions
//! on coordinated-PR-set-merge completion (per-project opt-in). That *write* is
//! Task 320.5's — it hangs off the coordinated-merge loop. **317 ships only the
//! seam**: the [`IssueWriteBack`] trait + a LIVE no-op [`NoopWriteBack`] impl, so
//! 320.5 plugs its real `LinearJiraWriteBack` in behind the same trait without
//! re-touching 317.
//!
//! ## FROZEN surface (do not change in 320.5)
//!
//! - [`IssueWriteBack::transition_on_merge`] — the one method. Signature frozen.
//! - [`IssueRef`] — `{ provider, external_id, project_url }`. Field set frozen.
//! - [`IssueProvider`] ∈ `{ Linear, Jira }` — the trackers 317 supports.
//! - [`IssueTransition`] — `#[non_exhaustive]`; V1.0 ships only
//!   [`IssueTransition::MergedDone`] (the merge-completion forward transition).
//!   320.5 implements the trait for `MergedDone` and adds NO variant.
//!
//! The trait is `Send + Sync` (it is held behind `Arc<dyn IssueWriteBack>` by
//! the coordinated-merge loop) and `async` (via `async_trait`).

use std::sync::Arc;

use async_trait::async_trait;
use concerto_error::{Error, Result};
use concerto_keychain::SecretValue;
use serde::Deserialize;
use url::Url;

/// Which tracker an [`IssueRef`] belongs to (Task 317). Frozen vocabulary —
/// the two trackers the Linear/Jira clients support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueProvider {
    Linear,
    Jira,
}

impl IssueProvider {
    /// The stable lowercase wire/log spelling (`"linear"` / `"jira"`).
    pub fn as_str(self) -> &'static str {
        match self {
            IssueProvider::Linear => "linear",
            IssueProvider::Jira => "jira",
        }
    }
}

/// A reference to a tracker issue the write-back targets (Task 317, FROZEN).
///
/// Identifies the issue to transition: the tracker ([`IssueProvider`]), its
/// provider-native string id (`ENG-123` / `PROJ-45`), and the project URL the
/// fetch came from (the Linear workspace / Jira cloud base, so 320.5 can resolve
/// the right credential + base URL). Field set frozen — 320.5 consumes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRef {
    pub provider: IssueProvider,
    /// The provider-native id (`ENG-123` / `PROJ-45`).
    pub external_id: String,
    /// The issue/project URL (e.g. the `linear.app/...` or `*.atlassian.net`
    /// base) — lets 320.5 resolve the credential scope + the API base.
    pub project_url: String,
}

/// The issue-status transition vocabulary (Task 317, FROZEN, `#[non_exhaustive]`).
///
/// V1.0 ships exactly one transition: the forward "the coordinated PR set
/// merged, mark the issue done" move. `#[non_exhaustive]` reserves room for
/// future transitions (e.g. a revert→reopen) without a breaking change, but
/// 320.5 adds NO variant — it implements [`IssueWriteBack`] for `MergedDone`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IssueTransition {
    /// The workarea's coordinated PR set finished merging → transition the
    /// linked issue to its done/closed status.
    MergedDone,
}

/// The issue write-back abstraction (Task 317, FROZEN — locked decision D5).
///
/// The coordinated-merge loop (Task 320) calls [`Self::transition_on_merge`]
/// once the PR set finishes merging, when the project opted into issue
/// write-back. 317 wires the no-op [`NoopWriteBack`] as the default; Task 320.5
/// supplies the real Linear (`issueUpdate`) / Jira (transition) impl behind this
/// exact trait. **Do not change the signature in 320.5.**
#[async_trait]
pub trait IssueWriteBack: Send + Sync {
    /// Transition `issue_ref` per `transition` after a coordinated merge.
    ///
    /// The LIVE no-op ([`NoopWriteBack`]) returns `Ok(())`; 320.5's real impl
    /// performs the tracker API call. Errors are the tracker/transport failures
    /// 320.5 surfaces — the no-op never errors.
    async fn transition_on_merge(
        &self,
        issue_ref: &IssueRef,
        transition: IssueTransition,
    ) -> Result<()>;
}

/// The LIVE no-op [`IssueWriteBack`] (Task 317, D5).
///
/// Returns `Ok(())` and logs at `debug` — it does NOT call any tracker. This is
/// the default wired in P3 so the coordinated-merge loop has a real (inert)
/// write-back to hold; Task 320.5 swaps in the transitioning impl behind the
/// same trait. It is NOT a `todo!()`/`unimplemented!()` stub: it is a complete,
/// shippable no-op (the merge-without-write-back is the default project state).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopWriteBack;

#[async_trait]
impl IssueWriteBack for NoopWriteBack {
    async fn transition_on_merge(
        &self,
        issue_ref: &IssueRef,
        transition: IssueTransition,
    ) -> Result<()> {
        tracing::debug!(
            provider = issue_ref.provider.as_str(),
            external_id = %issue_ref.external_id,
            ?transition,
            "issue write-back is the no-op default (real transition lands in Task 320.5)"
        );
        Ok(())
    }
}

// ===========================================================================
// Task 320.5 — the REAL Linear/Jira status-transition write-back.
// ===========================================================================

/// Resolves the per-provider access token the [`LinearJiraWriteBack`] needs
/// (Task 320.5). The Core implements this against the keychain
/// (`VcsSecretSlot::{LinearAccessToken, JiraAccessToken}` via 313's accessor,
/// keyed by the most-recently-connected `vcs_credentials` account for the
/// provider — exactly how the `FetchIssueByUrl` handler resolves the fetch
/// token); tests inject a fake returning a static token pointed at the
/// `testkit` wiremock base. The carrier never owns secret material long-term —
/// the token is read on demand, wrapped in [`SecretValue`], and never logged.
///
/// Returns `Ok(None)` when no credential is configured for the provider (the
/// project opted into write-back but never connected the tracker) — the
/// write-back then records a `skipped`/`failed` outcome rather than erroring
/// the merge.
#[async_trait]
pub trait WriteBackTokens: Send + Sync {
    /// The access token for `provider`, or `None` when no credential is stored.
    async fn token(&self, provider: IssueProvider) -> Result<Option<SecretValue>>;
}

/// The LIVE Linear (`issueUpdate`) / Jira (`POST transitions`) write-back impl
/// (Task 320.5) behind 317's FROZEN [`IssueWriteBack`] trait.
///
/// Performs the real status transition on coordinated-merge completion:
///
/// - **Linear** (GraphQL): resolve the issue's team workflow states, pick the
///   `type == "completed"` state, then run the `issueUpdate(input:{stateId})`
///   mutation. Auth is `Authorization: <token>` (raw token — OAuth access token
///   OR personal API key, the same header form the Linear fetch client uses).
/// - **Jira** (REST): `GET /rest/api/3/issue/{key}/transitions` to find a
///   transition whose target status is a "done"-category status, then
///   `POST /rest/api/3/issue/{key}/transitions` with `{transition:{id}}`. Auth
///   is `Authorization: Bearer <token>`.
///
/// Tokens come from the [`WriteBackTokens`] seam (the Core reads 317's keychain
/// `VcsSecretSlot` accessors; no token is minted here). The production API base
/// for each provider is the issue's `project_url` host; `linear_base`/`jira_base`
/// override it for the `testkit` wiremock doubles. V1.0 ships the single
/// [`IssueTransition::MergedDone`] transition; no enum variant is added.
pub struct LinearJiraWriteBack {
    http: reqwest::Client,
    tokens: Arc<dyn WriteBackTokens>,
    /// Override the Linear GraphQL base (`testkit`). `None` → production.
    linear_base: Option<String>,
    /// Override the Jira REST base (`testkit`). `None` → the issue URL's site.
    jira_base: Option<String>,
}

impl LinearJiraWriteBack {
    /// Build a write-back against the production Linear/Jira endpoints, reading
    /// tokens through `tokens` (the Core's keychain-backed resolver).
    pub fn new(tokens: Arc<dyn WriteBackTokens>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| Error::Vcs(format!("write_back: build http client: {e}")))?;
        Ok(Self {
            http,
            tokens,
            linear_base: None,
            jira_base: None,
        })
    }

    /// Override the Linear API base (the `testkit` wiremock base in tests).
    pub fn with_linear_base(mut self, base: &str) -> Self {
        self.linear_base = Some(base.trim_end_matches('/').to_string());
        self
    }

    /// Override the Jira API base (the `testkit` wiremock base in tests).
    pub fn with_jira_base(mut self, base: &str) -> Self {
        self.jira_base = Some(base.trim_end_matches('/').to_string());
        self
    }

    /// Resolve the Linear GraphQL base: the override, else production.
    fn linear_base(&self) -> String {
        self.linear_base
            .clone()
            .unwrap_or_else(|| crate::linear::DEFAULT_LINEAR_BASE_URI.to_string())
    }

    /// Resolve the Jira REST base: the override, else the Atlassian site derived
    /// from the issue's `project_url`.
    fn jira_base(&self, project_url: &str) -> Result<String> {
        if let Some(base) = &self.jira_base {
            return Ok(base.clone());
        }
        let url = Url::parse(project_url).map_err(|e| {
            Error::Vcs(format!(
                "write_back: jira project_url `{project_url}` is not a URL: {e}"
            ))
        })?;
        let host = url.host_str().ok_or_else(|| {
            Error::Vcs(format!("write_back: jira project_url has no host: {url}"))
        })?;
        Ok(format!("{}://{}", url.scheme(), host))
    }

    /// Run the Linear `issueUpdate` to the issue team's completed workflow state.
    async fn transition_linear(&self, external_id: &str) -> Result<()> {
        let token = self
            .tokens
            .token(IssueProvider::Linear)
            .await?
            .ok_or_else(|| {
                Error::VcsNotAuthenticated(
                    "linear write-back: no Linear credential connected".to_string(),
                )
            })?;
        let endpoint = format!("{}/graphql", self.linear_base());

        // 1. Resolve the team's "completed" workflow state for this issue.
        let query = "query($id:String!){issue(id:$id){id team{states{nodes{id name type}}}}}";
        let body = serde_json::json!({ "query": query, "variables": { "id": external_id } });
        let resp: LinearGraphQl<LinearIssueStatesData> =
            self.linear_post(&endpoint, token.expose(), &body).await?;
        let issue = resp
            .data
            .and_then(|d| d.issue)
            .ok_or_else(|| Error::Vcs(format!("linear write-back: no issue `{external_id}`")))?;
        let state_id = issue
            .team
            .and_then(|t| t.states)
            .map(|s| s.nodes)
            .unwrap_or_default()
            .into_iter()
            .find(|s| s.state_type == "completed")
            .map(|s| s.id)
            .ok_or_else(|| {
                Error::Vcs(format!(
                    "linear write-back: issue `{external_id}` team has no completed workflow state"
                ))
            })?;

        // 2. Move the issue to that state.
        let mutation =
            "mutation($id:String!,$stateId:String!){issueUpdate(id:$id,input:{stateId:$stateId}){success}}";
        let body = serde_json::json!({
            "query": mutation,
            "variables": { "id": issue.id, "stateId": state_id },
        });
        let resp: LinearGraphQl<LinearIssueUpdateData> =
            self.linear_post(&endpoint, token.expose(), &body).await?;
        let success = resp
            .data
            .and_then(|d| d.issue_update)
            .map(|u| u.success)
            .unwrap_or(false);
        if !success {
            return Err(Error::Vcs(format!(
                "linear write-back: issueUpdate for `{external_id}` did not succeed"
            )));
        }
        Ok(())
    }

    /// POST a GraphQL body to Linear, classify the HTTP/GraphQL errors, decode.
    async fn linear_post<T: for<'de> Deserialize<'de>>(
        &self,
        endpoint: &str,
        token: &str,
        body: &serde_json::Value,
    ) -> Result<LinearGraphQl<T>> {
        let resp = self
            .http
            .post(endpoint)
            .header("Authorization", token)
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| Error::Vcs(format!("linear write-back: request failed: {e}")))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::VcsNotAuthenticated(
                "linear write-back: token rejected (401)".to_string(),
            ));
        }
        if !status.is_success() {
            return Err(Error::Vcs(format!("linear write-back: HTTP {status}")));
        }
        let parsed: LinearGraphQl<T> = resp
            .json()
            .await
            .map_err(|e| Error::Vcs(format!("linear write-back: decode response: {e}")))?;
        if let Some(errors) = &parsed.errors {
            if let Some(first) = errors.first() {
                return Err(Error::Vcs(format!(
                    "linear write-back: GraphQL error: {}",
                    first.message
                )));
            }
        }
        Ok(parsed)
    }

    /// Run the Jira `POST transitions` to a "done"-category status.
    async fn transition_jira(&self, external_id: &str, project_url: &str) -> Result<()> {
        let token = self
            .tokens
            .token(IssueProvider::Jira)
            .await?
            .ok_or_else(|| {
                Error::VcsNotAuthenticated(
                    "jira write-back: no Jira credential connected".to_string(),
                )
            })?;
        let base = self.jira_base(project_url)?;
        let route = format!("{base}/rest/api/3/issue/{external_id}/transitions");

        // 1. List transitions; pick one whose target status is "done"-category.
        let resp = self
            .http
            .get(&route)
            .bearer_auth(token.expose())
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| Error::Vcs(format!("jira write-back: list transitions failed: {e}")))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::VcsNotAuthenticated(
                "jira write-back: token rejected (401)".to_string(),
            ));
        }
        if !status.is_success() {
            return Err(Error::Vcs(format!(
                "jira write-back: list transitions HTTP {status}"
            )));
        }
        let listing: JiraTransitions = resp
            .json()
            .await
            .map_err(|e| Error::Vcs(format!("jira write-back: decode transitions: {e}")))?;
        let transition_id = pick_done_transition(&listing.transitions).ok_or_else(|| {
            Error::Vcs(format!(
                "jira write-back: issue `{external_id}` has no done-category transition"
            ))
        })?;

        // 2. Apply the transition.
        let resp = self
            .http
            .post(&route)
            .bearer_auth(token.expose())
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "transition": { "id": transition_id } }))
            .send()
            .await
            .map_err(|e| Error::Vcs(format!("jira write-back: apply transition failed: {e}")))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::VcsNotAuthenticated(
                "jira write-back: token rejected (401)".to_string(),
            ));
        }
        // Jira returns 204 No Content on a successful transition.
        if !status.is_success() {
            return Err(Error::Vcs(format!(
                "jira write-back: apply transition HTTP {status}"
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl IssueWriteBack for LinearJiraWriteBack {
    async fn transition_on_merge(
        &self,
        issue_ref: &IssueRef,
        transition: IssueTransition,
    ) -> Result<()> {
        // V1.0 ships only `MergedDone`. The enum is `#[non_exhaustive]` (317),
        // so adding a future variant in this crate is a compile error here
        // (forcing a deliberate impl), not a silent no-op.
        let IssueTransition::MergedDone = transition;
        match issue_ref.provider {
            IssueProvider::Linear => self.transition_linear(&issue_ref.external_id).await,
            IssueProvider::Jira => {
                self.transition_jira(&issue_ref.external_id, &issue_ref.project_url)
                    .await
            }
        }
    }
}

/// Pick a Jira transition whose **target status category** is "done"
/// (`statusCategory.key == "done"`). Falls back to a transition whose target
/// status *name* contains "done" (case-insensitive) when the category is
/// absent, so a tracker that omits the category metadata still resolves.
fn pick_done_transition(transitions: &[JiraTransition]) -> Option<String> {
    transitions
        .iter()
        .find(|t| {
            t.to.as_ref()
                .and_then(|s| s.status_category.as_ref())
                .map(|c| c.key.eq_ignore_ascii_case("done"))
                .unwrap_or(false)
        })
        .or_else(|| {
            transitions.iter().find(|t| {
                t.to.as_ref()
                    .map(|s| s.name.to_ascii_lowercase().contains("done"))
                    .unwrap_or(false)
            })
        })
        .map(|t| t.id.clone())
}

// --- GraphQL / REST response projections (local; hand-rolled) ---

#[derive(Debug, Deserialize)]
struct LinearGraphQl<T> {
    #[serde(default = "none")]
    data: Option<T>,
    #[serde(default)]
    errors: Option<Vec<LinearGraphQlError>>,
}

fn none<T>() -> Option<T> {
    None
}

#[derive(Debug, Deserialize)]
struct LinearGraphQlError {
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
struct LinearIssueStatesData {
    #[serde(default = "none")]
    issue: Option<LinearIssueStates>,
}

#[derive(Debug, Deserialize)]
struct LinearIssueStates {
    #[serde(default)]
    id: String,
    #[serde(default = "none")]
    team: Option<LinearTeam>,
}

#[derive(Debug, Deserialize)]
struct LinearTeam {
    #[serde(default = "none")]
    states: Option<LinearStateNodes>,
}

#[derive(Debug, Deserialize)]
struct LinearStateNodes {
    #[serde(default)]
    nodes: Vec<LinearWorkflowState>,
}

#[derive(Debug, Deserialize)]
struct LinearWorkflowState {
    #[serde(default)]
    id: String,
    #[serde(default, rename = "type")]
    state_type: String,
}

#[derive(Debug, Deserialize)]
struct LinearIssueUpdateData {
    #[serde(default = "none", rename = "issueUpdate")]
    issue_update: Option<LinearIssueUpdate>,
}

#[derive(Debug, Deserialize)]
struct LinearIssueUpdate {
    #[serde(default)]
    success: bool,
}

#[derive(Debug, Deserialize)]
struct JiraTransitions {
    #[serde(default)]
    transitions: Vec<JiraTransition>,
}

#[derive(Debug, Deserialize)]
struct JiraTransition {
    #[serde(default)]
    id: String,
    #[serde(default = "none")]
    to: Option<JiraTransitionTarget>,
}

#[derive(Debug, Deserialize)]
struct JiraTransitionTarget {
    #[serde(default)]
    name: String,
    #[serde(default = "none", rename = "statusCategory")]
    status_category: Option<JiraStatusCategory>,
}

#[derive(Debug, Deserialize)]
struct JiraStatusCategory {
    #[serde(default)]
    key: String,
}

#[cfg(test)]
mod write_back_tests {
    use super::*;

    #[test]
    fn picks_done_category_transition() {
        let listing: JiraTransitions = serde_json::from_value(serde_json::json!({
            "transitions": [
                { "id": "11", "to": { "name": "In Progress",
                    "statusCategory": { "key": "indeterminate" } } },
                { "id": "31", "to": { "name": "Done",
                    "statusCategory": { "key": "done" } } }
            ]
        }))
        .unwrap();
        assert_eq!(
            pick_done_transition(&listing.transitions).as_deref(),
            Some("31")
        );
    }

    #[test]
    fn falls_back_to_done_in_name_when_no_category() {
        let listing: JiraTransitions = serde_json::from_value(serde_json::json!({
            "transitions": [
                { "id": "11", "to": { "name": "Start" } },
                { "id": "41", "to": { "name": "Mark as Done" } }
            ]
        }))
        .unwrap();
        assert_eq!(
            pick_done_transition(&listing.transitions).as_deref(),
            Some("41")
        );
    }

    #[test]
    fn no_done_transition_returns_none() {
        let listing: JiraTransitions = serde_json::from_value(serde_json::json!({
            "transitions": [ { "id": "11", "to": { "name": "Reopen",
                "statusCategory": { "key": "new" } } } ]
        }))
        .unwrap();
        assert!(pick_done_transition(&listing.transitions).is_none());
    }
}
