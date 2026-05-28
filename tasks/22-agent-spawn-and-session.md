# Task 22 — Agent Spawn Flow (Core Side)

| Field | Value |
|---|---|
| Phase | 2 |
| Size | medium (1–3d) |
| Depends on | 10, 20, 21 |
| Touches subsystem(s) | 04 (Agent Supervisor), 09 (Persistence) |
| Smoke gate | unchanged |

## Goal
Implement the Core-side `AgentSupervisorActor` that spawns `concerto-agent-host`, connects to its UDS, completes the `Hello/Ready` handshake, and streams agent output back through the broadcast channel. V0.1 starts with `echo` and proves the pipeline; then with `claude`. Sessions are persisted to the `sessions` table.

## Inputs to read before starting
- `design/04_Agent_Supervisor.md` §3.1 (PTY library is portable-pty inside host), §3.9 (host bridge), §3.10 (permission modes — read but don't fully enforce in this task; Task 42), §3.11 (preamble), §4.1 (in-memory session state), §6.1 (spawn sequence), §6.2 (output pipeline).
- `design/09_Persistence.md` §4.2 (`sessions` schema).
- `tasks/21-agent-host-binary.md` → "Handoff Notes" (confirms detachment strategy).

## Scope — in
- Implement `crates/core/src/agent_supervisor/` with:
  - `AgentSupervisorActor` (impl `Actor` from Task 12).
  - `start_session(req: StartSession) -> Result<SessionId>` that:
    1. Validates the workarea exists and is in a startable state.
    2. Generates a 32-byte cookie via `getrandom`.
    3. Allocates a socket path `<data_dir>/runtime/agents/<session_id>.sock`.
    4. Generates a (V0.1 minimal) Concerto preamble — see Implementation notes for V0.1 simplification.
    5. Persists a `sessions` row with `status=starting`, `host_pid=NULL`, `pty_cookie=<bytes>`.
    6. Spawns `concerto-agent-host` via `tokio::process::Command` with the detachment flag (`pre_exec` calling `setsid` on Unix).
    7. Polls for the socket file (timeout 10s).
    8. Connects, sends `Hello`, awaits `Ready`.
    9. On `Ready`, updates the `sessions` row: `host_pid`, `status=running`.
    10. Spawns a per-session background task that reads frames from the bridge and forwards to a `tokio::sync::broadcast::Sender<AgentEvent>` (one event per parsed frame).
  - `send_input(sid, bytes) -> Result<()>` writes a `StdinBytes` frame.
  - `stop_session(sid, reason) -> Result<()>` sends a signal (host detects child exit; in V0.1 just `kill` the host PID and let it shut down; cleaner shutdown via stdin EOF is V1.0).
  - `subscribe_events(sid) -> broadcast::Receiver<AgentEvent>` for `Streams` subscribers.
- For V0.1, the only supported `agent_kind` values are `echo` (a test mode that runs `echo "$ARGS"`) and `claude` (the real Claude Code CLI). `codex` and `gemini` accept the kind but error with `NOT_IMPLEMENTED` until a parser pack arrives in V1.0.
- For V0.1, parsing is minimal: every `StdoutBytes` frame becomes an `AgentEvent::Message { role: Assistant, content: String }`. No tool-call detection yet (Task 33). No turn-complete detection (Task 34). The structured parser packs from `design/04 §3.2` arrive in Phase 3.
- Persistence helpers in `crates/persist/src/sessions.rs`:
  - `insert(tx, NewSession) -> Result<SessionId>`
  - `update_host(tx, id, host_pid, status) -> Result<()>`
  - `update_status(tx, id, status) -> Result<()>`
  - `get(reader, id) -> Result<Option<Session>>`
  - `list_by_workarea(reader, workarea_id) -> Result<Vec<Session>>`
  - `mark_ended(tx, id, ended_at) -> Result<()>`
- Integration test using `test-harness`:
  - Create project, repo, workspace, workarea.
  - Spawn a session with `agent_kind=echo`.
  - Subscribe to events; assert an `AgentEvent::Message` arrives with the echo output.
  - Stop the session; assert `status=finished` in DB.

## Scope — out
- Tool-approval intercept (Task 33).
- Checkpoints (Task 34).
- Per-CLI parser packs (Phase 3 — Task 33 introduces the architecture).
- `revert_to_checkpoint` (Task 34).
- MCP config surfacing (Task 35).
- `concerto-mcp` in-process server (V1.0 or later).
- Multi-session per workarea concurrency (V1.0 — `design/04 §3.5` per-workarea write mutex).
- Cold resume (Task 37).
- Hot reconnect after Core restart (Task 36).

## Public interface this task locks
- Rust: `crates/core/src/agent_supervisor/mod.rs` — `AgentSupervisorHandle::start_session`, `.send_input`, `.stop_session`, `.subscribe_events`. Signatures FROZEN.
- Socket path scheme: `<data_dir>/runtime/agents/<session_id>.sock`. Frozen.
- `AgentEvent` enum in `crates/core/src/agent_supervisor/events.rs` — V0.1 ships with `Started`, `Message`, `Exited` variants. Phase 3 adds `ToolCall`, `ToolResult`, `AwaitingApproval`, `CheckpointCreated`, `TurnComplete`, `ContextUsage`, `Error`, `Crashed`. Variants are open; field numbers via prost will be assigned in Task 23's proto.

## Implementation notes
- V0.1 preamble: just a string `"You are running inside Concerto. Workarea root: <path>. Repositories: <list>."`. The full templated preamble from `design/04 §3.11` is a Phase 3 task.
- For `echo` agent kind: spawn `concerto-agent-host --agent-bin echo --agent-arg "<the input>"` — useful as a smoke target.
- For `claude` agent kind: spawn `concerto-agent-host --agent-bin claude` and forward stdin/stdout. No `--system-prompt` flag yet; preamble comes via the working directory `CLAUDE.md` file in Phase 3.
- The `getrandom` crate is the standard for OS-provided randomness; add as a dep.
- Use `tokio::time::timeout` on the socket-poll loop with 10s; on timeout, kill the host process and return an error.
- The `pre_exec` callback for `setsid`:
  ```rust
  use std::os::unix::process::CommandExt;
  cmd.pre_exec(|| { unsafe { libc::setsid(); } Ok(()) });
  ```
- Spawn the agent-host binary via `env!("CARGO_BIN_EXE_concerto-agent-host")` if testing, or by resolving it from the same dir as the Core binary at runtime (use `std::env::current_exe()` + `parent()`).

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core agent_supervisor` → integration test passes (echo round-trip).
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual: spawn Core; via gRPC (next task wires the RPC; for this task use a Rust-test client directly) start an `echo` session; verify output streams; stop session; verify DB row reflects `finished`.
5. `find $CONCERTO_DATA_DIR/runtime/agents/` should have NO `.sock` files after the session ends cleanly.
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] Echo round-trip works end-to-end.
- [ ] Session DB row transitions starting→running→finished.
- [ ] Per-session log file is created (basic — `$CONCERTO_DATA_DIR/agents/<sid>/stdout.log` per `design/04 §4`).
- [ ] No `TODO` / `FIXME` / `todo!()` in new code beyond explicitly-deferred placeholders for Phase 3 (note any in Handoff).
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/core/src/agent_supervisor/mod.rs` (new)
- `crates/core/src/agent_supervisor/actor.rs` (new)
- `crates/core/src/agent_supervisor/bridge.rs` (new — Core-side of host CBOR protocol)
- `crates/core/src/agent_supervisor/events.rs` (new)
- `crates/core/src/agent_supervisor/spawn.rs` (new)
- `crates/persist/src/sessions.rs` (new)
- `crates/persist/src/lib.rs` (modified)
- `crates/core/src/main.rs` (modified — spawns AgentSupervisorActor)
- `crates/core/tests/agent_spawn.rs` (new)
- `docs/interfaces/rust-api.md`, `schema.md` (regenerated)

## Commit message
```
phase-2: agent spawn flow (Core side)

AgentSupervisorActor spawns concerto-agent-host with setsid()
detachment, completes Hello/Ready over CBOR, streams stdout as
AgentEvent::Message through a broadcast channel. V0.1 supports echo
and claude kinds; codex/gemini error NOT_IMPLEMENTED.

Refs: tasks/22-agent-spawn-and-session.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** no parser packs, no tool-approval intercept, no checkpoints, no cold/hot resume — Phase 3 covers each.
- **Smoke-gate state:** unchanged.
