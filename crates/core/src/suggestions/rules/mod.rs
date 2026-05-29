//! Built-in suggestion rules (Task 40).
//!
//! Six V0.1 rules per `design/07 §3.2`'s "V0.1 rules" row. Each rule
//! lives in its own file so future rules can be added by dropping a
//! new file in and exporting it from [`builtin_rules`]. The rule ids
//! are reserved namespace — new rules MUST use new ids.

use crate::agent_supervisor::AgentEvent;
use crate::suggestions::chip::Chip;
use crate::suggestions::state::WorkareaState;
use concerto_persist::WorkareaId;

mod agent_crashed;
pub(crate) mod commit_uncommitted;
mod context_50;
mod context_80;
mod review_tool;
mod tests_failed;

pub use agent_crashed::AgentCrashedRule;
pub use commit_uncommitted::CommitUncommittedRule;
pub use context_50::Context50Rule;
pub use context_80::Context80Rule;
pub use review_tool::ReviewToolRule;
pub use tests_failed::TestsFailedRule;

/// The contract every suggestion rule implements (Task 40, FROZEN).
///
/// `applies` is invoked by the engine on every [`AgentEvent`] the
/// rule's workarea observes; returning `Some(chip)` queues the chip
/// for emission. `id` is the stable rule identifier used in
/// `suggestion_learn` and on the wire; `priority` decides ordering
/// when multiple chips race for the same slot.
pub trait SuggestionRule: Send + Sync {
    /// Stable, static rule id. Frozen for the six V0.1 rules.
    fn id(&self) -> &'static str;

    /// Higher wins when ranking chips. V0.1 rules use 1..=100.
    fn priority(&self) -> i32;

    /// Decide whether `event` (in the context of the workarea's
    /// summarised `state`) warrants emitting a chip. Returning `None`
    /// is the common case.
    fn applies(
        &self,
        workarea_id: &WorkareaId,
        state: &WorkareaState,
        event: &AgentEvent,
    ) -> Option<Chip>;
}

/// Construct the six V0.1 built-in rules. Order doesn't matter — the
/// engine evaluates every rule on every event and uses each rule's
/// `priority()` for ordering.
pub fn builtin_rules() -> Vec<Box<dyn SuggestionRule>> {
    vec![
        Box::new(Context50Rule),
        Box::new(Context80Rule),
        Box::new(TestsFailedRule::new()),
        Box::new(CommitUncommittedRule),
        Box::new(ReviewToolRule),
        Box::new(AgentCrashedRule),
    ]
}
