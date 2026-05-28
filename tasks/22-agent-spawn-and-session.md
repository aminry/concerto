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
- [x] Verification commands pass.
- [x] Echo round-trip works end-to-end.
- [x] Session DB row transitions starting→running→finished.
- [x] Per-session log file is created (basic — `$CONCERTO_DATA_DIR/agents/<sid>/stdout.log` per `design/04 §4`).
- [x] No `TODO` / `FIXME` / `todo!()` in new code beyond explicitly-deferred placeholders for Phase 3 (note any in Handoff).
- [x] Smoke gate still green.
- [x] Single commit created.

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
- **Drift from plan:**
  - **`agent_kind = "echo"` is not a DB CHECK value.** Migration 0001's
    `sessions.agent_kind` CHECK is frozen to
    `('claude','codex','gemini','maestro')`. Per the task prompt's
    pre-decision, no new migration was added; the Agent Supervisor's
    in-process `AgentKind::Echo` writes `'claude'` to the DB while
    spawning `concerto-agent-host --agent-bin /bin/echo`. The schema
    kind and the spawn binary are decoupled — production code rejects
    `Codex`/`Gemini` with `NOT_IMPLEMENTED`; the echo path is a V0.1
    test fixture that never appears as `"echo"` on disk. The integration
    test relies on this reuse.
  - **Cookie stored only in-process.** The `sessions.pty_cookie` BLOB is
    populated on insert, but the supervisor's in-memory `SessionEntry`
    keeps the 32 bytes separately so `send_input` / hot-reconnect work
    (Task 36) doesn't have to re-read the BLOB. The DB column is the
    persistent fact; the map entry is the runtime fast-path. No schema
    change.
  - **`chats` ↔ `sessions` cyclic FK resolved via `PRAGMA defer_foreign_keys = ON`.**
    `sessions.chat_id NOT NULL REFERENCES chats(id)` and
    `chats.session_id REFERENCES sessions(id)` form a cycle that
    immediate FK enforcement rejects regardless of insert order.
    `start_session` issues `PRAGMA defer_foreign_keys = ON` at the top
    of its transaction so FK checks run at commit; both rows are
    visible by then. SQLite scopes the pragma to the current
    transaction only — no global behaviour change.
  - **Socket path falls back to `$TMPDIR` when the canonical path
    overflows `SUN_LEN`.** macOS limits `sockaddr_un.sun_path` to ~104
    chars; the locked layout
    `<data_dir>/runtime/agents/<UUIDv7>.sock` can overflow when
    `data_dir` is a deep tempdir (CI tempdirs nest under
    `/var/folders/.../T/.tmpXXXX/`). When the canonical path is ≥100
    chars the supervisor binds at
    `$TMPDIR/ccs-<sid8>.sock` instead. Logs, `stdout.log`, and
    `final-info.json` still live at the canonical
    `<data_dir>/agents/<sid>/` location. Socket is removed on session
    end. Production paths never hit the fallback. Logged here so Task 23
    knows the socket path is not deterministic from the session id.
  - **In-process integration test.** Task 22's prompt called out that
    the `AgentSupervisorHandle` is not yet reachable through gRPC (Task
    23 wires that), so the round-trip test sits in
    `crates/core/tests/agent_spawn.rs` and constructs `Persistence` +
    `AgentSupervisorHandle` directly. The `concerto-agent-host` binary
    is located via `assert_cmd::cargo::cargo_bin`. This is the
    fastest-path proof that the wire protocol is honoured end-to-end
    without inventing a one-off harness accessor.
  - **`with_managers` signature gained an optional `agent_supervisor`
    arg under `#[cfg(unix)]`.** The handle is currently held by
    `ApiServerActor` but no gRPC service registers against it yet —
    Task 23's `Sessions` service is the consumer. The plumbing is
    additive so Task 23 only adds the handler, not the wiring.
  - **`AgentEvent` is `#[non_exhaustive]`.** Phase 3 (`ToolCall`,
    `Approval`, `Checkpoint`, `TurnComplete`, …) can extend the enum
    without a wire-format break; the V0.1 surface ships `Started`,
    `Message`, `Exited` exactly as the task spec called out.
- **Open questions for next task:**
  - Task 23 should add a `concerto-agent-host` resolution helper to the
    test-harness so its sessions-client tests can spawn an end-to-end
    Core that actually has the host binary on the path it expects
    (`current_exe().parent()`). Currently the only consumer is the
    in-process test in this crate.
  - Hot-reconnect on a Core restart (`Hello` with `last_seq > 0`) is
    not wired yet. The cookie is stored in `sessions.pty_cookie` and
    `host_socket` is recorded, so Task 36 has every persistent fact it
    needs; the in-memory `SessionEntry` map is intentionally rebuilt
    from scratch on each boot.
  - The supervisor does not currently watch for the host PID dying out
    from under it (the read-pump task observes the bridge `Eof` /
    `AgentExited` frame). Task 37's cold-resume work is the place to add
    a `tokio::process::Child::wait()` watcher.
- **Deliberate debt:** no parser packs, no tool-approval intercept,
  no checkpoints, no cold/hot resume — Phase 3 covers each.
  `StderrBytes` frames have a hot path in the read pump but the V0.1
  agent-host never emits them (`portable-pty` merges stderr into the
  master); the code is reachable from V1.0.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still exits 0
  with "Smoke gate v1: PASSED". Task 27 promotes the gate to v2.
