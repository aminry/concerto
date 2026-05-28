//! `AgentEvent` enum broadcast by the Agent Supervisor (Task 22).
//!
//! V0.1 ships three variants — `Started`, `Message`, `Exited` — per the
//! locked surface in `tasks/22-agent-spawn-and-session.md §"Public
//! interface this task locks"`. Phase 3 adds `ToolCall`, `ToolResult`,
//! `AwaitingApproval`, `CheckpointCreated`, `TurnComplete`,
//! `ContextUsage`, `Error`, `Crashed`. The enum is `#[non_exhaustive]`
//! so adding variants is not a wire-format break for downstream callers
//! that only match the V0.1 set.

use concerto_persist::SessionId;

/// Role on a chat-style message surfaced from the agent. Mirrors the
/// `chat_messages.role` CHECK set in migration 0001 so the V1.0
/// per-CLI parser packs can write the same role values straight into
/// `chat_messages` without a mapping table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

/// Event broadcast by the Agent Supervisor for one session.
///
/// V0.1 surface, per Task 22:
///
/// - [`AgentEvent::Started`] — the session row transitioned to `running`
///   and the bridge is up.
/// - [`AgentEvent::Message`] — a chunk of stdout was received and is
///   surfaced as an assistant message. V0.1 has no per-CLI parsing
///   (Task 33), so every `StdoutBytes` frame becomes one `Message` with
///   `role = Assistant`.
/// - [`AgentEvent::Exited`] — the host reported `AgentExited`. The
///   supervisor has already marked the session `finished` in the DB.
///
/// The enum is `#[non_exhaustive]` so Phase 3 can add tool-call /
/// approval / checkpoint variants without a wire-format break.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum AgentEvent {
    /// Bridge handshake succeeded. Session row is now in `running`.
    Started { session_id: SessionId },
    /// A chunk of agent output. V0.1 always sets `role = Assistant`.
    Message {
        session_id: SessionId,
        role: MessageRole,
        content: String,
    },
    /// Agent process ended. `exit_code` / `signal` follow the
    /// `HostFrame::AgentExited` semantics from `design/04 §3.9`.
    Exited {
        session_id: SessionId,
        exit_code: Option<i32>,
        signal: Option<i32>,
    },
}
