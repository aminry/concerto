//! Per-CLI parser packs for the Agent Supervisor (Task 33).
//!
//! A parser pack is the in-process adapter between the raw byte stream
//! the host emits over `HostFrame::StdoutBytes` and the typed
//! [`crate::agent_supervisor::events::AgentEvent`] view the Streams
//! service surfaces to clients. V0.1 ships two packs:
//!
//! - [`echo::EchoPack`] — trivial pass-through used by the smoke gate
//!   (`agent_kind = Echo`). Emits one [`ParseEvent::Message`] per chunk,
//!   never raises [`ParseEvent::AwaitingApproval`].
//! - [`claude_code::ClaudeCodePack`] — V0.1 terminal-mode regex pack
//!   that watches for Claude Code's tool-approval prompt. Fragile by
//!   design (per `design/04 §3.2` — the V1.0 structured parser is the
//!   robust path); test coverage is via the captured fixture under
//!   `crates/core/tests/fixtures/claude_code/`.
//!
//! The trait signatures are locked per `tasks/33-tool-approval-intercept.md
//! §Public interface this task locks` — adding new variants to
//! [`ParseEvent`] / [`MsgRole`] is fine (additive, non-breaking), but
//! repurposing existing variants or changing the function signatures
//! is not.

use crate::agent_supervisor::AgentKind;
use crate::security::Decision;

pub mod claude_code;
pub mod echo;
// Task 402: the Maestro's structured/no-op pack (its tool calls ride the
// MCP channel, not the PTY scrape — so it is NOT `ClaudeCodePack`).
pub mod maestro;

/// Locked trait for per-CLI parser packs.
///
/// Pack state lives in the implementor; the supervisor holds it as
/// `Box<dyn ParserPack>` on the [`crate::agent_supervisor::SessionEntry`]
/// and feeds it one `StdoutBytes` payload at a time via [`parse_chunk`].
/// Returning multiple [`ParseEvent`]s per chunk is supported (e.g. a
/// chunk that contains both a message + a turn-complete marker).
///
/// [`parse_chunk`]: ParserPack::parse_chunk
pub trait ParserPack: Send + Sync {
    /// Which agent CLI this pack adapts.
    fn agent_kind(&self) -> AgentKind;

    /// Regex matching the agent's `--version` output. V0.1 uses this as
    /// a documentation hint only; the supervisor does not run version
    /// detection in the spawn path because the agent CLI may not expose
    /// a `--version` flag (echo's wrapper doesn't, and Claude Code
    /// changes the format over releases). V1.0 ties this string to a
    /// per-version pack registry.
    fn version_pattern(&self) -> &str;

    /// Consume `buf` (which the supervisor accumulates across chunks
    /// for partial-line buffering) and return zero or more
    /// [`ParseEvent`]s.
    ///
    /// Implementors MAY drain `buf` (the canonical pattern when the
    /// pack just emits a `Message` for the whole chunk) or hold bytes
    /// back for partial-line accumulation. The supervisor doesn't
    /// inspect `buf` between calls.
    fn parse_chunk(&self, buf: &mut Vec<u8>) -> Vec<ParseEvent>;

    /// Compute the bytes to write back to the agent's stdin to resolve
    /// a pending tool-approval prompt. Called once per resolved
    /// [`ParseEvent::AwaitingApproval`] (auto or manual).
    ///
    /// V0.1 menu mappings live in each pack — echo returns an empty
    /// `Vec` (no-op), Claude Code writes `"y\n" | "2\n" | "n\n"` per
    /// its rendered menu.
    fn inject_approval(&self, decision: Decision) -> Vec<u8>;
}

/// Role on a parsed [`ParseEvent::Message`]. Mirrors the
/// `chat_messages.role` CHECK set so V1.0 parser packs can write the
/// same string straight into the DB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgRole {
    User,
    Assistant,
    System,
    Tool,
}

/// One event drained from a parser pack's `parse_chunk`.
///
/// `#[non_exhaustive]` so V1.0 can add `ToolResult` / `ContextUsage` /
/// `Error` / `Crashed` without a wire break.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ParseEvent {
    /// Raw bytes pass-through for the `session.io.<sid>` subject. V0.1
    /// emits this for every `StdoutBytes` frame so the terminal view
    /// always sees the raw stream regardless of which parsed events the
    /// pack pulls out.
    Bytes(Vec<u8>),
    /// A parsed chat-style message (typically one assistant message per
    /// terminal-mode chunk).
    Message { role: MsgRole, content: String },
    /// A parsed tool call. V0.1's parser packs surface this as a sibling
    /// of `AwaitingApproval` when the gate fires; V1.0's structured
    /// parsers emit it as a standalone event tied to a later
    /// `ToolResult`.
    ToolCall {
        name: String,
        args: serde_json::Value,
        call_id: String,
    },
    /// The agent paused for a tool-approval decision. The supervisor
    /// consults the [`crate::security::PermissionResolver`], persists a
    /// `tool_approvals` row, and either auto-decides + calls
    /// [`ParserPack::inject_approval`] OR raises
    /// `AgentEvent::AwaitingApproval` and parks waiting for the user.
    AwaitingApproval {
        tool: String,
        summary: String,
        payload: serde_json::Value,
    },
    /// The agent finished a turn. Mapped to
    /// `AgentEvent::TurnComplete` and forwarded as
    /// `SessionEvent.kind.turn_complete` on the wire.
    TurnComplete,
}
