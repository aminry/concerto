# Maestro Live Conversation — Design

**Date:** 2026-06-12
**Status:** Approved (design phase)
**Branch:** `maestro-live-conversation` (stacked on `maestro-live-integration` / PR #178; rebases onto `main` once #178 merges)

## Problem

PR #178 landed the Maestro tool-serving foundation: the in-process MCP server serves the 11 read tools over a real bridge/UDS, the global Maestro session spawns at boot, `@composer` routing scopes to the viewed workspace, and `/digest` plumbing works. But the **conversational surface does not work** — typing "what are my workareas doing?" does nothing.

Root cause, found in Tier-3: the Maestro launches `claude` as an **interactive PTY/TUI**, but the Maestro UI is a **chat-bubble surface**. So:

- `forward_freeform` injects raw text into the PTY with **no submit** (no `\r`), so the message is never sent.
- Even if submitted, Claude's reply is **raw ANSI terminal output**, which the chat surface can't render.
- A one-time interactive **"trust this MCP server?"** prompt blocks the headless session.

Key discovery: the chat-bubble **rendering path already exists**. `apps/desktop/src/components/maestro/MaestroChat.tsx` subscribes to the `maestro.events` stream and renders `{kind:"message", text, role}` events as bubbles. What's missing is that the Core **never emits message events for a freeform turn** — because a TUI produces no clean turns. Concerto also already knows how to drive `claude` programmatically: the structured `stream-json` headless mode (`--input-format stream-json --output-format stream-json`) is exactly how agent harnesses run Claude with multi-turn + tool calls. The Maestro provider just doesn't use it.

## Approach (chosen)

**Reuse the agent-host; add a structured `stream-json` lane for the Maestro.** Switch the Maestro session from interactive TUI to headless `stream-json`, frame input as structured user-message envelopes, parse Claude's structured output into the `maestro.events` `message` stream the chat UI already renders, and auto-approve the live read tools. This reuses the entire session lifecycle (spawn, host-survival, cold-resume, the MCP socket) and the existing chat rendering; the net-new work is small and well-bounded. It also subsumes the trust-prompt gap (headless mode = no interactive prompts).

Rejected: a dedicated pipe-based runner that bypasses the agent-host (reinvents spawn/restart/cold-resume/MCP-wiring; loses host-survival); and keeping the TUI to scrape ANSI output (brittle — fights spinners/menus/prompts).

**Permission scope (decided):** minimal for this milestone. Only the 11 read tools are live (writes return typed-unimplemented), so auto-approve the read tools and **defer** the chip-gated write-permission flow to M2 where it belongs with the write tools.

## Components

Six well-bounded pieces, mostly modifying existing seams.

| # | Component | File | Change |
|---|---|---|---|
| 1 | Provider flags | `crates/core/src/maestro/provider.rs` | Launch spec adds `--input-format stream-json --output-format stream-json --verbose` (headless multi-turn, no TUI) + `--allowedTools mcp__concerto-maestro-mcp` (auto-approve the read tools). Keeps `--model`, `--mcp-config`, `--strict-mcp-config`, `--append-system-prompt`. NO `--dangerously-skip-permissions`. |
| 2 | MCP-trust preseed (gap #2) | `crates/core/src/maestro/` + the spawn path (`handle.rs::spawn_maestro_session`) | `ensure_maestro_mcp_trusted()` writes `enabledMcpjsonServers:["concerto-maestro-mcp"]` into `~/.claude.json` `projects.<scratch>`, mirroring `agent_supervisor::ensure_claude_trusts_dir`. Idempotent; called at spawn. Deterministic guarantee against any trust gate. |
| 3 | Input framing | `crates/core/src/maestro/handle.rs::forward_freeform` | Send a `stream-json` user-message envelope (`{"type":"user","message":{"role":"user","content":[{"type":"text","text":<body>}]}}` + `\n`) via `send_input`, instead of raw keystrokes. A pure `compose_user_envelope(body) -> String` helper is the testable unit. |
| 4 | Output parser | `crates/core/src/agent_supervisor/parsers/maestro.rs` | Replace the no-op `MaestroPack` with a `stream-json` parser. Buffers partial lines until newline; parses each JSON object; emits `ParseEvent`s (see Parser Contract). Pure + table-testable. |
| 5 | events bridge | Core (414's `maestro.events` publisher / the place that maps Maestro session `AgentEvent::Message` → `maestro.events`) | Publish the Maestro session's parsed assistant turns **and** the user's submitted turn onto `maestro.events` as `{kind:"message", role, text}` frames — the missing link. |
| 6 | History persistence | `handle.rs` + a small load path + `MaestroChat.tsx` | Persist the user turn to the maestro `chat_messages` (assistant turns already persist on `TurnComplete` via the checkpoint path). Load the maestro chat history when `MaestroChat` mounts so the conversation + digest survive reload. |

## Data flow

```
You type → MaestroComposer → SendToMaestro{text, workspace_id}
  → handle.send_to_maestro → pre_parse → Freeform
      ├─ persist user turn to maestro chat_messages
      ├─ publish maestro.events {message, role:user, text}   ──▶ MaestroChat bubble (your turn)
      └─ forward_freeform: stream-json user envelope ─▶ send_input ─▶ agent-host PTY stdin ─▶ claude
                                                                            │ (headless; MCP read tools live)
   claude emits stream-json events ─▶ PTY stdout ─▶ HostFrame::StdoutBytes ─▶ Core read-pump
      → MaestroStreamJsonPack parses → ParseEvent::Message (assistant text)
          ├─ publish maestro.events {message, role:assistant, text} ──▶ MaestroChat bubble (streamed reply)
          └─ on TurnComplete: persist assistant turn (existing checkpoint path)
```

Tool calls (`list_workspaces`, etc.) happen inside Claude's turn via the MCP loop already proven by the `maestro_e2e` test. In M1 they do not surface as bubbles (seam kept for later "used N tools" activity rendering).

## Parser contract

The one genuinely new unit. **In:** newline-delimited Claude `stream-json` objects from the session stdout. **Out:** `ParseEvent`s. Mapping (M1):

- `assistant` message text (`content[].text`, including streamed deltas) → `ParseEvent::Message{role:assistant, text}`
- `result` / turn-end object → `ParseEvent::TurnComplete` (drives existing assistant-turn persistence)
- `tool_use` / `tool_result` → swallowed in M1 (no bubble); kept as a seam
- unparseable / non-JSON / partial line → buffered until newline; a complete-but-unparseable line is logged + skipped (never panics, never blocks the stream)

The exact `stream-json` event schema (event `type` discriminants, delta vs full message) is validated by an early implementation spike against real Claude output captured to a fixture; the parser is then table-tested over that fixture.

## Permission & trust

- `--allowedTools mcp__concerto-maestro-mcp` auto-approves the live read tools. The 5 write + 2 side-channel tools remain typed-unimplemented, so nothing needs gating in M1.
- The MCP-trust preseed (component 2) guarantees no interactive trust gate.
- **M2 replaces** the blanket allow with the `--permission-prompt-tool` → `PermissionResolver` chip flow when the write tools land. No chip work here.

## Error handling

- Malformed / partial JSON line → buffer until newline; complete-but-unparseable → log + skip.
- Claude exits / crashes → existing supervisor restart + cold-resume re-spawns a fresh session; chat history persists in `chat_messages`.
- Budget/inert / disabled-by-policy → existing `guard_llm` path, unchanged.
- Quota exhausted → Claude emits an error `result` event; the parser surfaces it as an assistant `message` so the user sees the reason instead of silence.
- PTY-incompatibility spike fails → fall back to a pipe-mode variant in the agent-host (scoped contingency, not a rewrite).

## Testing

- **Parser unit tests** — table-driven over a captured `stream-json` fixture (the load-bearing tests).
- **Input-framing test** — `compose_user_envelope` emits a well-formed user envelope + newline.
- **events-bridge test** — a parsed assistant `Message` produces a `maestro.events {message}` frame.
- **Integration** — a scripted fake `stream-json` agent (echoes a canned assistant turn) drives the full loop; assert a bubble-shaped `message` event reaches the stream. CI-runnable, no real Claude.
- **Tier-3 manual** — real Claude: type → grounded streamed reply that used the read tools; reload → history persists.

## Scope / non-goals

- **In:** freeform chat works end-to-end (grounded by the read tools); trust preseed; user+assistant turn persistence + history-on-open; quota/error surfacing.
- **Out (M2):** write tools + confirmation chips; tool-activity bubbles; the full `--permission-prompt-tool` → `PermissionResolver` flow.
- **Separate follow-up (#3):** the digest LLM summarizer (the returning-user digest still uses the deterministic stub) — untouched here.

## First implementation spike (de-risk before the bulk)

Validate that Claude's `stream-json` mode runs cleanly inside the agent-host's **PTY** and capture a real output transcript to a fixture: spawn `claude --input-format stream-json --output-format stream-json --verbose --mcp-config … --strict-mcp-config --allowedTools mcp__concerto-maestro-mcp` through the existing spawn path, send one user envelope, and confirm parseable JSON events come back. Done sparingly to respect the user's Claude quota. If the PTY misbehaves, pivot component 1/4 to a pipe-mode agent-host variant.
