//! Tool-approval coordination (Task 33).
//!
//! Owns the in-process bookkeeping for pending approval gates: the
//! `pending_approvals` map stored on each
//! [`crate::agent_supervisor::SessionEntry`] and the helpers the
//! supervisor's read-pump uses to persist the matching
//! `tool_approvals` row and inject the resolution bytes back into the
//! agent's stdin.
//!
//! V0.1 keeps the data model minimal:
//!
//! - One `oneshot::Sender<Decision>` per pending approval. The waiter
//!   task spawned for each `MustAsk` outcome parks on the matching
//!   `oneshot::Receiver`; `Sessions.ResolveApproval` looks up the
//!   sender by `approval_id` and sends the decision through.
//! - All persistence writes happen in the supervisor's actor (so the
//!   `Arc<Persistence>` doesn't need to leak into this module).
//!
//! See `tasks/33-tool-approval-intercept.md` §"Implementation notes".

use std::path::Path;

use serde_json::Value;
use tokio::sync::oneshot;

use crate::agent_supervisor::tool_args;
use crate::security::{classify_path, AllowList, Decision, DenyList, PathDecision};

/// Per-session map of pending-approval waiters. Keyed by the
/// `tool_approvals.id` of the row inserted when the gate fired; the
/// value is the `oneshot::Sender` that
/// [`crate::agent_supervisor::AgentSupervisorHandle::resolve_approval`]
/// fires when a client sends `Sessions.ResolveApproval`.
///
/// The supervisor wraps this map in a `tokio::sync::Mutex` so the
/// read-pump task (which inserts entries) and the resolve handler
/// (which removes entries) don't race.
pub type PendingApprovals = std::collections::HashMap<String, oneshot::Sender<Decision>>;

/// Convert a [`Decision`] into the row string written to
/// `tool_approvals.decision` when the decision is *user-initiated*
/// (i.e. the resolver returned `MustAsk` and a client resolved via
/// `Sessions.ResolveApproval`). The `auto_*` row strings are returned
/// by [`crate::security::PermissionResolver::auto_decision_string`].
pub fn user_decision_string(d: Decision) -> &'static str {
    match d {
        Decision::AutoApprove => "approve",
        Decision::AutoApproveOnce => "approve_once",
        Decision::AutoDeny => "deny",
        // MustAsk is not a terminal verdict; callers should never reach
        // here. Default to "deny" so a bug surfaces as a safe refusal.
        Decision::MustAsk => "deny",
    }
}

/// `tool_approvals.decision` wire string written when the filesystem
/// deny-list (`design/12 §3.7`) forces a denial. Frozen by Task 41 —
/// distinguishes a policy-floor denial from a user `"deny"` or a
/// resolver `"auto_*"` in the audit log (Task 44).
pub const DENIED_BY_POLICY: &str = "denied_by_policy";

/// Output of [`policy_override`] — describes how the filesystem policy
/// modulates the resolver's mode-class decision for a tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyVerdict {
    /// No path was extracted, OR the path classifies as Allowed —
    /// proceed with the resolver's original decision.
    Passthrough,
    /// Path classifies as Outside — the resolver's mode-class decision
    /// still wins (per Task 41 pre-decision 9: "Outside and not in deny
    /// → fall through; mode-class table handles"). Carried as its own
    /// variant so the call-site can surface it in tracing if desired.
    Outside,
    /// Path matches the hard deny-list. The resolver MUST treat this
    /// as `AutoDeny` and persist the row with
    /// `decision = "denied_by_policy"` (see [`DENIED_BY_POLICY`]).
    Denied,
}

/// Consult the filesystem allow/deny lists for `tool` + `args` and
/// return whether the policy overrides the resolver's decision.
///
/// V0.1 path extraction (per
/// [`crate::agent_supervisor::tool_args::extract_path`]) covers the
/// Claude Code built-in tools: `Write`, `Edit`, `Read`, `Bash`.
/// Unparseable tools return [`PolicyVerdict::Passthrough`] — V0.1's
/// destructive-command intercept (Task 43) provides the second line of
/// defense.
pub fn policy_override(
    tool: &str,
    args: &Value,
    allow: &AllowList,
    deny: &DenyList,
) -> PolicyVerdict {
    let Some(path) = tool_args::extract_path(tool, args) else {
        return PolicyVerdict::Passthrough;
    };
    classify_policy_for_path(&path, allow, deny)
}

/// Like [`policy_override`] but for a path the caller already has in
/// hand (e.g. tests). Exposed so the integration test in
/// `crates/core/tests/path_policy.rs` can poke the classifier without
/// needing to materialise a tool-call payload.
pub fn classify_policy_for_path(path: &Path, allow: &AllowList, deny: &DenyList) -> PolicyVerdict {
    match classify_path(path, allow, deny) {
        PathDecision::Allowed => PolicyVerdict::Passthrough,
        PathDecision::Outside => PolicyVerdict::Outside,
        PathDecision::Denied => PolicyVerdict::Denied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn passthrough_when_no_path_extracted() {
        let allow = AllowList::new();
        let deny = DenyList::new();
        let v = policy_override("Mystery", &json!({}), &allow, &deny);
        assert_eq!(v, PolicyVerdict::Passthrough);
    }

    #[test]
    fn denied_overrides_resolver() {
        let td = TempDir::new().unwrap();
        let base = crate::security::path_policy::canonicalize_or_clean(td.path());
        std::fs::create_dir_all(base.join(".ssh")).unwrap();
        let mut deny = DenyList::new();
        deny.push(base.join(".ssh"));
        let allow = AllowList::new();
        let args = json!({"file_path": base.join(".ssh/config").to_string_lossy()});
        let v = policy_override("Write", &args, &allow, &deny);
        assert_eq!(v, PolicyVerdict::Denied);
    }

    #[test]
    fn allowed_passes_through() {
        let td = TempDir::new().unwrap();
        let base = crate::security::path_policy::canonicalize_or_clean(td.path());
        let mut allow = AllowList::new();
        allow.push(base.clone());
        let deny = DenyList::new();
        let target = base.join("subdir/file.txt");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let args = json!({"file_path": target.to_string_lossy()});
        let v = policy_override("Write", &args, &allow, &deny);
        assert_eq!(v, PolicyVerdict::Passthrough);
    }

    #[test]
    fn outside_returns_outside() {
        let td_allow = TempDir::new().unwrap();
        let td_other = TempDir::new().unwrap();
        let mut allow = AllowList::new();
        allow.push(td_allow.path().to_path_buf());
        let deny = DenyList::new();
        let args = json!({"file_path": td_other.path().join("x.txt").to_string_lossy()});
        let v = policy_override("Write", &args, &allow, &deny);
        assert_eq!(v, PolicyVerdict::Outside);
    }

    #[test]
    fn classify_policy_for_path_is_publicly_callable() {
        let mut deny = DenyList::new();
        deny.push(PathBuf::from("/etc/secret"));
        let allow = AllowList::new();
        // Use a path under a non-existent prefix so canonicalize_or_clean
        // falls back to the lexical cleaner.
        let v = classify_policy_for_path(Path::new("/etc/secret/key"), &allow, &deny);
        assert_eq!(v, PolicyVerdict::Denied);
    }
}
