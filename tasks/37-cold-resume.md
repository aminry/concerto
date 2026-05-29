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
- [x] Verification commands pass.
- [x] Cold resume works end-to-end.
- [x] Auto-resume gated correctly on the project setting.
- [x] Sessions without external_session_id error cleanly with NOT_FOUND.
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

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
- **Drift from plan:**
  - **Cold resume REUSES the original `sessions` row instead of creating a continuation row.** The task scope says "transitions the session back to `running`" and the pre-decisions say "marks status `running`" — both implied a single-row lifecycle, so I added `AgentSupervisorHandle::cold_resume_existing(session_id, cwd, resume_token)` which rewrites `host_pid`, `host_socket`, `pty_cookie`, `last_acked_seq=0`, `status='starting'→'running'`, and clears `ended_at` in-place. The chat thread (and thus the Concerto-side conversation history) is preserved verbatim; the agent CLI loads its own JSONL via `--resume`. Cookie rotates per spawn so any defunct host still holding the old cookie can't accept the Hello.
  - **`final-info.json` lives at `<data_dir>/agents/<sid>/final-info.json`, not `<data_dir>/runtime/agents/<sid>.final.json`.** The task pre-decision (1) named the wrong path; the actual layout (set by Task 22's `start_session`, line 394) puts per-session artefacts under `<data_dir>/agents/<sid>/`. The cold-path classifier in `adopt::cold_path_one` reads from there and tolerates absence (host vanished without writing the file → marked `crashed` + `ended_at = now`).
  - **Auto-resume sweep is in `adopt.rs`, not a separate sweep.** The task allowed splitting; I folded the cold-path scan into `adopt_orphans` after the hot pass so a single boot-time sweep handles both halves. After the socket scan, a separate SQL pulls every `starting|running|awaiting` row that the hot pass did NOT re-attach, classifies it via `final-info.json`, and (if `crashed`) calls `cold_resume::maybe_auto_resume`. The auto-resume opt-in is read from `projects.settings_json.auto_resume_agents` via a JOIN in `read_auto_resume_for_session`.
  - **Auto-resume gating integration test skipped per pre-decision (9).** The unit-level coverage (`read_auto_resume_for_session` returns false on missing key, true on `{"auto_resume_agents": true}`) plus the explicit `cold_resume_session` happy-path test (which proves the spawn cycle works end-to-end) cover the behaviour without a second integration test that would need a full Core+host restart with synthetic `projects.settings_json` mutation. The gating SQL JOIN is short and shared with the happy-path code, so it's exercised whenever the auto-resume branch fires.
  - **`agent-host` already accepts `--resume-jsonl`; this task only wires the Core forwarding.** Task 21's CLI parameter is named `--resume-jsonl` (historical name from when the on-disk artefact was thought to be a JSONL slice). The agent-host already forwards a plain `--resume <token>` to the wrapped CLI (`crates/agent-host/src/main.rs:254`). This task added an optional `resume_jsonl: Option<&str>` param to `spawn_host` and a new field `resume_session_id: Option<String>` on `StartSessionRequest`; updating all 5 existing constructors took a single chained edit.
  - **`AgentSupervisorHandle::cold_resume_existing` duplicates ~120 lines of `start_session`.** The post-handshake half (parser pack, broadcast channels, pump spawn) is structurally identical; I considered extracting a `wire_up_pump` helper but every captured variable is named slightly differently and the borrow shape across the writer/child Arcs is delicate enough that a single inlined function reads more cleanly than a 9-argument helper. V1.0 has room to factor this out as part of the session-state-machine cleanup.
- **Open questions for next task:**
  - The V0.1 Claude parser pack doesn't yet extract `external_session_id` from the agent's first banner — sessions started in production today will still error with `session.no_external_id` if cold-resumed. Phase 3 parser polish (Task 33's follow-on) should add a `system: init` regex to `ClaudeCodePack` that writes the row via `concerto_persist::sessions::set_external_session_id`. Helper is in place; the parser just needs to call it.
  - The cold-path classifier (`adopt::cold_path_one`) treats `exit_code == Some(0) && signal.is_none()` as `finished`; non-zero exits and signals as `crashed`. If a future Task wants to surface the distinction (e.g. crash-loop detection), `cold_path_one` is the single hook point — it already reads the full `FinalInfo` projection.
- **Deliberate debt:**
  - UI "Resume agent" chip is V1.0 Desktop work — gRPC surface is wired (`Sessions.ColdResumeSession`); the chip just needs a button.
  - The continuation strategy is "reuse row" — chat history stays on the original row; in V1.0 we may want a separate "session_episodes" table that tracks each resume cycle.
  - `cold_resume_existing` does not detect whether the agent's JSONL file actually exists on disk before passing `--resume`; per task scope (out), we trust the agent CLI to error if its file is gone. The host's writer task will surface that as an early `AgentExited` and the cold-resume RPC will appear to succeed — the immediate `Exited` event flows to subscribers.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still exits 0 with "Smoke gate v2: PASSED".
