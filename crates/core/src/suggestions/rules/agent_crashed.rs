//! `agent_crashed` rule (Task 40).
//!
//! Fires on [`AgentEvent::Crashed`]. The Agent Supervisor's adoption
//! sweep marks the session row `'crashed'` and emits the event — the
//! rule surfaces a chip nudging the user to resume or start fresh.

use crate::agent_supervisor::AgentEvent;
use crate::suggestions::chip::{Chip, ChipAction};
use crate::suggestions::rules::SuggestionRule;
use crate::suggestions::state::WorkareaState;
use concerto_persist::WorkareaId;

const RULE_ID: &str = "agent_crashed";

pub struct AgentCrashedRule;

impl SuggestionRule for AgentCrashedRule {
    fn id(&self) -> &'static str {
        RULE_ID
    }

    fn priority(&self) -> i32 {
        100
    }

    fn applies(
        &self,
        workarea_id: &WorkareaId,
        _state: &WorkareaState,
        event: &AgentEvent,
    ) -> Option<Chip> {
        if matches!(event, AgentEvent::Crashed { .. }) {
            Some(Chip {
                rule_id: RULE_ID.to_string(),
                workarea_id: workarea_id.clone(),
                title: "Resume agent".to_string(),
                priority: 100,
                created_at: now_unix_ms(),
                action: ChipAction::Resume,
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
