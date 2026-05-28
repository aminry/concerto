# Task 23 — `Sessions` gRPC Service + `Streams.Subscribe`

| Field | Value |
|---|---|
| Phase | 2 |
| Size | medium (1–3d) |
| Depends on | 22 |
| Touches subsystem(s) | 10 (Local API), 04 (Agent Supervisor) |
| Smoke gate | unchanged |

## Goal
Expose the agent-spawning surface via gRPC. After this task, a client can call `Sessions.CreateSession` to spawn an agent, `Sessions.SendMessage` to write to its stdin, `Sessions.StopSession` to end it, and `Streams.Subscribe(subject="session.events.<sid>")` to receive the typed `AgentEvent` stream. The `Streams` service is the V0.1 minimum needed for the client's terminal view to work.

## Inputs to read before starting
- `design/10_Local_API_Protocol.md` §3.2 (Streams service shape), §3.3 (streaming reconnect — V0.1 skips offset semantics per the phase table, but the SubscribeRequest field exists), §5.1 (`Sessions` and `Streams` service surface), §5.2 (subject catalog — V0.1 needs `session.events.<sid>` and `session.io.<sid>`).
- `design/04_Agent_Supervisor.md` §5.2 (Agents gRPC surface — note this is now consolidated into `Sessions` per `10` design), §4.2 (event types — implement the V0.1 subset).
- `tasks/22-agent-spawn-and-session.md` → "Handoff Notes".

## Scope — in
- Extend `crates/proto/proto/concerto/v1/sessions.proto` (the `Session` message exists from Task 07; add the service):
  ```proto
  service Sessions {
    rpc CreateSession(CreateSessionRequest) returns (Session);
    rpc GetSession(SessionId) returns (Session);
    rpc ListSessions(ListSessionsRequest) returns (ListSessionsResponse);
    rpc SendMessage(SendMessageRequest) returns (google.protobuf.Empty);
    rpc StopSession(StopSessionRequest) returns (google.protobuf.Empty);
  }
  
  message CreateSessionRequest {
    string workarea_id = 1;
    string agent_kind = 2;       // echo | claude (codex/gemini error in V0.1)
    optional string model = 3;
    optional PermissionMode permission_mode = 4;
  }
  
  message SendMessageRequest {
    string session_id = 1;
    bytes  payload = 2;          // bytes to write to agent stdin (typically UTF-8 + newline)
  }
  
  message StopSessionRequest {
    string session_id = 1;
    string reason = 2;           // user_request | error | revert
  }
  ```
- Create `crates/proto/proto/concerto/v1/streams.proto`:
  ```proto
  service Streams {
    rpc Subscribe(SubscribeRequest) returns (stream Event);
  }
  
  message SubscribeRequest {
    string subject = 1;                   // "session.events.<sid>" | "session.io.<sid>" | "workspace.events" | "workarea.events"
    optional string filter = 2;
    optional uint64 since_offset = 3;     // V0.1 ignores
  }
  
  message Event {
    uint64 offset = 1;                    // monotonic per subject
    google.protobuf.Timestamp at = 2;
    oneof body {
      SessionEvent session = 10;
      SessionIoChunk session_io = 11;
      WorkspaceEvent workspace = 12;
      WorkareaEvent workarea = 13;
    }
  }
  
  message SessionEvent {
    string session_id = 1;
    oneof kind {
      AgentStarted started = 10;
      AgentMessage message = 11;
      AgentExited exited = 12;
    }
  }
  
  message AgentStarted { string model = 1; string mode = 2; }
  message AgentMessage { string role = 1; bytes content = 2; }
  message AgentExited { optional int32 exit_code = 1; }
  
  message SessionIoChunk {
    string session_id = 1;
    string stream = 2;                    // stdout | stderr
    bytes  data = 3;
  }
  
  // WorkspaceEvent and WorkareaEvent: minimal V0.1 set (created/archived).
  message WorkspaceEvent { string workspace_id = 1; string kind = 2; }
  message WorkareaEvent  { string workarea_id  = 1; string kind = 2; }
  ```
- Implement `SessionsHandler` in `crates/core/src/handlers/sessions.rs` delegating to `AgentSupervisorHandle`.
- Implement `StreamsHandler` in `crates/core/src/handlers/streams.rs`:
  - For `session.events.<sid>`: extract `<sid>`; call `AgentSupervisorHandle::subscribe_events(sid)`; map each `AgentEvent` to a proto `Event` with monotonic per-subject offset; forward as a gRPC server-stream.
  - For `session.io.<sid>`: subscribe to the raw bytes broadcast (the supervisor exposes a second channel for raw bytes — add this channel to Task 22's actor if it isn't there).
  - For `workspace.events` / `workarea.events`: subscribe to the in-process broadcast channels created in Tasks 19 / 20.
  - V0.1 ignores `since_offset` per the design doc's V0.1 row.
  - Drop the stream when the gRPC channel closes.
- Hook a per-subject monotonic offset counter (`AtomicU64`); offset is assigned at publish time.
- Integration test (`test-harness`):
  - Create workarea.
  - Connect a `Streams` client, subscribe to `session.events.<sid>` (sid known beforehand? — pattern: create session, get sid back, subscribe immediately after).
  - `CreateSession` with `agent_kind=echo`.
  - Receive `AgentStarted` then `AgentMessage` events.
  - `StopSession`; receive `AgentExited`.

## Scope — out
- `since_offset` resume semantics (V1.0 — add when ring buffer arrives).
- `AckOffset` unary RPC (V1.0).
- `GapDetected` events (V1.0).
- Tool-approval streams (Task 33).
- Per-subject ring buffer (V1.0).
- `diff.<workarea>.<repo>` and `checks.<...>` streams (Phase 3 / V1.0).

## Public interface this task locks
- Proto: `Sessions` service with 5 RPCs; `Streams` service with `Subscribe`; `Event.body` oneof variants for V0.1. **Field numbers FROZEN** — Phase 3 adds new oneof variants (`ToolCall`, `AwaitingApproval`, etc.) at higher numbers.
- Subject naming: `session.events.<session_id>`, `session.io.<session_id>`, `workspace.events`, `workarea.events`. Frozen.

## Implementation notes
- Use `tokio_stream::wrappers::BroadcastStream` to convert a `broadcast::Receiver` into a `tonic::codegen::futures_core::Stream`.
- Each `Subscribe` RPC handler returns `Response<Self::SubscribeStream>` where `SubscribeStream = Pin<Box<dyn Stream<Item = Result<Event, Status>> + Send + 'static>>`.
- For correct backpressure: use `broadcast::channel(256)` (already done) and drop messages when the receiver is lagged — the test should still pass because `echo` produces little output.
- The `AgentEvent` (Rust enum) → proto `Event` mapping lives in `crates/core/src/handlers/streams.rs` as a `fn map_agent_event(e: AgentEvent, sid: SessionId, offset: u64) -> Event`.
- Subject parsing: a small helper `parse_subject(s: &str) -> Result<Subject>` returns enum variants like `Subject::SessionEvents(SessionId)`; reject unknown subjects with `INVALID_ARGUMENT`.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core sessions_grpc` → integration test passes end-to-end.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual: start Core; use the smoke client (Task 15) extended to also do `Sessions.CreateSession` + `Streams.Subscribe`; verify events arrive.
5. `./scripts/regen-interfaces.sh && git diff` → committed.
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] Echo agent end-to-end via gRPC (create, subscribe, stop).
- [ ] Unknown stream subject returns INVALID_ARGUMENT with a clear error.
- [ ] No `TODO` / `FIXME` in new code beyond explicit Phase 3 placeholders (noted in Handoff).
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/proto/proto/concerto/v1/sessions.proto` (modified — adds service)
- `crates/proto/proto/concerto/v1/streams.proto` (new)
- `crates/core/src/handlers/sessions.rs` (new)
- `crates/core/src/handlers/streams.rs` (new)
- `crates/core/src/api_server.rs` (modified — registers SessionsServer + StreamsServer)
- `crates/core/src/agent_supervisor/actor.rs` (possibly modified — adds raw-bytes broadcast channel if missing)
- `crates/core/tests/sessions_grpc.rs` (new)
- `docs/interfaces/proto.md` (regenerated)

## Commit message
```
phase-2: Sessions gRPC service + Streams.Subscribe (V0.1 subset)

Sessions service exposes Create/Get/List/SendMessage/Stop.
Streams.Subscribe handles session.events.<sid>, session.io.<sid>,
workspace.events, workarea.events with monotonic offsets. V0.1
ignores since_offset; ring buffer + ack is V1.0.

Refs: tasks/23-sessions-grpc-service.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** no offset/ack/ring-buffer (V1.0); SessionEvent.kind oneof has only 3 variants in V0.1 — Phase 3 adds the rest.
- **Smoke-gate state:** unchanged.
