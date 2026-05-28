# Task 37 — Cold Resume from Agent JSONL

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 36 |
| Touches subsystem(s) | 04 (Agent Supervisor) |
| Smoke gate | unchanged |

## Goal
When the agent-host is dead too (machine reboot, host OOM-killed), spawn a new agent with `claude --resume <external_session_id>` so the agent loads its own conversation JSONL from disk. After this task, even a full machine restart preserves session continuity (provided the agent CLI's own session store on disk survives).

## Inputs to read before starting
- `design/04_Agent_Supervisor.md` §3.9 (cold resume — relies on agent CLI's own JSONL), §6.4 Layer 2 (cold resume flow + auto-resume opt-in).
- `tasks/36-pty-hot-reconnect.md` → "Handoff Notes".

## Scope — in
- Extend `AgentSupervisorActor::adopt_orphans` to handle the cold case:
  - For each `sessions` row in `running` / `awaiting` / `starting` status whose host_socket is absent OR `Hello` failed:
    - Read host's `final.json` if present (`<data_dir>/runtime/agents/<sid>.final.json`).
    - If present: treat as normal "agent ended" → set status to `finished` or `crashed` per the exit code; emit `AgentEvent::Exited`.
    - If absent: the host vanished without writing exit info (likely reboot). Set status to `crashed`. Do NOT auto-restart.
- Add gRPC: `Sessions.ColdResumeSession(SessionId)`:
  - Looks up the session's `external_session_id`.
  - If present, spawn a new agent-host with `--resume <id>` for the same workarea.
  - If absent, spawn a fresh agent (the session row is reused; chat history is preserved but the agent doesn't see it).
  - Either way, transitions the session back to `running`.
- Add auto-resume opt-in: a project setting `auto_resume_agents` (bool, default false) in `projects.settings_json`. If true and `external_session_id` is set, `adopt_orphans` automatically cold-resumes; otherwise the session stays `crashed` until the user calls the RPC.
- Tests:
  - Use `test-harness` to spawn a session, capture its `external_session_id`, kill the host AND remove the socket file, restart Core, verify status is `crashed`.
  - Call `Sessions.ColdResumeSession`; verify a new host process spawns and status returns to `running`.
  - With `auto_resume_agents=true`, verify the same scenario auto-resumes without the RPC.

## Scope — out
- Auto-resume when `external_session_id` is NULL (only the user explicitly clicks "Start fresh" — V1.0 polishes this UX).
- Detecting whether the agent's JSONL file actually exists before attempting `--resume` (V1.0 — for now we trust the agent CLI to error if its file is gone).
- The "Resume agent" UI chip (Desktop work — V1.0).

## Public interface this task locks
- Proto: `Sessions.ColdResumeSession` RPC. Frozen.
- Project setting: `auto_resume_agents` (bool, default false). Field name frozen.
- Cold-resume contract: the new agent-host is invoked with `--resume <external_session_id>`; the agent CLI loads its own JSONL.

## Implementation notes
- Reading `final.json`: a small struct with `serde_json::from_str`; tolerate missing fields.
- Spawning a host with `--resume`: extend Task 22's `start_session` to accept an optional `resume_session_id` parameter; pass it through to `concerto-agent-host`.
- For `auto_resume_agents`: read from `projects.settings_json` via the persistence layer — fall back to `false` on missing key.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core cold_resume` → tests pass.
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual:
   - Spawn `claude` session; verify `external_session_id` populated (parser extracts it on first banner; for now you may need to insert a stub external_session_id manually if the V0.1 parser doesn't yet extract it — note in Handoff).
   - Kill host PID via `kill -9`; remove the socket file.
   - Restart Core; verify session is `crashed`.
   - `Sessions.ColdResumeSession`; verify a new host spawns + agent resumes.
5. `./scripts/regen-interfaces.sh && git diff` → committed.
6. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] Cold resume works end-to-end.
- [ ] Auto-resume gated correctly on the project setting.
- [ ] Sessions without external_session_id error cleanly with NOT_FOUND.
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/core/src/agent_supervisor/adopt.rs` (modified — cold path)
- `crates/core/src/agent_supervisor/cold_resume.rs` (new)
- `crates/core/src/agent_supervisor/actor.rs` (modified)
- `crates/proto/proto/concerto/v1/sessions.proto` (modified)
- `crates/core/src/handlers/sessions.rs` (modified)
- `crates/core/tests/cold_resume.rs` (new)
- `docs/interfaces/proto.md`, `rust-api.md` (regenerated)

## Commit message
```
phase-3: cold resume from agent JSONL

When host is gone too, Sessions.ColdResumeSession spawns a new
agent-host with --resume <external_session_id>; the agent CLI loads
its conversation from disk. Project setting auto_resume_agents
(default false) controls auto-resume on Core start.

Refs: tasks/37-cold-resume.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** parser may not yet extract external_session_id; gap noted for Phase 3 polish.
- **Deliberate debt:** UI "Resume agent" chip is V1.0 Desktop work.
- **Smoke-gate state:** unchanged.
