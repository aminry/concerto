//! Claude Code parser pack — V0.1 terminal-mode regex (Task 33).
//!
//! Watches the agent's rendered stdout for the tool-approval menu
//! pattern Claude Code prints when it wants permission to perform an
//! `edit` / `write` / `apply_patch`. The exact wording is version-tied
//! and intentionally fragile per `design/04 §3.2`; V1.0 swaps this for
//! a structured parser. Until then, the regex is the line of defence.
//!
//! ## V0.1 detection patterns
//!
//! The reference menu (see
//! `crates/core/tests/fixtures/claude_code/approval_v1.txt` for the
//! synthetic capture this pack is tuned against):
//!
//! ```text
//! ╭─ Edit file ─────────────────────────────────╮
//! │ tool: edit                                  │
//! │ path: src/foo.rs                            │
//! ╰─────────────────────────────────────────────╯
//! Do you want to make this edit? (y/n/a) >
//! ```
//!
//! The pack treats the trailing `Do you want … ?` line as the boundary
//! marker. The detected tool name + a short summary are pulled from the
//! header / body — the regex is best-effort but always falls back to
//! `"edit"` + the prompt line as the summary.
//!
//! ## Injection bytes (V0.1 menu)
//!
//! - [`Decision::AutoApprove`] → `"y\n"`.
//! - [`Decision::AutoApproveOnce`] → `"2\n"` (the "approve once" menu
//!   slot — Claude Code's "Yes, and don't ask again this session"
//!   variant is `"3"`, so V0.1 picks the safe middle option).
//! - [`Decision::AutoDeny`] → `"n\n"`.
//! - [`Decision::MustAsk`] is not a terminal verdict; the supervisor
//!   never asks the pack to inject in this case.

use regex::Regex;
use std::sync::OnceLock;

use crate::agent_supervisor::parsers::{MsgRole, ParseEvent, ParserPack};
use crate::agent_supervisor::AgentKind;
use crate::security::Decision;

/// Stateless V0.1 pack — the regex is a static `OnceLock<Regex>` so
/// construction is free.
#[derive(Debug, Default, Clone)]
pub struct ClaudeCodePack;

impl ClaudeCodePack {
    pub fn new() -> Self {
        Self
    }
}

/// Compiled prompt regex. The pattern matches the trailing
/// `Do you want to make this edit?` line with a small allow-list of
/// per-tool variants (`change`, `edit`, `write`, `apply this patch`).
fn approval_regex() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        // Case-insensitive: Claude Code occasionally renders the menu
        // in title case. The trailing `>` cursor is optional because
        // the byte stream may arrive before the terminal redraws the
        // prompt.
        Regex::new(
            r"(?i)Do you want to (make this (edit|change|write)|apply this patch)\??\s*\(.*?\)\s*>?",
        )
        .expect("static regex compiles")
    })
}

/// Pull the tool name from the menu's "tool:" line. Falls back to
/// `"edit"` when the body cannot be parsed (terminal mode is lossy by
/// design).
fn extract_tool(haystack: &str) -> String {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| Regex::new(r"(?im)^.*tool:\s*(\w+)").expect("tool regex"));
    re.captures(haystack)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "edit".to_string())
}

/// Pull the operation's path from the menu's "path:" line. Best-effort.
fn extract_path(haystack: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| Regex::new(r"(?im)^.*path:\s*(\S+)").expect("path regex"));
    re.captures(haystack)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

impl ParserPack for ClaudeCodePack {
    fn agent_kind(&self) -> AgentKind {
        AgentKind::Claude
    }

    fn version_pattern(&self) -> &str {
        // Claude Code prints something like `claude-code 0.x.y` — V0.1
        // does not branch on the version, but document the pattern for
        // the V1.0 registry.
        r"claude(-code)?\s+\d+\.\d+\.\d+"
    }

    fn parse_chunk(&self, buf: &mut Vec<u8>) -> Vec<ParseEvent> {
        if buf.is_empty() {
            return Vec::new();
        }
        // Drain the whole buffer — V0.1 doesn't try to be clever about
        // partial-line accumulation, the regex either matches in the
        // current chunk or not. Terminal-mode flakiness is documented
        // as expected debt.
        let bytes = std::mem::take(buf);
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let mut events = Vec::with_capacity(2);
        events.push(ParseEvent::Bytes(bytes));

        let regex = approval_regex();
        if let Some(m) = regex.find(&text) {
            let tool = extract_tool(&text);
            let path = extract_path(&text);
            // The prompt line itself is the human-readable summary.
            let summary = m.as_str().trim().to_string();
            let payload = match path.as_deref() {
                Some(p) => serde_json::json!({ "path": p, "raw": text.clone() }),
                None => serde_json::json!({ "raw": text.clone() }),
            };
            events.push(ParseEvent::AwaitingApproval {
                tool,
                summary,
                payload,
            });
        } else {
            // No gate detected — surface as a regular assistant message.
            events.push(ParseEvent::Message {
                role: MsgRole::Assistant,
                content: text,
            });
        }
        events
    }

    fn inject_approval(&self, decision: Decision) -> Vec<u8> {
        // V0.1 menu mapping. `MustAsk` is not a terminal verdict so the
        // supervisor never calls into this path with it; return empty
        // defensively rather than panic.
        match decision {
            Decision::AutoApprove => b"y\n".to_vec(),
            Decision::AutoApproveOnce => b"2\n".to_vec(),
            Decision::AutoDeny => b"n\n".to_vec(),
            Decision::MustAsk => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/claude_code/approval_v1.txt");

    #[test]
    fn fixture_triggers_awaiting_approval() {
        let pack = ClaudeCodePack::new();
        let mut buf = FIXTURE.as_bytes().to_vec();
        let events = pack.parse_chunk(&mut buf);
        let has_gate = events
            .iter()
            .any(|e| matches!(e, ParseEvent::AwaitingApproval { .. }));
        assert!(
            has_gate,
            "fixture must trigger AwaitingApproval, got {events:?}"
        );
        // Verify the tool extraction picked up the menu's tool: line.
        let tool = events.iter().find_map(|e| match e {
            ParseEvent::AwaitingApproval { tool, .. } => Some(tool.clone()),
            _ => None,
        });
        assert_eq!(tool.as_deref(), Some("edit"));
    }

    #[test]
    fn plain_message_does_not_trigger_gate() {
        let pack = ClaudeCodePack::new();
        let mut buf = b"hello there, just a regular message".to_vec();
        let events = pack.parse_chunk(&mut buf);
        let has_gate = events
            .iter()
            .any(|e| matches!(e, ParseEvent::AwaitingApproval { .. }));
        assert!(!has_gate);
        let has_message = events
            .iter()
            .any(|e| matches!(e, ParseEvent::Message { .. }));
        assert!(has_message);
    }

    #[test]
    fn inject_approval_menu_bytes() {
        let pack = ClaudeCodePack::new();
        assert_eq!(pack.inject_approval(Decision::AutoApprove), b"y\n".to_vec());
        assert_eq!(
            pack.inject_approval(Decision::AutoApproveOnce),
            b"2\n".to_vec()
        );
        assert_eq!(pack.inject_approval(Decision::AutoDeny), b"n\n".to_vec());
        assert!(pack.inject_approval(Decision::MustAsk).is_empty());
    }
}
