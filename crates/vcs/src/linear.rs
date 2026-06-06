//! [`LinearClient`] — the native Linear issue-fetch client (Task 317,
//! `design/13 §3.7`).
//!
//! Linear exposes a single GraphQL endpoint (`https://api.linear.app/graphql`).
//! Given an issue identifier (`ENG-123`) or a `linear.app/.../issue/ENG-123/...`
//! URL, this client POSTs one hand-rolled `issue(id:)` query and maps the
//! response to the shared [`Issue`] value type (title, description→body,
//! `labels[].name`, `state.name`, `identifier`→`external_id`).
//!
//! ## Auth (locked decision D6)
//!
//! Linear accepts **either** an OAuth access token **or** a personal API key in
//! the SAME header form — `Authorization: <token>` (no `Bearer ` prefix; Linear
//! takes the raw token for both). The token is read from the keychain
//! (`VcsSecretSlot::LinearAccessToken`) by the caller and handed in as a
//! [`SecretValue`]; it is never logged. There is **no** transparent refresh on
//! the Linear arm (a personal API key does not refresh, and the OAuth path's
//! refresh is the Desktop-mediated flow re-storing a new token via
//! `SetVcsCredential`) — contrast the Jira arm, which does one refresh on 401.
//!
//! ## No `graphql_client`
//!
//! A single hand-rolled query string + a typed `serde` response struct is enough
//! (the workspace pins `graphql_client` for Task 316's typed GitHub GraphQL, but
//! pulling its codegen here buys nothing for one query). Pure `reqwest` (rustls).

use concerto_error::{Error, Result};
use concerto_keychain::SecretValue;
use serde::Deserialize;
use url::Url;

use crate::provider::Issue;

/// Linear's production GraphQL endpoint.
pub const DEFAULT_LINEAR_BASE_URI: &str = "https://api.linear.app";

/// The one GraphQL query: fetch an issue by its identifier (`ENG-123`) or its
/// UUID. Linear's `issue(id:)` accepts both the human identifier and the UUID.
const ISSUE_QUERY: &str = "query($id:String!){issue(id:$id){identifier title description labels{nodes{name}} state{name} url}}";

/// The native Linear GraphQL client.
///
/// Cheap to build; holds a `reqwest::Client` (rustls) + the resolved base URI.
/// The bearer token is supplied per `fetch` call (the caller reads it from the
/// keychain) so the client itself never owns secret material.
pub struct LinearClient {
    http: reqwest::Client,
    base_uri: String,
}

impl LinearClient {
    /// Build a client against Linear's production endpoint.
    pub fn new() -> Result<Self> {
        Self::with_base(DEFAULT_LINEAR_BASE_URI)
    }

    /// Build a client against a caller-supplied base URI (the `testkit`
    /// wiremock base in tests; the production endpoint otherwise).
    pub fn with_base(base_uri: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| Error::Vcs(format!("linear: build http client: {e}")))?;
        Ok(Self {
            http,
            base_uri: base_uri.trim_end_matches('/').to_string(),
        })
    }

    /// Fetch an issue by `ENG-123` (or a `linear.app/.../issue/ENG-123/...` URL)
    /// authenticating with `token` (an OAuth access token or a personal API
    /// key — same header form, D6). Maps the GraphQL response to [`Issue`].
    ///
    /// `Issue.number` is `0` (Linear has no integer id); the human identifier
    /// is in [`Issue::external_id`]. Returns [`Error::Vcs`] on a transport /
    /// GraphQL error and [`Error::NotFound`] when Linear reports no such issue.
    pub async fn fetch(&self, id_or_url: &str, token: &SecretValue) -> Result<Issue> {
        let id = parse_linear_id(id_or_url)?;
        let endpoint = format!("{}/graphql", self.base_uri);
        let body = serde_json::json!({
            "query": ISSUE_QUERY,
            "variables": { "id": id },
        });
        let resp = self
            .http
            .post(&endpoint)
            // Linear takes the raw token (OAuth access token OR personal API
            // key) in `Authorization` with no `Bearer ` prefix.
            .header("Authorization", token.expose())
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Vcs(format!("linear: request failed: {e}")))?;

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::VcsNotAuthenticated(
                "linear: token rejected (401); reconnect Linear in Settings".to_string(),
            ));
        }
        if !status.is_success() {
            return Err(Error::Vcs(format!("linear: HTTP {status}")));
        }
        let parsed: GraphQlResponse = resp
            .json()
            .await
            .map_err(|e| Error::Vcs(format!("linear: decode response: {e}")))?;

        if let Some(errors) = parsed.errors {
            if let Some(first) = errors.into_iter().next() {
                return Err(Error::Vcs(format!(
                    "linear: GraphQL error: {}",
                    first.message
                )));
            }
        }
        let issue = parsed
            .data
            .and_then(|d| d.issue)
            .ok_or_else(|| Error::NotFound(format!("linear: no issue `{id}`")))?;
        Ok(issue.into_issue())
    }
}

/// Parse a Linear issue identifier from either a bare `ENG-123` id or a
/// `linear.app/<workspace>/issue/ENG-123/<slug>` URL.
///
/// A bare id (no scheme/host) passes through verbatim; a URL has its
/// `/issue/<ID>` segment extracted. Anything else is a [`Error::Validation`].
pub fn parse_linear_id(id_or_url: &str) -> Result<String> {
    let trimmed = id_or_url.trim();
    if trimmed.is_empty() {
        return Err(Error::Validation("linear: empty issue id/url".to_string()));
    }
    // A bare identifier (no `://`) is taken as-is.
    if !trimmed.contains("://") {
        return Ok(trimmed.to_string());
    }
    let url = Url::parse(trimmed)
        .map_err(|e| Error::Validation(format!("linear: invalid URL `{trimmed}`: {e}")))?;
    // Path: /<workspace>/issue/<ID>/<slug>  → take the segment after `issue`.
    let segments: Vec<&str> = url.path().trim_matches('/').split('/').collect();
    if let Some(pos) = segments.iter().position(|s| *s == "issue") {
        if let Some(id) = segments.get(pos + 1) {
            if !id.is_empty() {
                return Ok((*id).to_string());
            }
        }
    }
    Err(Error::Validation(format!(
        "linear: URL `{trimmed}` has no /issue/<ID> segment"
    )))
}

// --- GraphQL response projections (local; one query, hand-rolled) ---

#[derive(Debug, Deserialize)]
struct GraphQlResponse {
    #[serde(default)]
    data: Option<GraphQlData>,
    #[serde(default)]
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
struct GraphQlData {
    #[serde(default)]
    issue: Option<LinearIssue>,
}

#[derive(Debug, Deserialize)]
struct LinearIssue {
    #[serde(default)]
    identifier: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    labels: Option<LinearLabels>,
    #[serde(default)]
    state: Option<LinearState>,
    #[serde(default)]
    url: String,
}

#[derive(Debug, Deserialize)]
struct LinearLabels {
    #[serde(default)]
    nodes: Vec<LinearLabelNode>,
}

#[derive(Debug, Deserialize)]
struct LinearLabelNode {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct LinearState {
    #[serde(default)]
    name: String,
}

impl LinearIssue {
    fn into_issue(self) -> Issue {
        Issue {
            // Linear has no integer id; the human identifier goes in external_id.
            number: 0,
            title: self.title,
            body: self.description.unwrap_or_default(),
            state: self
                .state
                .map(|s| s.name.to_lowercase())
                .unwrap_or_default(),
            url: self.url,
            labels: self
                .labels
                .map(|l| l.nodes.into_iter().map(|n| n.name).collect())
                .unwrap_or_default(),
            external_id: self.identifier,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_id() {
        assert_eq!(parse_linear_id("ENG-123").unwrap(), "ENG-123");
    }

    #[test]
    fn parses_issue_url() {
        assert_eq!(
            parse_linear_id("https://linear.app/acme/issue/ENG-123/fix-the-thing").unwrap(),
            "ENG-123"
        );
    }

    #[test]
    fn rejects_url_without_issue_segment() {
        assert!(parse_linear_id("https://linear.app/acme/team/ENG").is_err());
    }
}
