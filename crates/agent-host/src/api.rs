//! Public surface of `concerto-agent-host`.
//!
//! Per the workspace convention locked in Task 04, this module is what
//! `scripts/regen-interfaces.sh` reads to produce
//! `docs/interfaces/rust-api.md`. Types live here directly (not as
//! `pub use` re-exports) so the interface generator captures them.
//!
//! The locked surface for Task 21 is:
//!
//! * [`HostFrame`] — the length-prefixed CBOR frame protocol exchanged
//!   between the host and the Core. Variant ordering and field names are
//!   FROZEN: changing any of them is a wire-format break that needs a new
//!   task. See `design/04 §3.9` for the protocol context.
//! * [`AgentKind`] — the agent CLI we are wrapping. V0.1 covers Claude and
//!   Codex; `Other(String)` carries the raw `--agent-bin` basename for
//!   logging when neither matches.
//! * [`FinalInfo`] — JSON written to `--final-info` once the PTY child
//!   exits. Schema is locked by the task spec; consumed by the Core's
//!   Agent Supervisor when adopting (or mourning) a host on next boot.
//!
//! The frame-encoding cap and ring-buffer cap are documented next to
//! their constants in `crate::bridge` and `crate::ring`.

use serde::{Deserialize, Serialize};

/// Maximum encoded frame length the host will read or send. Frames larger
/// than this are treated as a protocol error and tear the connection down
/// rather than allocating an unbounded buffer (see `design/00 §7.3`).
///
/// 16 MiB is well above any plausible single PTY chunk (we cap individual
/// reads at 64 KiB) but small enough that a malicious peer can't OOM the
/// host process.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Size of the in-memory ring buffer that survives Core
/// disconnect/reconnect. Per `design/04 §3.9`.
pub const RING_BUFFER_BYTES: usize = 1024 * 1024;

/// Kind of agent CLI being wrapped.
///
/// V0.1 only ships Claude and Codex (per `tasks/README.md §2`); the
/// `Other` variant carries the unrecognised basename so logs stay useful
/// in development without forcing a wire-format change every time a new
/// agent is added.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name")]
pub enum AgentKind {
    Claude,
    Codex,
    Other(String),
}

/// Wire frame exchanged over the host-bridge UDS.
///
/// Frame layout on the socket:
///
/// ```text
/// [u32 BE length] [CBOR-encoded HostFrame]
/// ```
///
/// Variant set and field names match `design/04 §3.9` verbatim. The
/// `Hello { core_version, expected_cookie }` variant is sent by the Core
/// first; the host either replies with `Ready { .. }` (cookie matches and
/// no other Core is connected) or with `CookieMismatch` followed by a
/// connection close.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum HostFrame {
    /// First frame sent by the Core after the TCP/UDS handshake. Carries
    /// the cookie the Core read from `agent_sessions.pty_cookie` and the
    /// Core's build version (used by the host for diagnostics only — there
    /// is no compatibility check in V0.1).
    Hello {
        core_version: String,
        expected_cookie: [u8; 32],
        /// Last sequence number the Core successfully consumed before its
        /// previous disconnect. The host replays buffered output past this
        /// watermark when reconnecting. Use `0` on first connect.
        last_seq: u64,
    },
    /// Response to a successful `Hello`. Tells the Core the agent metadata
    /// it learned and the highest `seq` the host has emitted so far.
    Ready {
        agent_kind: AgentKind,
        version: String,
        external_session_id: Option<String>,
        last_seq: u64,
    },
    /// Bytes from the Core to inject into the PTY child's stdin.
    StdinBytes { data: Vec<u8> },
    /// PTY stdout chunk pushed to the connected Core. `seq` is
    /// monotonically increasing per host process lifetime.
    StdoutBytes { seq: u64, data: Vec<u8> },
    /// PTY stderr chunk. Reserved for V1.0; `portable-pty` merges stderr
    /// into stdout by default and V0.1 never emits this variant, but the
    /// wire slot is locked here so callers don't add it later.
    StderrBytes { seq: u64, data: Vec<u8> },
    /// Resize the underlying PTY (terminal resize relayed from the user's
    /// xterm.js pane). `rows` and `cols` are character cells.
    Resize { rows: u16, cols: u16 },
    /// Heartbeat request. The host responds with [`HostFrame::Pong`].
    Ping,
    /// Heartbeat response.
    Pong,
    /// Watermark advance from the Core: "I have consumed up to and
    /// including `seq`". The host can prune its ring buffer past this
    /// point. Hot-reconnect ack semantics are finalised in Task 36; the
    /// frame slot is locked here.
    Ack { seq: u64 },
    /// Sent by the host after the PTY child exits. Followed by a clean
    /// socket close. The Core uses this to surface "agent ended" in the
    /// UI; the same information is also written to `--final-info` so a
    /// late-arriving Core can read it from disk.
    AgentExited {
        exit_code: Option<i32>,
        signal: Option<i32>,
    },
    /// Sent by the host when an incoming `Hello`'s cookie does not match
    /// the value passed via `--cookie`. The connection is closed
    /// immediately after this frame is flushed.
    CookieMismatch,
    /// Sent by the host when a second `Hello` arrives while another Core
    /// is already connected (single-connection invariant from
    /// `design/04 §3.9`). The new connection is closed; the existing one
    /// is untouched.
    AlreadyConnected,
}

/// Schema of the JSON document the host writes to `--final-info` once
/// the PTY child has exited. The Core's Agent Supervisor reads this on
/// next boot when the host process is gone but the Core needs to render
/// a meaningful "agent ended" event for the UI.
///
/// Field names match the task spec verbatim and are part of the locked
/// public interface for Task 21.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FinalInfo {
    /// Exit status of the agent CLI, if it terminated normally.
    pub exit_code: Option<i32>,
    /// Signal number if the agent CLI was killed by a signal (Unix). On
    /// platforms or paths where the PTY layer doesn't surface a signal,
    /// this stays `None`.
    pub signal: Option<i32>,
    /// Last lines of PTY output the host saw, capped at 100. Used by the
    /// UI to render the trailing transcript when "agent ended" is shown.
    pub last_lines: Vec<String>,
    /// Agent-emitted session identifier (Claude/Codex resume token), if
    /// the parser found one. None means cold resume will start a fresh
    /// conversation.
    pub external_session_id: Option<String>,
    /// Wall-clock Unix-epoch milliseconds when the host observed the PTY
    /// child exit. The host reads this from `SystemTime::now()` right
    /// before writing the file.
    pub exited_at_unix_ms: i64,
}
