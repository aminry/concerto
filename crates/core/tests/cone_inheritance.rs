//! Integration coverage for the Task 302 three-layer sparse-cone-defaults
//! inheritance resolver (`concerto_core::repo_manager::cones::resolve_cones`).
//!
//! The resolver is a pure function over three raw JSON strings (repository
//! `cone_defaults_json` → workspace `settings_json.cone_defaults[repo_id]` →
//! workarea `sparse_cones_json`), most-specific layer wins. This table test
//! pins the FROZEN precedence + the FROZEN nested
//! `settings_json.cone_defaults` shape (`PHASE3_PLANNING §2`/§4) from the
//! Core crate's public surface — `cones.rs` carries finer-grained unit tests,
//! this asserts the contract the way 305/306/307/322 will consume it.

use concerto_core::repo_manager::cones::resolve_cones;

const REPO: &str = "repo-123";

/// Each layer wins in turn as it becomes the most-specific *present* layer.
#[test]
fn three_layer_precedence_table() {
    // (repo_defaults, workspace_settings, workarea_cones, expected)
    let cases: &[(&str, &str, &str, Vec<&str>)] = &[
        // All absent → empty.
        ("[]", "{}", "[]", vec![]),
        // Only repo-default present (workarea/workspace absent, not empty).
        (
            r#"["packages/core"]"#,
            "{}",
            "not-json",
            vec!["packages/core"],
        ),
        // Workspace-default beats repo-default.
        (
            r#"["packages/core"]"#,
            r#"{"cone_defaults":{"repo-123":["apps/web"]}}"#,
            "not-json",
            vec!["apps/web"],
        ),
        // Workarea beats both.
        (
            r#"["packages/core"]"#,
            r#"{"cone_defaults":{"repo-123":["apps/web"]}}"#,
            r#"["services/api","libs/shared"]"#,
            vec!["services/api", "libs/shared"],
        ),
        // Explicit empty workarea is a *present* empty cone → wins.
        (
            r#"["packages/core"]"#,
            r#"{"cone_defaults":{"repo-123":["apps/web"]}}"#,
            "[]",
            vec![],
        ),
        // Workspace map keyed by repository_id: a different repo's entry must
        // not leak.
        (
            "[]",
            r#"{"cone_defaults":{"other":["x"],"repo-123":["y"]}}"#,
            "not-json",
            vec!["y"],
        ),
    ];

    for (i, (repo, ws, wa, expected)) in cases.iter().enumerate() {
        let got = resolve_cones(repo, ws, wa, REPO);
        let want: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        assert_eq!(got, want, "case {i}: repo={repo:?} ws={ws:?} wa={wa:?}");
    }
}

/// A malformed more-specific layer (not an array / not strings) is treated as
/// absent and falls through — a bad `settings_json` can never break resolution.
#[test]
fn malformed_layers_fall_through() {
    // Object (not array) at the workarea layer → skip → workspace wins.
    assert_eq!(
        resolve_cones(
            r#"["repo"]"#,
            r#"{"cone_defaults":{"repo-123":["ws"]}}"#,
            r#"{"oops":true}"#,
            REPO,
        ),
        vec!["ws".to_string()],
    );
    // Array of non-strings at the workarea layer → skip → repo wins.
    assert_eq!(
        resolve_cones(r#"["repo"]"#, "{}", "[1,2,3]", REPO),
        vec!["repo".to_string()],
    );
    // Entirely unparsable everywhere → empty.
    assert_eq!(
        resolve_cones("not-json", "not-json", "not-json", REPO),
        Vec::<String>::new(),
    );
}
