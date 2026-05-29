//! Integration tests for Task 42 — permission modes end-to-end +
//! managed.json cap + hot reload + deny-list-still-applies-in-yolo.
//!
//! Coverage (per Task 42 pre-decision 10):
//!
//! 1. **4 modes × 3 tool classes = 12 cases for [`PermissionResolver::decide`]**
//!    — verifies the full decision matrix from `design/04 §3.10`.
//! 2. **Managed cap to `auto` blocks yolo** via
//!    [`enforce_managed_cap`].
//! 3. **`allow_yolo = false` blocks yolo** with the `policy.yolo_blocked`
//!    subcode.
//! 4. **`allow_bypass_destructive_guard = false` blocks bypass** with
//!    the `policy.bypass_blocked` subcode.
//! 5. **Hot reload**: write `managed.json` with `allow_yolo = true`;
//!    subscribe to the [`ManagedPolicySource`]; write the file again
//!    with `allow_yolo = false`; assert the receiver observes the
//!    change within 2 s (debounce = 500 ms + filesystem-event latency).
//! 6. **Deny-list still applies in yolo**: even with mode = yolo and
//!    `bypass = true`, a path inside the deny-list classifies as
//!    [`PolicyVerdict::Denied`] via
//!    [`classify_policy_for_path`].

#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use concerto_core::agent_supervisor::approval::{classify_policy_for_path, PolicyVerdict};
use concerto_core::security::permission::{
    enforce_managed_bypass, enforce_managed_cap, POLICY_BYPASS_BLOCKED, POLICY_YOLO_BLOCKED,
};
use concerto_core::security::{
    AllowList, Decision, DenyList, ManagedPolicy, ManagedPolicySource, PermissionMode,
    PermissionResolver,
};
use concerto_error::Error;
use tempfile::TempDir;

// -------- 1. Full mode × class matrix (12 cases) -----------------------

/// One row of the decision matrix. Each case names a representative
/// tool that the [`tool_classes`] table maps to the listed class.
struct MatrixCase {
    mode: PermissionMode,
    tool: &'static str,
    expected: Decision,
    /// Set when the case requires `bypass_destructive_guard = true` to
    /// match `expected`. V0.1 only `Yolo + Dangerous` flips on this.
    bypass: bool,
}

fn matrix() -> Vec<MatrixCase> {
    use Decision::*;
    use PermissionMode::*;
    vec![
        // Strict — every class asks.
        MatrixCase {
            mode: Strict,
            tool: "Read",
            expected: MustAsk,
            bypass: false,
        },
        MatrixCase {
            mode: Strict,
            tool: "Write",
            expected: MustAsk,
            bypass: false,
        },
        MatrixCase {
            mode: Strict,
            tool: "Delete",
            expected: MustAsk,
            bypass: false,
        },
        // Normal — Safe auto-approves; Restricted + Dangerous ask.
        MatrixCase {
            mode: Normal,
            tool: "Read",
            expected: AutoApprove,
            bypass: false,
        },
        MatrixCase {
            mode: Normal,
            tool: "Write",
            expected: MustAsk,
            bypass: false,
        },
        MatrixCase {
            mode: Normal,
            tool: "Delete",
            expected: MustAsk,
            bypass: false,
        },
        // Auto — Safe + Restricted auto-approve; Dangerous asks.
        MatrixCase {
            mode: Auto,
            tool: "Read",
            expected: AutoApprove,
            bypass: false,
        },
        MatrixCase {
            mode: Auto,
            tool: "Write",
            expected: AutoApprove,
            bypass: false,
        },
        MatrixCase {
            mode: Auto,
            tool: "Delete",
            expected: MustAsk,
            bypass: false,
        },
        // Yolo — Safe + Restricted auto-approve; Dangerous auto-approves
        // ONLY when bypass = true (otherwise MustAsk).
        MatrixCase {
            mode: Yolo,
            tool: "Read",
            expected: AutoApprove,
            bypass: false,
        },
        MatrixCase {
            mode: Yolo,
            tool: "Write",
            expected: AutoApprove,
            bypass: false,
        },
        MatrixCase {
            mode: Yolo,
            tool: "Delete",
            expected: AutoApprove,
            bypass: true,
        },
    ]
}

#[test]
fn full_matrix_4_modes_x_3_classes() {
    for case in matrix() {
        let r = PermissionResolver::new(case.mode, case.bypass);
        let got = r.decide(case.tool);
        assert_eq!(
            got, case.expected,
            "mode={:?} tool={:?} bypass={} expected={:?} got={:?}",
            case.mode, case.tool, case.bypass, case.expected, got
        );
    }
}

#[test]
fn yolo_dangerous_without_bypass_must_ask() {
    // Specifically the "yolo + Dangerous + bypass=false" row — keep it
    // as its own assertion in case future refactors collapse the
    // matrix-driven test.
    let r = PermissionResolver::new(PermissionMode::Yolo, false);
    assert_eq!(r.decide("Delete"), Decision::MustAsk);
}

// -------- 2. Managed cap to auto blocks yolo ---------------------------

#[test]
fn managed_cap_auto_blocks_yolo() {
    let mp = ManagedPolicy {
        max_permission_mode: Some(PermissionMode::Auto),
        ..ManagedPolicy::default()
    };
    let err = enforce_managed_cap(PermissionMode::Yolo, &mp).unwrap_err();
    let msg = format!("{err}");
    assert!(matches!(err, Error::PolicyLocked(_)));
    assert!(
        msg.contains(POLICY_YOLO_BLOCKED),
        "yolo refusal must carry policy.yolo_blocked subcode, got: {msg}"
    );
}

// -------- 3. allow_yolo = false blocks yolo ----------------------------

#[test]
fn allow_yolo_false_blocks_yolo() {
    let mp = ManagedPolicy {
        allow_yolo: false,
        ..ManagedPolicy::default()
    };
    let err = enforce_managed_cap(PermissionMode::Yolo, &mp).unwrap_err();
    assert!(matches!(err, Error::PolicyLocked(_)));
    assert!(format!("{err}").contains(POLICY_YOLO_BLOCKED));
    // Auto is still allowed.
    assert!(enforce_managed_cap(PermissionMode::Auto, &mp).is_ok());
}

// -------- 4. allow_bypass_destructive_guard = false blocks bypass ------

#[test]
fn allow_bypass_false_blocks_bypass() {
    let mp = ManagedPolicy {
        allow_bypass_destructive_guard: false,
        ..ManagedPolicy::default()
    };
    let err = enforce_managed_bypass(true, &mp).unwrap_err();
    assert!(matches!(err, Error::PolicyLocked(_)));
    assert!(format!("{err}").contains(POLICY_BYPASS_BLOCKED));
    // Disable always succeeds.
    assert!(enforce_managed_bypass(false, &mp).is_ok());
}

// -------- 5. Hot reload via ManagedPolicySource ------------------------

/// Write `managed.json` once, build a [`ManagedPolicySource`], rewrite
/// the file with a different `allow_yolo`, then assert the watch
/// receiver observes the change within 2 seconds (debounce + FS-event
/// latency).
#[tokio::test(flavor = "multi_thread")]
async fn hot_reload_observes_managed_json_changes() {
    let d = TempDir::new().expect("tempdir");
    let path = d.path().join("managed.json");
    std::fs::write(&path, r#"{"version":1,"allow_yolo":true}"#).expect("seed write");

    let src = ManagedPolicySource::new(d.path()).expect("build source");
    let initial = src.current();
    assert!(initial.allow_yolo, "seed must parse with allow_yolo=true");

    let mut rx = src.subscribe();

    // Replace the file. Use a tempfile + rename to mimic editor save
    // semantics; that also exercises the Create-then-Modify event burst
    // the debouncer is designed to coalesce.
    let tmp = d.path().join("managed.json.tmp");
    std::fs::write(&tmp, r#"{"version":1,"allow_yolo":false}"#).expect("rewrite");
    std::fs::rename(&tmp, &path).expect("rename");

    // The watcher debounces at 500 ms, then re-parses. Allow up to 2 s
    // for the FS-event → debounce → parse path to land.
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            // `changed()` resolves on the NEXT update. The seed value
            // already passed through the channel so we can wait without
            // missing the rewrite.
            if rx.changed().await.is_err() {
                return None;
            }
            let p = rx.borrow_and_update().clone();
            if !p.allow_yolo {
                return Some(p);
            }
        }
    })
    .await;

    let observed = result
        .expect("watcher did not republish within 2s")
        .expect("watch channel closed unexpectedly");
    assert!(
        !observed.allow_yolo,
        "post-reload policy must have allow_yolo=false, got {observed:?}"
    );
}

// -------- 6. Deny-list still applies in yolo --------------------------

#[test]
fn deny_list_still_applies_in_yolo() {
    let td = TempDir::new().expect("tempdir");
    let base = concerto_core::security::path_policy::canonicalize_or_clean(td.path());
    std::fs::create_dir_all(base.join(".ssh")).expect("mkdir .ssh");

    // Simulate the "everything goes" runtime: yolo + bypass + path
    // inside the allow list. The deny-list match still wins.
    let mut allow = AllowList::new();
    allow.push(base.clone());
    let mut deny = DenyList::new();
    deny.push(base.join(".ssh"));

    // A path under .ssh classifies as Denied regardless of allow-list
    // overlap.
    let candidate = base.join(".ssh/config");
    let verdict = classify_policy_for_path(&candidate, &allow, &deny);
    assert_eq!(verdict, PolicyVerdict::Denied);

    // The resolver in yolo + bypass still says AutoApprove for the
    // mode-class case — but the dispatch path consults the policy
    // verdict FIRST and forces AutoDeny. That layering is the property
    // the supervisor relies on; here we assert the floor in isolation.
    let resolver = PermissionResolver::new(PermissionMode::Yolo, true);
    assert_eq!(resolver.decide("Write"), Decision::AutoApprove);
}

// -------- 7. Version reject ------------------------------------------

#[test]
fn future_version_managed_json_errors() {
    let d = TempDir::new().expect("tempdir");
    std::fs::write(d.path().join("managed.json"), r#"{"version": 99}"#).expect("write");
    let err = concerto_core::security::load_managed_policy(d.path()).unwrap_err();
    let msg = format!("{err}");
    assert!(matches!(err, Error::Internal(_)));
    assert!(msg.contains("unsupported version"), "got: {msg}");
}

// -------- 8. Smoke-test the path_policy::canonicalize_or_clean ---------
//
// (Sanity check that the deny-list test setup actually uses a stable
// canonical path; otherwise a symlink in $TMPDIR could mask the
// classification.)
#[test]
fn temp_paths_canonicalize_stably() {
    let d = TempDir::new().expect("tempdir");
    let a = concerto_core::security::path_policy::canonicalize_or_clean(d.path());
    let b = concerto_core::security::path_policy::canonicalize_or_clean(d.path());
    assert_eq!(a, b, "canonicalize must be deterministic");
    let _ = PathBuf::from(&a);
}
