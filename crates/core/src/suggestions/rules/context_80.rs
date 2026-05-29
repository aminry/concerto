//! `context_window_80` rule (Task 40).
//!
//! Fires when an [`AgentEvent::ContextUsage`] with `pct >= 80` arrives.
//! Out-ranks `context_window_50` so the user sees the more urgent
//! suggestion when both apply.

use crate::agent_supervisor::AgentEvent;
use crate::suggestions::chip::{Chip, ChipAction};
use crate::suggestions::rules::SuggestionRule;
use crate::suggestions::state::WorkareaState;
use concerto_persist::WorkareaId;

const RULE_ID: &str = "context_window_80";
const THRESHOLD_PCT: u8 = 80;

pub struct Context80Rule;

impl SuggestionRule for Context80Rule {
    fn id(&self) -> &'static str {
        RULE_ID
    }

    fn priority(&self) -> i32 {
        80
    }

    fn applies(
        &self,
        workarea_id: &WorkareaId,
        _state: &WorkareaState,
        event: &AgentEvent,
    ) -> Option<Chip> {
        match event {
            AgentEvent::ContextUsage { pct, .. } if *pct >= THRESHOLD_PCT => Some(Chip {
                rule_id: RULE_ID.to_string(),
                workarea_id: workarea_id.clone(),
                title: "Start new session with a summary".to_string(),
                priority: 80,
                created_at: now_unix_ms(),
                action: ChipAction::NewSession,
            }),
            _ => None,
        }
    }
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
