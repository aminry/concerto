//! `awaiting_approval` rule (Task 40).
//!
//! Fires on [`AgentEvent::AwaitingApproval`].

use crate::agent_supervisor::AgentEvent;
use crate::suggestions::chip::{Chip, ChipAction};
use crate::suggestions::rules::SuggestionRule;
use crate::suggestions::state::WorkareaState;
use concerto_persist::WorkareaId;

const RULE_ID: &str = "awaiting_approval";

pub struct ReviewToolRule;

impl SuggestionRule for ReviewToolRule {
    fn id(&self) -> &'static str {
        RULE_ID
    }

    fn priority(&self) -> i32 {
        90
    }

    fn applies(
        &self,
        workarea_id: &WorkareaId,
        _state: &WorkareaState,
        event: &AgentEvent,
    ) -> Option<Chip> {
        if matches!(event, AgentEvent::AwaitingApproval { .. }) {
            Some(Chip {
                rule_id: RULE_ID.to_string(),
                workarea_id: workarea_id.clone(),
                title: "Review tool call".to_string(),
                priority: 90,
                created_at: now_unix_ms(),
                action: ChipAction::ReviewTool,
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
