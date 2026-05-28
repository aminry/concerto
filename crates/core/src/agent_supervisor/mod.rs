//! Agent Supervisor subsystem (Task 22, design/04).
//!
//! Core-side counterpart to the `concerto-agent-host` binary built in
//! Task 21. The supervisor:
//!
//! - Spawns one `concerto-agent-host` per session, detached via
//!   Unix `pre_exec` + `setsid()` so the host survives Core restart
//!   (`design/01 §6.3`, `design/04 §3.9`).
//! - Completes the locked CBOR `Hello`/`Ready` handshake with the
//!   host over a per-session UDS at `<data_dir>/runtime/agents/<sid>.sock`.
//! - Streams `StdoutBytes` frames from the host as
//!   [`AgentEvent::Message`] on a `tokio::sync::broadcast` channel.
//! - Persists the `sessions` table row (`starting → running → finished`)
//!   inside the same transaction that inserts the per-session `chats` row
//!   the `chat_id` FK requires.
//!
//! ## V0.1 scope
//!
//! Per Task 22, V0.1 supports two `agent_kind` values:
//!
//! - `"echo"` — a test mode that spawns `concerto-agent-host --agent-bin echo`.
//!   The wrapped command prints its `--agent-arg` payload to stdout and
//!   exits, which the supervisor surfaces as one `AgentEvent::Message`
//!   followed by `AgentEvent::Exited`. The session row uses
//!   `agent_kind = "claude"` so the migration-0001 CHECK constraint stays
//!   intact — the schema kind and the spawn binary are decoupled.
//! - `"claude"` — the real Claude Code CLI.
//!
//! `"codex"` and `"gemini"` are accepted at the type level but currently
//! return a `NOT_IMPLEMENTED` error (parser-pack work, Task 33).
//!
//! ## Module layout
//!
//! - [`actor`] — [`AgentSupervisorActor`] + [`AgentSupervisorHandle`].
//! - [`bridge`] — thin wrapper around `concerto_agent_host::bridge` so the
//!   Core consumes the same locked CBOR codec the host emits.
//! - [`events`] — [`AgentEvent`] enum (`Started`/`Message`/`Exited`).
//! - [`spawn`] — Unix `pre_exec(setsid)` + socket-poll glue.

#![cfg(unix)]

pub mod actor;
pub mod bridge;
pub mod events;
pub mod spawn;

pub use actor::{
    AgentKind, AgentSupervisorActor, AgentSupervisorConfig, AgentSupervisorHandle,
    StartSessionRequest,
};
pub use events::{AgentEvent, MessageRole};
