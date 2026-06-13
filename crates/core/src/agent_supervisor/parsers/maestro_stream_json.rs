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

    // Helper: run pack over a single complete input (appends \n if missing).
    fn run(input: &str) -> Vec<ParseEvent> {
        let pack = MaestroStreamJsonPack::new();
        let mut buf = input.as_bytes().to_vec();
        if buf.last() != Some(&b'\n') {
            buf.push(b'\n');
        }
        pack.parse_chunk(&mut buf)
    }

    #[test]
    fn multiple_text_parts_emit_multiple_messages() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello"},{"type":"text","text":"World"}]}}"#;
        let events = run(line);
        let texts: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                ParseEvent::Message { role: MsgRole::Assistant, content } => Some(content.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["Hello", "World"]);
    }

    #[test]
    fn empty_text_part_emits_no_message() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":""}]}}"#;
        let events = run(line);
        assert!(events.is_empty(), "expected no events for empty text, got {events:?}");
    }

    #[test]
    fn tool_use_part_in_assistant_emits_nothing() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu1","name":"foo","input":{}}]}}"#;
        let events = run(line);
        assert!(events.is_empty(), "tool_use must not bubble, got {events:?}");
    }

    #[test]
    fn error_result_emits_message_then_turn_complete() {
        let line = r#"{"type":"result","is_error":true,"result":"boom"}"#;
        let events = run(line);
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[0], ParseEvent::Message { role: MsgRole::Assistant, content } if content == "boom"),
            "first event should be Message(boom), got {:?}",
            events[0]
        );
        assert!(matches!(events[1], ParseEvent::TurnComplete));
    }

    #[test]
    fn success_result_emits_only_turn_complete() {
        let line = r#"{"type":"result","is_error":false,"result":"done"}"#;
        let events = run(line);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], ParseEvent::TurnComplete));
    }

    #[test]
    fn garbage_line_skipped_does_not_abort_subsequent() {
        let pack = MaestroStreamJsonPack::new();
        let input = b"not json at all\n{\"type\":\"result\",\"is_error\":false}\n";
        let mut buf = input.to_vec();
        let events = pack.parse_chunk(&mut buf);
        assert!(buf.is_empty(), "all complete lines must be consumed");
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], ParseEvent::TurnComplete));
    }

    #[test]
    fn blank_line_skipped() {
        let pack = MaestroStreamJsonPack::new();
        let input = b"\n\n{\"type\":\"result\",\"is_error\":false}\n";
        let mut buf = input.to_vec();
        let events = pack.parse_chunk(&mut buf);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], ParseEvent::TurnComplete));
    }

    #[test]
    fn partial_line_retained_in_buf() {
        let pack = MaestroStreamJsonPack::new();
        // Feed first half with no newline — nothing should be emitted and partial must stay.
        let half = b"{\"type\":\"result\",\"is_error\":false}";
        let mut buf = half.to_vec();
        let events = pack.parse_chunk(&mut buf);
        assert!(events.is_empty(), "partial line must not be parsed");
        assert_eq!(buf, half, "partial bytes must remain in buf");
        // Now complete the line.
        buf.push(b'\n');
        let events = pack.parse_chunk(&mut buf);
        assert!(buf.is_empty());
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], ParseEvent::TurnComplete));
    }

    #[test]
    fn crlf_line_endings_handled() {
        let pack = MaestroStreamJsonPack::new();
        let input = b"{\"type\":\"result\",\"is_error\":false}\r\n";
        let mut buf = input.to_vec();
        let events = pack.parse_chunk(&mut buf);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], ParseEvent::TurnComplete));
    }

    #[test]
    fn system_and_user_lines_emit_nothing() {
        let sys = r#"{"type":"system","subtype":"init","tools":[]}"#;
        let usr = r#"{"type":"user","message":{"role":"user","content":[]}}"#;
        assert!(run(sys).is_empty());
        assert!(run(usr).is_empty());
    }

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
