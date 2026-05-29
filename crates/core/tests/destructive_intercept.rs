//! Integration tests for Task 43 — destructive-command intercept
//! (`crates/core/src/security/destructive.rs`).
//!
//! Coverage:
//!
//! - **Smoke set**: 20 representative dangerous commands across every
//!   pattern category match `is_destructive` and surface the expected
//!   label.
//! - **Negative cases**: benign variants of the same commands (`rm
//!   file.txt`, `git push origin main`, `docker ps`, …) do NOT match.
//! - **Resolver wiring**: a destructive command with
//!   `bypass_destructive_guard = true` resolves to `AutoApprove`; the
//!   same command in `yolo` mode without the bypass still resolves to
//!   `MustAsk`.
//!
//! The tests exercise pure-Rust state — the supervisor wiring (urgent
//! flag, `tool_approvals.urgent` column, proto event field) is covered
//! by inspection of the dispatch path; an end-to-end fake-tool-call
//! test is out of scope for V0.1 (would require a fixture parser pack,
//! see `tasks/33-tool-approval-intercept.md §"Open questions"`).

#![cfg(unix)]

use concerto_core::security::{is_destructive, Decision, PermissionMode, PermissionResolver};
use serde_json::json;

/// Helper — wrap a shell command into the standard `Bash`-style args
/// blob and match against the destructive table.
fn label_for(command: &str) -> Option<&'static str> {
    is_destructive("Bash", &json!({"command": command})).map(|m| m.label)
}

/// 20-pattern smoke set covering every category in `PATTERNS`. Per
/// `tasks/43 §"Definition of Done"`: no false negatives on this set.
#[test]
fn smoke_set_of_twenty_dangerous_commands_all_match() {
    let cases: &[(&str, &str)] = &[
        // recursive-delete (4)
        ("rm -rf node_modules", "recursive-delete"),
        ("rm -fr /tmp/cache", "recursive-delete"),
        ("rm --recursive --force build", "recursive-delete"),
        ("rm -r -f dist", "recursive-delete"),
        // force-push (3)
        ("git push --force origin main", "force-push"),
        ("git push --force-with-lease origin feature", "force-push"),
        ("git push -f origin main", "force-push"),
        // git-reset-hard (1)
        ("git reset --hard HEAD~3", "git-reset-hard"),
        // git-branch-delete (2)
        ("git branch -D feature/old", "git-branch-delete"),
        ("git tag -d v0.0.1", "git-branch-delete"),
        // sql-drop (2)
        ("DROP TABLE users", "sql-drop"),
        ("truncate table sessions", "sql-drop"),
        // kubectl-delete (1)
        ("kubectl delete pod nginx", "kubectl-delete"),
        // docker-rm (3)
        ("docker rm -f mycontainer", "docker-rm"),
        ("docker volume rm myvol", "docker-rm"),
        ("docker system prune -a -f", "docker-rm"),
        // disk-wipe (3)
        ("mkfs.ext4 /dev/sda1", "disk-wipe"),
        ("dd if=/dev/zero of=/dev/sdb bs=1M", "disk-wipe"),
        ("wipefs -a /dev/sdb", "disk-wipe"),
        // sudo (1)
        ("sudo systemctl restart nginx", "sudo"),
    ];
    assert_eq!(cases.len(), 20, "smoke set must contain exactly 20 cases");
    for (cmd, expected) in cases {
        let got = label_for(cmd);
        assert_eq!(
            got,
            Some(*expected),
            "destructive intercept missed {cmd:?}: got {got:?}, expected {expected:?}"
        );
    }
}

/// Benign variants do NOT match. False positives on the negative cases
/// would be merely annoying (one extra prompt) — but `rm file.txt` is
/// the canonical example in the task spec for "must not fire".
#[test]
fn benign_commands_do_not_match() {
    let benign = &[
        "rm file.txt",
        "rm -i prompt.txt",
        "git push origin main",
        "git reset HEAD",
        "git branch -d safe", // lowercase d = safe delete
        "docker ps",
        "docker run nginx",
        "kubectl get pods",
        "select * from users",
        "ls -la",
    ];
    for cmd in benign {
        let got = label_for(cmd);
        assert_eq!(
            got, None,
            "false positive: {cmd:?} matched {got:?} but should not"
        );
    }
}

/// `bypass_destructive_guard = true` + destructive command → `AutoApprove`.
///
/// Mirrors the dispatch-time wiring in
/// `crates/core/src/agent_supervisor/actor.rs`'s `dispatch_parse_event`:
/// once the destructive intercept fires, the resolver-time decision is
/// `AutoApprove` iff `bypass_destructive_guard()` is true.
#[test]
fn bypass_destructive_guard_auto_approves() {
    let resolver = PermissionResolver::new(PermissionMode::Yolo, /* bypass */ true);
    assert!(resolver.bypass_destructive_guard());
    let dm = is_destructive("Bash", &json!({"command": "rm -rf /tmp/x"}))
        .expect("rm -rf must trigger the intercept");
    assert_eq!(dm.label, "recursive-delete");

    // Simulate the actor's dispatch logic: destructive + bypass → AutoApprove.
    let decision = if resolver.bypass_destructive_guard() {
        Decision::AutoApprove
    } else {
        Decision::MustAsk
    };
    assert_eq!(decision, Decision::AutoApprove);
}

/// Yolo mode + `bypass_destructive_guard = false` + destructive
/// command → `MustAsk`. Yolo does NOT bypass the destructive intercept
/// on its own; the bypass flag is its own gate.
#[test]
fn yolo_without_bypass_still_asks_for_destructive() {
    let resolver = PermissionResolver::new(PermissionMode::Yolo, /* bypass */ false);
    assert!(!resolver.bypass_destructive_guard());
    let dm = is_destructive("Bash", &json!({"command": "rm -rf node_modules"}))
        .expect("rm -rf must trigger the intercept");
    assert_eq!(dm.label, "recursive-delete");

    // Resolver mode-class decision: Yolo + Bash (Restricted) →
    // AutoApprove. The intercept overrides that to MustAsk.
    let mode_class_decision = resolver.decide("Bash");
    assert_eq!(mode_class_decision, Decision::AutoApprove);

    // After intercept overlay (per actor.rs dispatch_parse_event):
    let final_decision = if resolver.bypass_destructive_guard() {
        Decision::AutoApprove
    } else {
        Decision::MustAsk
    };
    assert_eq!(final_decision, Decision::MustAsk);
}

/// Negative: `rm file.txt` is NOT destructive. The decision falls
/// through to the resolver's mode-class verdict.
#[test]
fn rm_without_flags_is_not_destructive() {
    let v = json!({"command": "rm file.txt"});
    assert_eq!(is_destructive("Bash", &v), None);
}
