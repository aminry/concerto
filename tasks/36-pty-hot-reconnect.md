# Task 36 — PTY Hot Reconnect After Core Restart

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 21, 22 |
| Touches subsystem(s) | 04 (Agent Supervisor), 01 (Runtime) |
| Smoke gate | unchanged |

## Goal
Implement the hot-reconnect side of the Core ↔ agent-host bridge. After a Core restart (clean or crashed), the supervisor scans `~/concerto/runtime/agents/*.sock`, reconnects to each live host with the cookie-verified `Hello`, and replays the ring buffer from the last acked offset. After this task, killing Core mid-session and restarting it does NOT disrupt the agent.

## Inputs to read before starting
- `design/04_Agent_Supervisor.md` §3.9 (host design + bridge protocol), §6.4 (host adoption — hot vs cold), §7.3 (sequence diagram).
- `design/01_Core_Daemon_Runtime.md` §6.3 (agent host adoption from runtime POV).
- `tasks/21-agent-host-binary.md` → "Handoff Notes" (the host already supports reconnect; this task wires the Core side).

## Scope — in
- Add `adopt_orphans()` method to `AgentSupervisorActor`:
  - Scan `<data_dir>/runtime/agents/*.sock`.
  - For each socket: read the `sessions` row whose host_socket matches, retrieve `pty_cookie` + `last_acked_seq`.
  - Open the UDS; send `HostFrame::Hello { core_version, expected_cookie }`.
  - On `Ready { last_seq, external_session_id }` reply:
    - Persist `external_session_id` if newly known.
    - Resume the bridge-read task.
    - Bytes past `last_seq` arrive as replay; treated identically to live bytes by the parser.
  - On cookie mismatch or non-Ready: close the connection; mark the session `crashed` (cold-resume path is Task 37).
- Wire `adopt_orphans` into the Core startup sequence: called AFTER `RootSupervisor::spawn(AgentSupervisorActor)` but BEFORE accepting gRPC traffic. Update Task 11/12's startup order.
- Add ack tracking: the bridge-read task on the Core side periodically sends `HostFrame::Ack { seq }` to the host so the host can prune its ring buffer. V0.1 acks every 100 bytes received or every 100ms (whichever first). Persist the last-acked seq to `sessions` periodically so a Core crash doesn't lose progress.
- Add `last_acked_seq: u64` column to `sessions` table via a new migration `0002_session_ack.sql`:
  ```sql
  ALTER TABLE sessions ADD COLUMN last_acked_seq INTEGER NOT NULL DEFAULT 0;
  ```
- Tests:
  - Start a Core in `test-harness`; spawn an echo session (long-running — modify to `sleep` for the test fixture); kill the Core process forcibly; restart Core; verify the session resumes (same `session_id`, status returns to `running`, additional output continues to stream).
  - Cookie-mismatch path: corrupt the cookie in DB; restart; verify the session is marked `crashed`.

## Scope — out
- Cold resume from JSONL (Task 37).
- Watchdog-driven restart of hung sessions (V1.0).
- Adopt persistence beyond simple file-scan (e.g., race condition where a host died mid-spawn — V1.0 cleanup).

## Public interface this task locks
- Cookie persistence: `sessions.pty_cookie` (BLOB) is the canonical store.
- Ack scheme: Core sends `Ack { seq }`; host prunes ring buffer.
- DB migration `0002_session_ack.sql` adds `last_acked_seq`. Migration numbers move forward (Task 09 was 0001).

## Implementation notes
- Persist `last_acked_seq` opportunistically — every 5 seconds, write the in-memory ack watermark for every running session. Cheap, single SQL `UPDATE`.
- The replay during reconnect: the host streams the buffered bytes immediately on `Ready`. Each subsequent `StdoutBytes` frame from the host has its own `seq`. The Core's bridge-read task processes them identically to live frames.
- For the test, killing the Core: `process.kill()` is SIGKILL by default — the host should survive because it's detached.
- Use `tracing::info!` to log every adoption attempt (success, cookie-mismatch, no-host-found).

## Verification
1. `cargo build --workspace` → succeeds (new migration compiles).
2. `cargo test -p concerto-core hot_reconnect` → tests pass.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual end-to-end:
   - Start Core; spawn a `claude` session; have it doing a long task.
   - `kill -9 $CORE_PID`.
   - Start Core again.
   - Verify the session reappears in `Sessions.ListSessions` with `status=running`.
   - Verify ongoing output streams.
   - Verify no agent processes were killed (`ps aux | grep claude`).
5. `./scripts/regen-interfaces.sh && git diff docs/interfaces/schema.md` → migration reflected.
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] Hot reconnect works end-to-end (no agent process killed by Core restart).
- [ ] Cookie-mismatch path correctly marks session crashed.
- [ ] Ack persistence verified by killing Core mid-stream and observing no double-replay on reconnect.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/persist/migrations/0002_session_ack.sql` (new)
- `crates/core/src/agent_supervisor/adopt.rs` (new)
- `crates/core/src/agent_supervisor/actor.rs` (modified — calls adopt_orphans on start)
- `crates/core/src/agent_supervisor/bridge.rs` (modified — ack semantics)
- `crates/core/src/main.rs` (modified — startup ordering)
- `crates/core/tests/hot_reconnect.rs` (new)
- `docs/interfaces/schema.md` (regenerated)

## Commit message
```
phase-3: PTY hot reconnect across Core restart

adopt_orphans() scans /runtime/agents/*.sock at boot, sends
Hello{cookie}, replays past last_acked_seq. Sessions table gets a
last_acked_seq column. Killing Core mid-session no longer disrupts
agents.

Refs: tasks/36-pty-hot-reconnect.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** cold resume (host dead too) is Task 37.
- **Smoke-gate state:** unchanged.
