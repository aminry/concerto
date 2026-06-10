//! Maestro parser pack — structured / no-op pass-through (Task 402,
//! PHASE4_PLANNING §4.8, design/08 §3.1).
//!
//! Used by `agent_kind = Maestro`. The Maestro's structured output — its
//! tool calls — does **not** ride the PTY scrape: it travels over the
//! in-process `concerto-maestro-mcp` channel the CLI dials via
//! `--mcp-config` (Task 401). Reusing the fragile [`ClaudeCodePack`] regex
//! scraper here would double-fire / mis-parse those MCP tool calls against
//! the terminal stream, so the Maestro gets a deliberately inert pack.
//!
//! [`ClaudeCodePack`]: crate::agent_supervisor::parsers::claude_code::ClaudeCodePack
//!
//! Behaviour mirrors [`EchoPack`]: every chunk is surfaced raw on the
//! `session.io.<sid>` subject (so the terminal view still shows the CLI's
//! rendering) plus one assistant [`ParseEvent::Message`], and the pack
//! never raises [`ParseEvent::AwaitingApproval`] — the Maestro's tool
//! gating is enforced by the [`PermissionResolver`] over the MCP tool
//! names (the strict + `ReadOnly` matrix this task freezes), not by
//! scraping a terminal approval menu.
//!
//! [`EchoPack`]: crate::agent_supervisor::parsers::echo::EchoPack
//! [`PermissionResolver`]: crate::security::PermissionResolver

use crate::agent_supervisor::parsers::{MsgRole, ParseEvent, ParserPack};
use crate::agent_supervisor::AgentKind;
use crate::security::Decision;

/// Stateless Maestro pack — a structured/no-op pass-through (Task 402).
///
/// Distinct from [`EchoPack`](crate::agent_supervisor::parsers::echo::EchoPack)
/// only in [`ParserPack::agent_kind`] (so the supervisor's pack→kind
/// round-trip is honest) and in intent: the Maestro's real tool-call
/// channel is MCP, not the PTY scrape, so this pack stays inert on the
/// terminal stream.
#[derive(Debug, Default, Clone)]
pub struct MaestroPack;

impl MaestroPack {
    pub fn new() -> Self {
        Self
    }
}

impl ParserPack for MaestroPack {
    fn agent_kind(&self) -> AgentKind {
        AgentKind::Maestro
    }

    fn version_pattern(&self) -> &str {
        // The Maestro spawns one of the agent CLIs (Claude live in 402;
        // Codex/Gemini in 412) selected by the provider seam, so there is
        // no single stable `--version` string. The supervisor does not run
        // version detection in the spawn path; this is a documentation
        // hint only.
        r"(claude|codex|gemini)"
    }

    fn parse_chunk(&self, buf: &mut Vec<u8>) -> Vec<ParseEvent> {
        if buf.is_empty() {
            return Vec::new();
        }
        // Drain the whole buffer: surface it raw (terminal view) and as a
        // single assistant message. Tool calls do NOT come through here —
        // they ride the MCP channel — so we never emit `ToolCall` /
        // `AwaitingApproval` from the scrape.
        let bytes = std::mem::take(buf);
        let content = String::from_utf8_lossy(&bytes).into_owned();
        vec![
            ParseEvent::Bytes(bytes),
            ParseEvent::Message {
                role: MsgRole::Assistant,
                content,
            },
        ]
    }

    fn inject_approval(&self, _decision: Decision) -> Vec<u8> {
        // The Maestro never raises a PTY-scraped AwaitingApproval (its tool
        // gating is MCP + the PermissionResolver), so this is a no-op
        // rather than a panic on misuse.
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_kind_is_maestro() {
        assert_eq!(MaestroPack::new().agent_kind(), AgentKind::Maestro);
    }

    #[test]
    fn parse_chunk_emits_bytes_and_message_never_awaiting_approval() {
        let pack = MaestroPack::new();
        let mut buf = b"hello from maestro".to_vec();
        let events = pack.parse_chunk(&mut buf);
        assert!(buf.is_empty(), "buffer is fully drained");
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], ParseEvent::Bytes(_)));
        assert!(matches!(
            events[1],
            ParseEvent::Message {
                role: MsgRole::Assistant,
                ..
            }
        ));
        // The Maestro pack must NEVER surface a PTY-scraped approval prompt.
        assert!(!events
            .iter()
            .any(|e| matches!(e, ParseEvent::AwaitingApproval { .. })));
    }

    #[test]
    fn empty_buffer_yields_no_events() {
        let pack = MaestroPack::new();
        let mut buf: Vec<u8> = Vec::new();
        assert!(pack.parse_chunk(&mut buf).is_empty());
    }
}
