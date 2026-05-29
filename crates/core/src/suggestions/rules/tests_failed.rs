//! `tests_failed` rule (Task 40).
//!
//! Scans the workarea's recent message buffer for the pattern
//! `(?i)\d+ (test|spec) failed`. When a fresh
//! [`AgentEvent::Message`] arrives the engine has already appended the
//! content to [`WorkareaState::last_message_content`]; this rule
//! re-checks the buffer rather than the single event so multi-chunk
//! agent output (each chunk a `Message`) still matches when the
//! pattern spans chunks.
//!
//! The regex is compiled once per process — `regex::Regex::new` is
//! cheap but doing it on every event would be wasteful.

use std::sync::OnceLock;

use regex::Regex;

use crate::agent_supervisor::AgentEvent;
use crate::suggestions::chip::{Chip, ChipAction};
use crate::suggestions::rules::SuggestionRule;
use crate::suggestions::state::WorkareaState;
use concerto_persist::WorkareaId;

const RULE_ID: &str = "tests_failed";

pub struct TestsFailedRule {
    pattern: &'static Regex,
}

impl TestsFailedRule {
    pub fn new() -> Self {
        Self { pattern: pattern() }
    }
}

impl Default for TestsFailedRule {
    fn default() -> Self {
        Self::new()
    }
}

fn pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Task 40 pre-decision 10 specifies `(?i)\d+ (test|spec) failed`.
    // Tolerate the common plural ("3 tests failed", "12 specs failed")
    // with `s?` — the pre-decision is satisfied because a singular form
    // still matches the documented pattern.
    RE.get_or_init(|| {
        Regex::new(r"(?i)\d+ (test|spec)s? failed").expect("tests_failed regex compiles")
    })
}

impl SuggestionRule for TestsFailedRule {
    fn id(&self) -> &'static str {
        RULE_ID
    }

    fn priority(&self) -> i32 {
        60
    }

    fn applies(
        &self,
        workarea_id: &WorkareaId,
        state: &WorkareaState,
        event: &AgentEvent,
    ) -> Option<Chip> {
        // Only re-check on `Message` events — every other event leaves
        // the message buffer unchanged so re-running the regex would
        // produce duplicates (filtered out anyway by the engine's
        // dedup, but cheaper to short-circuit here).
        if !matches!(event, AgentEvent::Message { .. }) {
            return None;
        }
        if self.pattern.is_match(&state.last_message_content) {
            Some(Chip {
                rule_id: RULE_ID.to_string(),
                workarea_id: workarea_id.clone(),
                title: "Investigate test failure".to_string(),
                priority: 60,
                created_at: now_unix_ms(),
                action: ChipAction::OpenTestFailure,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_matches_expected_phrases() {
        let re = pattern();
        assert!(re.is_match("3 tests failed"));
        assert!(re.is_match("12 specs failed"));
        assert!(re.is_match("1 TEST FAILED"));
        assert!(!re.is_match("all tests passed"));
        assert!(!re.is_match("test failure"));
    }
}
