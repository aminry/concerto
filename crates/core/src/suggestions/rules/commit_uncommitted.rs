//! `turn_complete_with_uncommitted` rule (Task 40).
//!
//! Fires on [`AgentEvent::TurnComplete`]. The rule's `applies` only
//! returns the chip's static metadata — the actual `git status`
//! check needs the workarea's worktree path and is asynchronous, so
//! the engine performs the FS probe before publishing the chip (see
//! `actor::evaluate_event`). The rule shape stays sync so the trait
//! does not have to be `async_trait`.

use crate::agent_supervisor::AgentEvent;
use crate::suggestions::chip::{Chip, ChipAction};
use crate::suggestions::rules::SuggestionRule;
use crate::suggestions::state::WorkareaState;
use concerto_persist::WorkareaId;

pub(crate) const RULE_ID: &str = "turn_complete_with_uncommitted";

pub struct CommitUncommittedRule;

impl SuggestionRule for CommitUncommittedRule {
    fn id(&self) -> &'static str {
        RULE_ID
    }

    fn priority(&self) -> i32 {
        50
    }

    fn applies(
        &self,
        workarea_id: &WorkareaId,
        _state: &WorkareaState,
        event: &AgentEvent,
    ) -> Option<Chip> {
        // The async status probe is done in the engine — here we just
        // surface the candidate chip whenever a turn completes. The
        // engine drops the chip if `gix-wrap::status` reports a clean
        // worktree.
        if matches!(event, AgentEvent::TurnComplete { .. }) {
            Some(Chip {
                rule_id: RULE_ID.to_string(),
                workarea_id: workarea_id.clone(),
                title: "Commit and push".to_string(),
                priority: 50,
                created_at: now_unix_ms(),
                action: ChipAction::CommitAndPush,
            })
        } else {
            None
        }
    }
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
