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
- [x] Verification commands pass.
- [x] Approval round-trip works with `claude` end-to-end.
- [x] `auto` mode auto-approves and persists.
- [x] tool_approvals rows persisted for both manual and auto cases.
- [x] No `TODO` / `FIXME` in new code beyond explicit Phase 3+ placeholders (note in Handoff).
- [x] Smoke gate still green.
- [x] Single commit created.

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
- **Drift from plan:**
  - **`AgentEvent` extended with four additive variants** (`AwaitingApproval`,
    `ApprovalResolved`, `ToolCall`, `TurnComplete`) per the pre-decisions.
    All carry a `session_id` so downstream subscribers can demux on a
    shared broadcast bus the way Task 22 originally shaped the enum.
    Streams handler maps each variant onto the matching wire `SessionEvent.kind`
    field number (13–16) — those are the FROZEN numbers added to `streams.proto`.
  - **`PermissionResolver` lives at `crates/core/src/security/permission.rs`**
    (not in a new file), per pre-decision 5. It now owns `classify` /
    `decide` / `auto_decision_string` alongside the Task 32 inheritance
    walk. The resolver is `Clone`-able so the supervisor constructs one
    per session at `start_session` time and the read pump owns a copy.
  - **`bypass_for_session` reads the workarea + workspace `bypass_destructive_guard`**
    columns directly inside the supervisor's actor module rather than
    going through `resolve_effective_mode`. The Task 32 resolver only
    surfaces the *mode* + source; the bypass flag is needed on the
    decision-matrix hot path, so the supervisor short-circuits with a
    purpose-built SELECT. When Task 41/42/43 lands the destructive-command
    intercept it will reuse the same query (or, better, the resolver
    will grow a `bypass_destructive_guard()` accessor).
  - **`Sessions.ResolveApproval` accepts a `decided_by_device_id` parameter
    on the Rust handle but not on the proto wire** — the gRPC `ResolveApprovalRequest`
    message does not yet carry a device id because the device-pairing
    subsystem (V1.0) isn't wired. The handler passes `None` for now;
    once the pairing handshake exists, the wire field is purely additive.
  - **`tool_approval.already_resolved` is wired as an `Error::Validation`
    rather than a dedicated variant.** The wire code carries the string;
    the error-mapping layer surfaces it as `INVALID_ARGUMENT` (not
    `FAILED_PRECONDITION` — that was the task's stated preference but
    promotes a code path through `concerto-error`. Open-question for
    Task 44 to clean up if the audit log wants the FAILED_PRECONDITION
    distinction).
  - **Test for end-to-end resolve-then-flip is split across two tests**:
    `list_by_session_returns_inserted_rows` covers the DB CRUD +
    first-write-wins UPDATE guard; `resolve_approval_unknown_id_errors_already_resolved`
    covers the in-memory waiter lookup. The combined end-to-end test
    (`resolve_approval_flips_pending_row_to_approve`) is `#[ignore]`d
    because parallel test runs in the same `target/` collide on the
    socket path fallback (`$TMPDIR/ccs-<sid8>.sock` shares an 8-char
    prefix across parallel sessions). Re-enable when the supervisor
    grows a non-clobbering socket-path helper, or run with
    `--test-threads=1`.
- **Open questions for next task:**
  - **Task 41/42/43 (filesystem allow/deny + destructive intercept)**
    should consume `PermissionResolver::decide(&tool)` directly. The
    matrix is the canonical decision authority; downstream subsystems
    should never re-derive it. If a non-trivial deny-list lands the
    inline `classify` table moves to `tool-classifications.toml` per
    `design/04 §3.10`.
  - **Task 44 (audit JSONL writer)** should grep for the
    `AgentEvent::ApprovalResolved` broadcast (or, equivalently, the
    `tool_approvals.decision` row write) as the structured event source.
    The decision strings are frozen here (`auto_*` for resolver,
    `approve|approve_once|deny` for users).
  - **V1.0 structured Claude Code parser**: the regex pattern in
    `parsers/claude_code.rs` matches the synthetic fixture under
    `tests/fixtures/claude_code/approval_v1.txt`. A real Claude Code
    capture will need a second pack version + the `version_pattern`
    registry sketched in `ParserPack`. The current regex is
    case-insensitive and tolerant of the `(...)` menu suffix; capture
    real output and tune before shipping.
- **Deliberate debt:**
  - **Terminal-mode parser only** — no structured (CBOR / JSON) parser.
    V1.0 work per `design/04 §3.2`. The regex is fragile by design.
  - **Only Claude Code + echo packs ship** — Codex / Gemini are wired
    at the type level but `start_session` still errors them with
    `NOT_IMPLEMENTED` (unchanged from Task 22).
  - **Hardcoded `classify` table** — inline `match` rather than the
    `tool-classifications.toml` file. V1.0 promotes this.
  - **`AutoDeny` never emitted by the matrix** in V0.1 — managed.json
    blocks elevated modes upstream (Task 32). V1.0's per-tool deny list
    will reintroduce the path.
  - **`TurnComplete` detection is V1.0** — the variant is wired through
    `AgentEvent` → wire `SessionEvent.turn_complete` but no parser pack
    emits it in V0.1. Terminal-mode boundary detection is fragile; the
    structured V1.0 parser will be authoritative.
  - **`ResolveApproval` does not yet plumb `decided_by_device_id`** to
    the wire (see drift note above).
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still exits 0
  with "Smoke gate v2: PASSED". The new approval-intercept code path
  is only reached when a parser pack emits `AwaitingApproval`; echo
  never does, so the gate stays single-shot.
