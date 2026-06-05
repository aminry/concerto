//! The three-layer sparse-cone-defaults inheritance resolver (Task 302,
//! `design/02 §3.2`, `design/03 §3.2`/§12 R-2, `PHASE3_PLANNING §2`/§4).
//!
//! A new workarea's per-(workarea, repo) cone set is resolved from three
//! layers, **most-specific wins**:
//!
//! 1. **workarea** — `workarea_repos.sparse_cones_json` (a flat
//!    `["<cone_path>", …]` JSON array). The user's explicit per-(workarea,
//!    repo) override.
//! 2. **workspace-default** — `workspaces.settings_json.cone_defaults`, a
//!    `{ "<repository_id>": ["<cone_path>", …] }` map (the FROZEN nested
//!    shape — NO dedicated column). Looked up by `repository_id`.
//! 3. **repo-default** — `repositories.cone_defaults_json` (a flat
//!    `["<cone_path>", …]` JSON array).
//!
//! Precedence: **workarea > workspace-default > repo-default.** "Present"
//! means the layer's JSON parses to a `Some(Vec<…>)` — an *explicit empty
//! array* at a more-specific layer DOES win over a more-general non-empty
//! layer (an empty cone is a legitimate choice: "just the top-level
//! files"). Only a layer that is absent / unparsable / not-an-array is
//! skipped. When all three layers are absent the result is an empty cone
//! set.
//!
//! This module is a **pure function over three JSON strings** — no IO, no
//! DB, no git — so it is exhaustively table-testable. The Core's
//! `RepoManager` reads the three raw JSON strings from persistence and the
//! workarea-create path (306/307) calls this to seed the resolved cone.

/// A cone path. Matches `concerto_gix_wrap::ConePath` (= `String`); kept
/// as `String` here so this module has no gix-wrap dependency.
pub type ConePath = String;

/// Resolve the effective cone set for a `(workarea, repo)` from the three
/// inheritance layers (Task 302).
///
/// Arguments are the **raw JSON strings** as stored:
/// - `repo_cone_defaults_json` — `repositories.cone_defaults_json` (flat
///   array).
/// - `workspace_settings_json` — `workspaces.settings_json` (object; the
///   `cone_defaults[repository_id]` sub-key is the workspace layer).
/// - `workarea_sparse_cones_json` — `workarea_repos.sparse_cones_json`
///   (flat array).
/// - `repository_id` — the key into the workspace `cone_defaults` map.
///
/// Returns the most-specific present layer's cone set (see the module
/// doc for "present"). Never errors — an unparsable layer is treated as
/// absent so a malformed `settings_json` can never break workarea
/// creation; it simply falls through to the next layer.
pub fn resolve_cones(
    repo_cone_defaults_json: &str,
    workspace_settings_json: &str,
    workarea_sparse_cones_json: &str,
    repository_id: &str,
) -> Vec<ConePath> {
    // 1. workarea layer (most specific).
    if let Some(cones) = parse_flat_array(workarea_sparse_cones_json) {
        return cones;
    }
    // 2. workspace-default layer: settings_json.cone_defaults[repo_id].
    if let Some(cones) = parse_workspace_cone_default(workspace_settings_json, repository_id) {
        return cones;
    }
    // 3. repo-default layer (least specific).
    if let Some(cones) = parse_flat_array(repo_cone_defaults_json) {
        return cones;
    }
    // All layers absent.
    Vec::new()
}

/// Parse a flat `["<cone_path>", …]` JSON array. Returns `None` when the
/// input is not a JSON array of strings (absent / malformed / wrong type)
/// — the caller treats `None` as "layer not present" and falls through.
/// An explicit `[]` parses to `Some(vec![])` (a present, empty cone).
fn parse_flat_array(json: &str) -> Option<Vec<ConePath>> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let arr = value.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        // A non-string element makes the whole layer malformed → skip the
        // layer rather than silently dropping the bad element.
        out.push(item.as_str()?.to_string());
    }
    Some(out)
}

/// Extract `settings_json.cone_defaults[repository_id]` as a flat cone
/// array. Returns `None` when `settings_json` is not an object, has no
/// `cone_defaults` object, lacks the `repository_id` key, or the value is
/// not an array of strings.
fn parse_workspace_cone_default(settings_json: &str, repository_id: &str) -> Option<Vec<ConePath>> {
    let value: serde_json::Value = serde_json::from_str(settings_json).ok()?;
    let cone_defaults = value.get("cone_defaults")?.as_object()?;
    let per_repo = cone_defaults.get(repository_id)?;
    let arr = per_repo.as_array()?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        out.push(item.as_str()?.to_string());
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPO_ID: &str = "repo-123";

    #[test]
    fn all_layers_absent_yields_empty() {
        // Empty arrays / empty object — but note `[]` is a *present* empty
        // layer per the precedence rule, so use the "missing" forms here.
        assert_eq!(
            resolve_cones("[]", "{}", "[]", REPO_ID),
            Vec::<String>::new()
        );
    }

    #[test]
    fn repo_default_wins_when_only_layer() {
        // workarea + workspace absent (empty array / no key), repo present.
        // Workarea `[]` would win, so to test the repo layer we make the
        // workarea/workspace layers ABSENT (unparsable / missing key), not
        // empty.
        let repo = r#"["packages/core"]"#;
        let ws = r#"{"other_key": true}"#; // no cone_defaults
        let wa = "not json"; // unparsable → absent
        assert_eq!(
            resolve_cones(repo, ws, wa, REPO_ID),
            vec!["packages/core".to_string()]
        );
    }

    #[test]
    fn workspace_default_wins_over_repo_default() {
        let repo = r#"["packages/core"]"#;
        let ws = r#"{"cone_defaults": {"repo-123": ["apps/web"]}}"#;
        let wa = "not json"; // workarea absent
        assert_eq!(
            resolve_cones(repo, ws, wa, REPO_ID),
            vec!["apps/web".to_string()]
        );
    }

    #[test]
    fn workarea_wins_over_workspace_and_repo() {
        let repo = r#"["packages/core"]"#;
        let ws = r#"{"cone_defaults": {"repo-123": ["apps/web"]}}"#;
        let wa = r#"["services/api", "libs/shared"]"#;
        assert_eq!(
            resolve_cones(repo, ws, wa, REPO_ID),
            vec!["services/api".to_string(), "libs/shared".to_string()]
        );
    }

    #[test]
    fn explicit_empty_workarea_overrides_more_general_layers() {
        // An explicit `[]` at the workarea layer is a *present* empty cone
        // and beats a non-empty workspace/repo default.
        let repo = r#"["packages/core"]"#;
        let ws = r#"{"cone_defaults": {"repo-123": ["apps/web"]}}"#;
        let wa = "[]";
        assert_eq!(resolve_cones(repo, ws, wa, REPO_ID), Vec::<String>::new());
    }

    #[test]
    fn workspace_default_keyed_by_repository_id() {
        // The cone_defaults map is keyed by repository_id; a different
        // repo's entry must NOT leak.
        let repo = "[]"; // repo present but empty
        let ws = r#"{"cone_defaults": {"other-repo": ["x"], "repo-123": ["y"]}}"#;
        let wa = "not json";
        assert_eq!(resolve_cones(repo, ws, wa, REPO_ID), vec!["y".to_string()]);
    }

    #[test]
    fn malformed_workarea_falls_through_to_workspace() {
        let repo = r#"["packages/core"]"#;
        let ws = r#"{"cone_defaults": {"repo-123": ["apps/web"]}}"#;
        // A JSON object (not an array) at the workarea layer is malformed →
        // skip it, fall through to workspace.
        let wa = r#"{"oops": true}"#;
        assert_eq!(
            resolve_cones(repo, ws, wa, REPO_ID),
            vec!["apps/web".to_string()]
        );
    }

    #[test]
    fn non_string_array_element_skips_layer() {
        // `[1, 2]` is an array but not of strings → layer malformed, skip.
        let repo = r#"["packages/core"]"#;
        let ws = "{}";
        let wa = "[1, 2]";
        assert_eq!(
            resolve_cones(repo, ws, wa, REPO_ID),
            vec!["packages/core".to_string()]
        );
    }
}
