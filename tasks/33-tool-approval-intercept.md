# Task 33 — Tool-Approval Boundary Detection + PermissionResolver

| Field | Value |
|---|---|
| Phase | 3 |
| Size | medium (1–3d) |
| Depends on | 22, 23, 32 |
| Touches subsystem(s) | 04 (Agent Supervisor) |
| Smoke gate | unchanged |

## Goal
Detect when an agent CLI pauses for a tool-approval decision, route the decision through the `PermissionResolver` (from Task 32) for auto-approve/auto-deny, raise an `AwaitingApproval` event to clients only when the resolver returns `MustAsk`, and inject the user's decision back into the agent's stdin. Persist every approval (auto and manual) to the `tool_approvals` table.

## Inputs to read before starting
- `design/04_Agent_Supervisor.md` §3.2 (parser strategies: structured vs terminal), §3.3 (tool approval flow with PermissionResolver consultation), §3.10 (mode → decision mapping), §6.3 (approval injection).
- `design/09_Persistence.md` §4.2 (`tool_approvals` schema).
- `tasks/32-permission-mode-inheritance.md` → "Handoff Notes".

## Scope — in
- Implement `crates/core/src/agent_supervisor/parsers/`:
  - `mod.rs` exposing a `ParserPack` trait:
    ```rust
    pub trait ParserPack: Send + Sync {
        fn agent_kind(&self) -> AgentKind;
        fn version_pattern(&self) -> &str;             // regex on agent --version output
        fn parse_chunk(&self, buf: &mut Vec<u8>) -> Vec<ParseEvent>;
        fn inject_approval(&self, decision: Decision) -> Vec<u8>;   // bytes to write to stdin
    }
    pub enum ParseEvent {
        Bytes(Vec<u8>),           // raw stream pass-through
        Message { role: MsgRole, content: String },
        ToolCall { name: String, args: serde_json::Value, call_id: String },
        AwaitingApproval { tool: String, summary: String, payload: serde_json::Value },
        TurnComplete,
    }
    ```
  - `claude_code.rs` — implements `ParserPack` for Claude Code. V0.1 implementation: terminal-mode regex pack that detects Claude Code's tool-approval prompts. The regex patterns are versioned by Claude Code's `--version`. For V0.1 ship one pack matching the version Claude Code prints when this task is implemented; gate "unknown version" with a warning banner.
  - `echo.rs` — trivial pack that emits a `Message` event per chunk and never `AwaitingApproval`.
- Extend `Session` struct (Task 22) to hold:
  - `parser: Box<dyn ParserPack>`,
  - `permission_resolver: PermissionResolver`,
  - `awaiting_approval: Option<ApprovalCtx>`.
- Implement `PermissionResolver` per `design/04 §3.10` algorithm:
  - Looks up tool classification (V0.1 inline-table; full `tool-classifications.toml` is V1.0).
  - Decision: `AutoApprove`, `AutoApproveOnce`, `MustAsk`, `AutoDeny`.
- In the supervisor's per-session bridge-read task:
  - Each `StdoutBytes` frame → call `parser.parse_chunk`.
  - For each `ParseEvent::AwaitingApproval`:
    - Build a `ToolCall { name, payload }`.
    - Run resolver. If `AutoApprove` / `AutoApproveOnce`: persist a `tool_approvals` row with `decision = "auto_<mode>"` and inject the approve bytes via `parser.inject_approval`.
    - If `AutoDeny`: similar with deny bytes.
    - If `MustAsk`: persist a row with `decided_at = NULL`; emit `AgentEvent::AwaitingApproval`; store the waiter `oneshot::Receiver<Decision>` in the session.
  - For each `ParseEvent::TurnComplete`: emit `AgentEvent::TurnComplete`.
  - For each `ParseEvent::Message`: emit `AgentEvent::Message` (Task 22 placeholder is replaced by the typed path here).
- Implement `resolve_approval(approval_id, decision, by_device)` in the supervisor:
  - Look up the in-memory waiter; send the decision; persist `tool_approvals.decided_at`, `.decided_by_device_id`, `.decision`.
  - Reject with `AlreadyResolved` if the row was already decided (first-write-wins).
- Add proto + handler: `Sessions.ResolveApproval(ResolveApprovalRequest { session_id, approval_id, decision })`.
- Persist `tool_approvals` rows in `crates/persist/src/tool_approvals.rs` (new module): `insert`, `update_decision`, `get`, `list_by_session`.
- Update `SessionEvent.kind` oneof in `streams.proto` with new variants: `AwaitingApproval`, `ApprovalResolved`, `ToolCall`, `TurnComplete`.
- Tests:
  - Echo session: no approvals; `TurnComplete` arrives after the echo string.
  - Stub `claude_code` parser with a fixture stdin/stdout transcript: assert `AwaitingApproval` raised; resolve it; assert injection bytes written.
  - PermissionResolver: in `auto` mode, the same fixture yields `AutoApprove` and writes a tool_approvals row with `decision = "auto_auto"`.

## Scope — out
- Structured-mode parser (V1.0 — `design/04 §3.2` says terminal is V0.1 default).
- Codex / Gemini parser packs (V1.0).
- Multi-device approval fan-out via push (V1.0 — depends on subsystem 14).
- Per-tool classification toml file (V1.0 — V0.1 hardcodes the table).
- MCP project-level write (Task 35 handles MCP read).

## Public interface this task locks
- Rust: `crates/core/src/agent_supervisor/parsers/mod.rs::ParserPack` trait. Method signatures FROZEN.
- Proto: new `SessionEvent.kind` variants `AwaitingApproval`, `ApprovalResolved`, `ToolCall`, `TurnComplete` (field numbers ≥ 13 — preserve V0.1's earlier numbers).
- Proto: `Sessions.ResolveApproval` RPC + `ResolveApprovalRequest`.
- `tool_approvals` schema is already locked from Task 09; this task makes it functional.

## Implementation notes
- The Claude Code prompt detection is regex on the rendered output — fragile by design (per the doc). Test with real `claude` output captured to a file.
- `inject_approval` for Claude Code: write `"y\n"` for approve, `"n\n"` for deny, `"2\n"` for "approve once" (Claude's typical menu). Get the exact menu mapping from the captured transcript.
- The waiter is `oneshot::Sender<Decision>` stored in a HashMap keyed by `approval_id`. Drop on session end with `AwaitingApproval { reason: SessionEnded }`.
- For the per-session bridge-read task, ensure parser state survives between `StdoutBytes` chunks — the `buf: &mut Vec<u8>` is the parser's own buffer that accumulates partial lines.

## Verification
1. `cargo build --workspace` → succeeds.
2. `cargo test -p concerto-core tool_approval` → tests pass (fixture-based).
3. `cargo clippy --workspace -- -D warnings` → clean.
4. Manual with real `claude`: spawn a session that triggers a tool approval (e.g., asks to write a file); verify `AwaitingApproval` event arrives at the client; resolve via gRPC; verify the agent continues.
5. Manual with `auto` mode: same scenario; verify the approval is auto-approved and the tool_approvals row has `decision = "auto_auto"`.
6. `./scripts/regen-interfaces.sh && git diff` → committed.
7. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] Approval round-trip works with `claude` end-to-end.
- [ ] `auto` mode auto-approves and persists.
- [ ] tool_approvals rows persisted for both manual and auto cases.
- [ ] No `TODO` / `FIXME` in new code beyond explicit Phase 3+ placeholders (note in Handoff).
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/core/src/agent_supervisor/parsers/mod.rs` (new)
- `crates/core/src/agent_supervisor/parsers/claude_code.rs` (new)
- `crates/core/src/agent_supervisor/parsers/echo.rs` (new)
- `crates/core/src/agent_supervisor/approval.rs` (new)
- `crates/core/src/agent_supervisor/actor.rs` (modified — parser dispatch + resolver wire)
- `crates/persist/src/tool_approvals.rs` (new)
- `crates/persist/src/lib.rs` (modified)
- `crates/proto/proto/concerto/v1/sessions.proto` (modified — ResolveApproval RPC)
- `crates/proto/proto/concerto/v1/streams.proto` (modified — SessionEvent.kind variants)
- `crates/core/src/handlers/sessions.rs` (modified)
- `crates/core/tests/tool_approval.rs` (new)
- `crates/core/tests/fixtures/claude_code/approval_v*.txt` (new — captured transcripts)
- `docs/interfaces/proto.md`, `rust-api.md` (regenerated)

## Commit message
```
phase-3: tool-approval boundary detection + PermissionResolver

ParserPack trait with Claude Code + echo implementations. Per-chunk
parsing emits AwaitingApproval events; PermissionResolver consults
the inheritance-resolved mode (Task 32) and either auto-decides or
raises to the user. All tool calls persist a tool_approvals row.

Refs: tasks/33-tool-approval-intercept.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** terminal-mode parser (structured V1.0), only Claude Code pack, hardcoded tool-classifications table.
- **Smoke-gate state:** unchanged.
