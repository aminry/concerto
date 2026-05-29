//! `context_window_50` rule (Task 40).
//!
//! Fires when an [`AgentEvent::ContextUsage`] with `pct >= 50` arrives.
//! V0.1 parser packs do not yet emit `ContextUsage`, so the rule is
//! latent until a parser pack starts surfacing the signal.

use crate::agent_supervisor::AgentEvent;
use crate::suggestions::chip::{Chip, ChipAction};
use crate::suggestions::rules::SuggestionRule;
use crate::suggestions::state::WorkareaState;
use concerto_persist::WorkareaId;

const RULE_ID: &str = "context_window_50";
const THRESHOLD_PCT: u8 = 50;

pub struct Context50Rule;

impl SuggestionRule for Context50Rule {
    fn id(&self) -> &'static str {
        RULE_ID
    }

    fn priority(&self) -> i32 {
        40
    }

    fn applies(
        &self,
        workarea_id: &WorkareaId,
        _state: &WorkareaState,
        event: &AgentEvent,
    ) -> Option<Chip> {
        match event {
            AgentEvent::ContextUsage { pct, .. } if *pct >= THRESHOLD_PCT && *pct < 80 => {
                Some(Chip {
                    rule_id: RULE_ID.to_string(),
                    workarea_id: workarea_id.clone(),
                    title: "Compress context now".to_string(),
                    priority: 40,
                    created_at: now_unix_ms(),
                    action: ChipAction::Compress,
                })
            }
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
