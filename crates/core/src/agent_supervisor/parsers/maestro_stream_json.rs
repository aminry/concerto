//! Maestro `stream-json` parser pack: adapts Claude's `--output-format
//! stream-json` event lines into `ParseEvent`s for the Maestro chat. Distinct
//! from the regex `ClaudeCodePack` (terminal scrape) — this is the structured
//! path the Maestro chat needs. Tool calls ride the MCP channel; here they are
//! swallowed (no chat bubble in M1).

use crate::agent_supervisor::parsers::{MsgRole, ParseEvent, ParserPack};
use crate::agent_supervisor::AgentKind;
use crate::security::Decision;

/// Stateless Maestro stream-json pack.
#[derive(Debug, Default, Clone)]
pub struct MaestroStreamJsonPack;

impl MaestroStreamJsonPack {
    pub fn new() -> Self {
        Self
    }
}

/// Extract one line's `ParseEvent`s from a parsed Claude stream-json object.
fn events_for(v: &serde_json::Value) -> Vec<ParseEvent> {
    match v.get("type").and_then(|t| t.as_str()) {
        Some("assistant") => {
            let mut out = Vec::new();
            if let Some(parts) = v.pointer("/message/content").and_then(|c| c.as_array()) {
                for p in parts {
                    if p.get("type").and_then(|t| t.as_str()) == Some("text") {
                        if let Some(text) = p.get("text").and_then(|t| t.as_str()) {
                            if !text.is_empty() {
                                out.push(ParseEvent::Message {
                                    role: MsgRole::Assistant,
                                    content: text.to_string(),
                                });
                            }
                        }
                    }
                }
            }
            out
        }
        Some("result") => {
            let mut out = Vec::new();
            if v.get("is_error").and_then(|b| b.as_bool()) == Some(true) {
                let reason = v
                    .get("result")
                    .and_then(|r| r.as_str())
                    .unwrap_or("the Maestro hit an error");
                out.push(ParseEvent::Message {
                    role: MsgRole::Assistant,
                    content: reason.to_string(),
                });
            }
            out.push(ParseEvent::TurnComplete);
            out
        }
        _ => Vec::new(),
    }
}

impl ParserPack for MaestroStreamJsonPack {
    fn agent_kind(&self) -> AgentKind {
        AgentKind::Maestro
    }

    fn version_pattern(&self) -> &str {
        r"(claude|codex|gemini)"
    }

    fn parse_chunk(&self, buf: &mut Vec<u8>) -> Vec<ParseEvent> {
        let mut out = Vec::new();
        while let Some(nl) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=nl).collect();
            let line = &line[..line.len() - 1]; // strip '\n'
            let s = String::from_utf8_lossy(line);
            let s = s.trim();
            if s.is_empty() {
                continue;
            }
            match serde_json::from_str::<serde_json::Value>(s) {
                Ok(v) => out.extend(events_for(&v)),
                Err(e) => {
                    tracing::warn!(target: "concerto::maestro", error = %e, "skipping unparseable maestro stream-json line");
                }
            }
        }
        out
    }

    fn inject_approval(&self, _decision: Decision) -> Vec<u8> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_assistant_text_and_turn_complete_from_fixture() {
        let pack = MaestroStreamJsonPack::new();
        let data = include_bytes!("../../../tests/fixtures/maestro_stream_json/turn.jsonl");
        let mut buf = Vec::new();
        let mut events = Vec::new();
        // Feed in 7-byte chunks to prove partial lines are buffered, not dropped.
        for chunk in data.chunks(7) {
            buf.extend_from_slice(chunk);
            events.extend(pack.parse_chunk(&mut buf));
        }
        let texts: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                ParseEvent::Message {
                    role: MsgRole::Assistant,
                    content,
                } => Some(content.clone()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t.contains("check your workspaces")));
        assert!(texts.iter().any(|t| t.contains("1 workspace")));
        assert!(!texts
            .iter()
            .any(|t| t.contains("tool_use") || t.contains("tool_result")));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, ParseEvent::TurnComplete))
                .count(),
            1
        );
    }
}
