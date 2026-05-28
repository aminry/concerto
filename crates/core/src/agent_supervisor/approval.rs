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

use tokio::sync::oneshot;

use crate::security::Decision;

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
