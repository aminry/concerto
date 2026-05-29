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
    /// Task 33: parser pack detected a tool-approval prompt and the
    /// [`crate::security::PermissionResolver`] returned `MustAsk`. The
    /// `approval_id` is the freshly-persisted `tool_approvals` row id;
    /// clients call `Sessions.ResolveApproval` to send the decision.
    AwaitingApproval {
        session_id: SessionId,
        approval_id: String,
        tool: String,
        summary: String,
        payload_json: String,
        /// Task 43: destructive-command intercept fired for this gate.
        /// Clients render the prompt with the red-urgent styling.
        urgent: bool,
        /// Task 43: human-readable destructive-pattern label
        /// (`"force-push"`, `"recursive-delete"`, …) — `None` when the
        /// gate fired for a non-destructive reason. The label set is
        /// frozen by [`crate::security::destructive::PATTERNS`].
        destructive_label: Option<String>,
    },
    /// Task 33: an approval was resolved (auto or manual). `decision`
    /// is the `tool_approvals.decision` string (one of
    /// `approve|approve_once|deny|auto_*`).
    ApprovalResolved {
        session_id: SessionId,
        approval_id: String,
        tool: String,
        decision: String,
    },
    /// Task 33: a parsed tool call. V0.1 emits this as a sibling event
    /// to `AwaitingApproval` when the parser pack surfaces a structured
    /// call; the terminal-mode pack only emits it when the underlying
    /// `ParseEvent::ToolCall` variant fires (V1.0 work).
    ToolCall {
        session_id: SessionId,
        call_id: String,
        name: String,
        args_json: String,
    },
    /// Task 33: agent finished a turn. V0.1's terminal-mode parsers
    /// don't currently detect this boundary; V1.0 structured parsers
    /// are authoritative. The variant is wired here so the supervisor
    /// and streams handler can forward it once the parser packs
    /// surface it.
    TurnComplete { session_id: SessionId },
    /// Task 34: a per-repo checkpoint was created at the end of a turn.
    /// `git_ref` is the FROZEN
    /// `refs/concerto/checkpoints/<workarea>/<repository>/<n>` form;
    /// `checkpoint_id` is the freshly-persisted `checkpoints.id`. A
    /// multi-repo turn fires one variant per repo all sharing the same
    /// `chat_message_id` (V1.0).
    CheckpointCreated {
        session_id: SessionId,
        checkpoint_id: String,
        git_ref: String,
    },
    /// Task 40: agent reported a context-window utilisation percentage.
    /// `pct` is the 0..=100 integer the parser pack extracted from the
    /// CLI's status line. V0.1 parser packs do not yet emit this — the
    /// variant is wired so the Suggestion Engine's
    /// `context_window_50` / `context_window_80` rules can fire as soon
    /// as a parser pack starts surfacing the signal. The Suggestion
    /// Engine consumes the event; downstream `Streams.session.events`
    /// subscribers see no wire change in V0.1 (the variant is mapped to
    /// nothing on the wire today; V1.0 adds the proto field).
    ContextUsage { session_id: SessionId, pct: u8 },
    /// Task 40: agent host crashed (read pump exited unexpectedly). V0.1
    /// emits this only when the supervisor's adoption logic transitions
    /// a row to `'crashed'` — the rule engine listens so the
    /// `agent_crashed` chip can surface a "resume" suggestion. The
    /// session row's `status` column is the authoritative truth; this
    /// event is the in-process notification carrier.
    Crashed { session_id: SessionId },
}
