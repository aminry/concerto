//! [`JiraClient`] — the native Jira (Atlassian Cloud) issue-fetch client
//! (Task 317, `design/13 §3.7`).
//!
//! Jira Cloud exposes a REST API on the project's Atlassian cloud base
//! (`https://<site>.atlassian.net`). Given a Jira issue key (`PROJ-45`) or a
//! `*.atlassian.net/browse/PROJ-45` URL, this client `GET`s
//! `/rest/api/3/issue/{key}` with the stored OAuth bearer token and maps the
//! fields → [`Issue`] (`summary`→title, `description` [ADF flattened to text]
//! →body, `labels`, `status.name`, `key`→`external_id`).
//!
//! ## ADF → text
//!
//! Jira Cloud returns the description as **Atlassian Document Format** (a JSON
//! node tree), not plain text. [`flatten_adf`] walks the tree and concatenates
//! every `text` leaf (joining block-level nodes with newlines); unknown node
//! types are skipped (the flattener is small + total). We do not render ADF.
//!
//! ## OAuth refresh on 401 (locked decision D6)
//!
//! Jira uses Atlassian OAuth (no personal-key path). On a `401` the client
//! attempts **one** transparent refresh via the caller-supplied
//! [`RefreshToken`] callback (which the Core implements against the keychain's
//! `VcsSecretSlot::JiraRefreshToken` + Atlassian's token endpoint, re-storing
//! the new access token), then retries the GET once. A second `401` is a hard
//! [`Error::VcsNotAuthenticated`]. The token is never logged.

use std::future::Future;
use std::pin::Pin;

use concerto_error::{Error, Result};
use concerto_keychain::SecretValue;
use serde::Deserialize;
use url::Url;

use crate::provider::Issue;

/// A one-shot OAuth-refresh callback (locked decision D6).
///
/// Returns a freshly-minted Jira access token (the Core implements this against
/// the keychain `VcsSecretSlot::JiraRefreshToken` + Atlassian's token endpoint,
/// persisting the new token before returning it). `JiraClient::fetch` invokes it
/// at most once, on a `401`, before retrying. Boxed as a `dyn Fn` returning a
/// boxed future so the client stays object-safe + free of a generic param.
pub type RefreshToken<'a> = Box<
    dyn Fn() -> Pin<Box<dyn Future<Output = Result<SecretValue>> + Send + 'a>> + Send + Sync + 'a,
>;

/// The native Jira (Atlassian Cloud) REST client.
///
/// Holds a `reqwest::Client` (rustls) + the resolved site base URI. The access
/// token is supplied per `fetch` call (read from the keychain by the caller) so
/// the client never owns secret material.
pub struct JiraClient {
    http: reqwest::Client,
    base_uri: String,
}

impl JiraClient {
    /// Build a client against the project's Atlassian cloud base
    /// (`https://<site>.atlassian.net`, or the `testkit` wiremock base).
    pub fn with_base(base_uri: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| Error::Vcs(format!("jira: build http client: {e}")))?;
        Ok(Self {
            http,
            base_uri: base_uri.trim_end_matches('/').to_string(),
        })
    }

    /// Fetch an issue by `PROJ-45` (or a `*.atlassian.net/browse/PROJ-45` URL)
    /// with `token` (an Atlassian OAuth access token). On a `401`, invokes
    /// `refresh` once (if supplied) and retries. Maps the JSON → [`Issue`]
    /// (description ADF flattened to text).
    ///
    /// `Issue.number` is `0` (Jira keys on the string key, in
    /// [`Issue::external_id`]). Returns [`Error::NotFound`] on a 404 and
    /// [`Error::VcsNotAuthenticated`] when auth fails after the one refresh.
    pub async fn fetch(
        &self,
        key_or_url: &str,
        token: &SecretValue,
        refresh: Option<&RefreshToken<'_>>,
    ) -> Result<Issue> {
        let key = parse_jira_key(key_or_url)?;
        let route = format!("{}/rest/api/3/issue/{}", self.base_uri, key);

        let status = self.get_status(&route, token.expose()).await?;
        let body = match status {
            JiraResult::Ok(body) => body,
            JiraResult::NotFound => return Err(Error::NotFound(format!("jira: no issue `{key}`"))),
            JiraResult::Unauthorized => {
                // One transparent refresh, then retry once.
                let refresh = refresh.ok_or_else(|| {
                    Error::VcsNotAuthenticated(
                        "jira: token rejected (401) and no refresh available".to_string(),
                    )
                })?;
                let fresh = refresh().await?;
                match self.get_status(&route, fresh.expose()).await? {
                    JiraResult::Ok(body) => body,
                    JiraResult::NotFound => {
                        return Err(Error::NotFound(format!("jira: no issue `{key}`")))
                    }
                    JiraResult::Unauthorized => {
                        return Err(Error::VcsNotAuthenticated(
                            "jira: token still rejected after refresh; reconnect Jira in Settings"
                                .to_string(),
                        ))
                    }
                }
            }
        };

        let issue: JiraIssue = serde_json::from_value(body)
            .map_err(|e| Error::Vcs(format!("jira: decode issue: {e}")))?;
        Ok(issue.into_issue())
    }

    /// Perform the GET and classify the response into a small status enum so the
    /// 401-refresh-retry logic stays linear.
    async fn get_status(&self, route: &str, token: &str) -> Result<JiraResult> {
        let resp = self
            .http
            .get(route)
            .bearer_auth(token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| Error::Vcs(format!("jira: request failed: {e}")))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Ok(JiraResult::Unauthorized);
        }
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(JiraResult::NotFound);
        }
        if !status.is_success() {
            return Err(Error::Vcs(format!("jira: HTTP {status}")));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| Error::Vcs(format!("jira: decode response: {e}")))?;
        Ok(JiraResult::Ok(body))
    }
}

/// Outcome of a single Jira GET, distinguishing the auth/not-found cases the
/// refresh-retry logic branches on.
enum JiraResult {
    Ok(serde_json::Value),
    NotFound,
    Unauthorized,
}

/// Parse a Jira issue key from either a bare `PROJ-45` key or a
/// `*.atlassian.net/browse/PROJ-45` URL.
pub fn parse_jira_key(key_or_url: &str) -> Result<String> {
    let trimmed = key_or_url.trim();
    if trimmed.is_empty() {
        return Err(Error::Validation("jira: empty issue key/url".to_string()));
    }
    if !trimmed.contains("://") {
        return Ok(trimmed.to_string());
    }
    let url = Url::parse(trimmed)
        .map_err(|e| Error::Validation(format!("jira: invalid URL `{trimmed}`: {e}")))?;
    // Path: /browse/<KEY>  → take the segment after `browse`.
    let segments: Vec<&str> = url.path().trim_matches('/').split('/').collect();
    if let Some(pos) = segments.iter().position(|s| *s == "browse") {
        if let Some(key) = segments.get(pos + 1) {
            if !key.is_empty() {
                return Ok((*key).to_string());
            }
        }
    }
    Err(Error::Validation(format!(
        "jira: URL `{trimmed}` has no /browse/<KEY> segment"
    )))
}

/// Flatten an Atlassian Document Format (ADF) node tree to plain text.
///
/// Walks the tree depth-first: every `text` leaf contributes its string;
/// block-level container nodes (`paragraph`, `heading`, `listItem`, …) are
/// separated by a newline so the result reads sensibly. Unknown node types are
/// traversed for their `content` but contribute no text of their own — the
/// flattener is total (never errors) and small. A `null`/absent description →
/// empty string.
pub fn flatten_adf(node: &serde_json::Value) -> String {
    let mut out = String::new();
    walk_adf(node, &mut out);
    out.trim().to_string()
}

/// Block-level ADF node types: their text is separated from the previous
/// sibling's text by a newline so paragraphs/headings/list items read on
/// separate lines. Inline nodes (`text`, marks) concatenate without separation.
fn is_block_node(node: &serde_json::Value) -> bool {
    matches!(
        node.get("type").and_then(|t| t.as_str()),
        Some(
            "paragraph"
                | "heading"
                | "listItem"
                | "bulletList"
                | "orderedList"
                | "blockquote"
                | "codeBlock"
                | "rule"
                | "panel"
        )
    )
}

fn walk_adf(node: &serde_json::Value, out: &mut String) {
    let node_type = node.get("type").and_then(|t| t.as_str());
    // A `text` node contributes its literal text.
    if node_type == Some("text") {
        if let Some(text) = node.get("text").and_then(|t| t.as_str()) {
            out.push_str(text);
        }
        return;
    }
    // Hard break → newline.
    if node_type == Some("hardBreak") {
        out.push('\n');
        return;
    }
    // Otherwise recurse into `content`. A block-level child that produces text
    // is preceded by a newline (when there is already preceding text), so
    // block siblings land on their own lines; inline children concatenate.
    if let Some(content) = node.get("content").and_then(|c| c.as_array()) {
        for child in content {
            let before = out.len();
            if is_block_node(child) && !out.is_empty() {
                out.push('\n');
            }
            let sep_len = out.len();
            walk_adf(child, out);
            // If a block child produced nothing, drop the speculative newline.
            if out.len() == sep_len && sep_len > before {
                out.truncate(before);
            }
        }
    }
}

// --- REST response projections (local) ---

#[derive(Debug, Deserialize)]
struct JiraIssue {
    #[serde(default)]
    key: String,
    #[serde(default)]
    fields: JiraFields,
}

#[derive(Debug, Default, Deserialize)]
struct JiraFields {
    #[serde(default)]
    summary: String,
    /// ADF node tree (or `null`).
    #[serde(default)]
    description: serde_json::Value,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    status: Option<JiraStatus>,
}

#[derive(Debug, Deserialize)]
struct JiraStatus {
    #[serde(default)]
    name: String,
}

impl JiraIssue {
    fn into_issue(self) -> Issue {
        let body = if self.fields.description.is_null() {
            String::new()
        } else {
            flatten_adf(&self.fields.description)
        };
        Issue {
            number: 0,
            title: self.fields.summary,
            body,
            state: self
                .fields
                .status
                .map(|s| s.name.to_lowercase())
                .unwrap_or_default(),
            url: String::new(),
            labels: self.fields.labels,
            external_id: self.key,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_key() {
        assert_eq!(parse_jira_key("PROJ-45").unwrap(), "PROJ-45");
    }

    #[test]
    fn parses_browse_url() {
        assert_eq!(
            parse_jira_key("https://acme.atlassian.net/browse/PROJ-45").unwrap(),
            "PROJ-45"
        );
    }

    #[test]
    fn flattens_adf_paragraphs() {
        let adf = serde_json::json!({
            "type": "doc",
            "content": [
                { "type": "paragraph", "content": [ { "type": "text", "text": "Hello" } ] },
                { "type": "paragraph", "content": [ { "type": "text", "text": "World" } ] }
            ]
        });
        assert_eq!(flatten_adf(&adf), "Hello\nWorld");
    }

    #[test]
    fn flattens_adf_skips_unknown_nodes() {
        let adf = serde_json::json!({
            "type": "doc",
            "content": [
                { "type": "paragraph", "content": [
                    { "type": "text", "text": "a" },
                    { "type": "mention", "attrs": { "id": "x" } },
                    { "type": "text", "text": "b" }
                ] }
            ]
        });
        assert_eq!(flatten_adf(&adf), "ab");
    }

    #[test]
    fn flattens_null_description_to_empty() {
        assert_eq!(flatten_adf(&serde_json::Value::Null), "");
    }
}
