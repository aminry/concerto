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

use async_trait::async_trait;
use concerto_error::Result;

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
