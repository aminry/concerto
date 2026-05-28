//! Echo parser pack — trivial pass-through (Task 33).
//!
//! Used by `agent_kind = Echo` (the smoke gate's V0.1 spawn target).
//! Emits one [`ParseEvent::Message`] per chunk with the entire chunk
//! UTF-8-lossy-decoded as the body, and never raises
//! [`ParseEvent::AwaitingApproval`]. [`ParserPack::inject_approval`]
//! returns an empty `Vec` — the echo binary has no stdin loop, so any
//! injection would be a no-op anyway.

use crate::agent_supervisor::parsers::{MsgRole, ParseEvent, ParserPack};
use crate::agent_supervisor::AgentKind;
use crate::security::Decision;

/// Stateless echo pack.
#[derive(Debug, Default, Clone)]
pub struct EchoPack;

impl EchoPack {
    pub fn new() -> Self {
        Self
    }
}

impl ParserPack for EchoPack {
    fn agent_kind(&self) -> AgentKind {
        AgentKind::Echo
    }

    fn version_pattern(&self) -> &str {
        // `/bin/echo` doesn't have a meaningful --version; document the
        // BSD/GNU variants so a future version-aware registry has a
        // plausible regex to inherit.
        r"echo \(GNU coreutils\)|echo \(.*\)"
    }

    fn parse_chunk(&self, buf: &mut Vec<u8>) -> Vec<ParseEvent> {
        if buf.is_empty() {
            return Vec::new();
        }
        // Drain the whole buffer and emit it both as raw bytes and as
        // one assistant message (terminal-mode V0.1 has no
        // partial-line accumulation).
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
        // Echo never raises AwaitingApproval, so this is unreachable in
        // the happy path. Return empty so a misuse is a no-op rather
        // than a panic.
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_pack_emits_bytes_and_message_per_chunk() {
        let pack = EchoPack::new();
        let mut buf = b"hello-world".to_vec();
        let events = pack.parse_chunk(&mut buf);
        assert!(buf.is_empty(), "buf should be drained");
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], ParseEvent::Bytes(_)));
        match &events[1] {
            ParseEvent::Message { role, content } => {
                assert_eq!(*role, MsgRole::Assistant);
                assert_eq!(content, "hello-world");
            }
            other => panic!("expected Message, got {other:?}"),
        }
    }

    #[test]
    fn echo_pack_empty_chunk_is_noop() {
        let pack = EchoPack::new();
        let mut buf = Vec::new();
        let events = pack.parse_chunk(&mut buf);
        assert!(events.is_empty());
    }

    #[test]
    fn echo_pack_inject_approval_is_empty() {
        let pack = EchoPack::new();
        assert!(pack.inject_approval(Decision::AutoApprove).is_empty());
        assert!(pack.inject_approval(Decision::AutoDeny).is_empty());
    }
}
