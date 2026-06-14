# Maestro Live-Integration — Design

**Date:** 2026-06-11
**Status:** Approved (design phase)
**Branch:** `maestro-live-integration` (off `origin/main` @ ac9769f)

## Problem

Phase 4 built every Maestro part — the 18-tool MCP server, the provider/CLI launch
spec, the routing pre-parser, summary cache, digest, gRPC service, and desktop UI —
and unit-tested each in isolation (159 backend + 207 desktop tests pass). But running
the real desktop app revealed the live agent loop was **never connected end to end**.
A user who opens the app and types into the Maestro composer hits two failures:

- `@Graphify what's up?` → `not_found: maestro.routing: NoSuchWorkarea` — routing
  resolves the composer name in the *most-recent* workspace, not the one being viewed.
- `what are my workareas doing?` → `not_found: no live Maestro session to forward to`
  — no Maestro CLI session was ever spawned.

The Maestro is, in effect, dead on arrival.

## The seam inventory

Seven disconnected seams (one already fixed live, uncommitted):

| # | Seam | State |
|---|------|-------|
| 1 | `serve_maestro_mcp` is only ever called from a test — never bound to a live transport | unwired |
| 1b | `MaestroMcpServer` is `Default` with no Core handles; `call_tool` routes to the **handle-less typed-unimplemented stub** `tools::dispatch()`, not the real async `dispatch_read`/`dispatch_write`/`dispatch_side` | unwired (the large one) |
| 2 | The `.mcp.json` the CLI dials via `--mcp-config` is never written (a comment blames Task 414, which never did it) | unwired |
| 3 | The `AgentKind::Maestro` session is never spawned — boot builds `MaestroHandle::new` but never starts a session | unwired |
| 4 | `sessions.workarea_id TEXT NOT NULL REFERENCES workareas(id)` — the global Maestro has no host workarea | conflict |
| 4b | `maestro_session_id()` looks up `chats.kind='maestro'`, but `start_session` creates `kind='session'` chats | mismatch |
| 5 | `MaestroMessageRequest` has no `workspace_id` → routing can't scope to the viewed workspace | unwired |
| 6 | Tauri shell `rpc.rs` Maestro.* dispatch arms | fixed live, uncommitted |

**Key insight on seam 1b:** the 18 tool *bodies* already exist (Tasks 405/406/407 wrote
`dispatch_read`/`dispatch_write`/`dispatch_side` — async, Core-handle-bearing). They are
simply unreachable: the live `ServerHandler::call_tool` → `dispatch_tool` →
`tools::dispatch()` is the handle-less stub that returns a typed-unimplemented error.
Wiring the read tools means giving `MaestroMcpServer` the Core handles and routing
`call_tool` to the async dispatchers.

## Architecture decisions

### A — MCP transport bridging (the crux)

The design pins an **in-process rmcp stdio server in the Core**. But the Claude CLI's
`--mcp-config` only understands two server kinds: a **stdio command it spawns as a
subprocess**, or an **HTTP URL**. It cannot be handed a pre-opened pipe to an
in-process server. Something must bridge the CLI's spawn-a-subprocess model to the
Core's in-process server.

**Decision: A1 — dumb stdio bridge over a dedicated UDS.**

- The Core binds `~/.concerto/maestro-mcp.sock`, runs an accept loop, and serves
  `serve_maestro_mcp(AsyncRwTransport::new(read, write))` over each accepted connection.
- `.mcp.json` points the CLI at a tiny bridge command that copies bytes between its
  stdin/stdout and that socket. The bridge needs **zero MCP knowledge** — MCP stdio
  framing (newline-delimited JSON-RPC) flows transparently over any byte pipe.
- The bridge is a subcommand on the already-shipped `concerto-agent-host` binary
  (`concerto-agent-host mcp-bridge --socket <path>`). Boot already resolves that
  binary's absolute path to spawn sessions, so we write that path into `.mcp.json`.

**Why:** honors the stdio-framing pin; reuses the UDS substrate already used for Core
gRPC; UDS file permissions (0600) make it local-only with **no auth token required**;
and it is a near-drop-in — the existing test already serves over `AsyncRwTransport`, so
we swap the in-memory pipe for an accepted `UnixStream` half.

Rejected: **A2 loopback HTTP** (departs from the stdio pin; binds a TCP port → macOS
firewall prompts; needs an auth token). **A3 standalone MCP-server binary calling Core
over gRPC** (contradicts the in-process design, duplicates the server, turns all 18
tools into gRPC round-trips).

### B — Host-workarea model (satisfying the NOT NULL FK)

**Decision: B1 — reserved hidden system workspace + workarea.**

- Boot ensures a sentinel workspace + workarea (reserved id, e.g. `__maestro__`) exist.
- The Maestro session FKs to that workarea, satisfying `sessions.workarea_id NOT NULL`.
- UI list queries filter the sentinel id so it never appears to the user.

**Why:** zero schema change; the supervisor's existing `start_session` workarea
validation passes unchanged; session rows stay uniform. Cost is a UI filter + an
id convention — cheap.

Rejected: **B2 make `workarea_id` nullable** (migration touching a core invariant and
every join assuming NOT NULL; wide blast radius). **B3 track the Maestro session outside
the `sessions` table** (the supervisor is built around `sessions`).

Paired fix for **4b**: the Maestro gets its own spawn path that creates a
`kind='maestro'` chat and binds the session to it (the `chats` CHECK already permits
`kind='maestro'` with a NULL `session_id`), rather than the generic `start_session`
which forces `kind='session'`.

### C — Spawn timing

**Decision: C1 — spawn at boot, degrade to inert if no provider/CLI.**

After building `MaestroHandle`, boot (gated on the existing enabled + policy check):

1. ensures the system workspace/workarea + `kind='maestro'` chat,
2. binds the `maestro-mcp.sock` + accept loop,
3. writes `.mcp.json` into the scratch cwd,
4. spawns the Maestro session via the Maestro-specific spawn path.

If no CLI provider resolves, leave the Maestro **inert** — the `GetState.inert_reason`
surface already exists to show that in the UI.

**Why:** matches design/08's "long-lived Maestro session"; gives the digest +
daily-condensation background loops a session to run against; keeps `forward_freeform`
dead-simple. Rejected **C2 lazy-on-first-message** (cold-start latency on the first
chat; every background loop would also have to trigger-spawn).

### D — Workspace scope on the wire

**Decision: D1 — add an optional `workspace_id` to `MaestroMessageRequest`.**

- Desktop passes the active workspace as a **scope hint**.
- Routing resolves bare `@composer` names within that workspace first, still allowing
  cross-workspace; falls back to `default_workspace_id()` when absent.
- Additive proto change (backward compatible).

**Why:** directly fixes the `@Graphify NoSuchWorkarea` bug. The Maestro stays global
(can route across workspaces); the hint just sets the default scope for unqualified
names.

## Scope: two milestones

The routing pre-parser (`@composer …`) is deterministic and runs *before* the LLM;
freeform chat needs the live LLM session. This cleaves the work cleanly:

### Milestone 1 (this build) — live read-capable Maestro

A1 + B1 + C1 + D1, plus:

- the chat-kind fix (4b),
- thread the **read-tool** Core handles into `MaestroMcpServer` and route `call_tool`
  to the async `dispatch_read` (the 11 read tools),
- formalize + commit the `rpc.rs` Maestro.* dispatch fix (seam 6),
- the end-to-end test that was missing (see Testing).

**Result:** the user chats → a live Claude session answers using the 11 read tools;
`@composer` routing works in the right workspace; `/digest` works. The 5 write MCP
tools + 2 side-channel tools still return their typed-unimplemented error — **safe**,
they don't panic.

### Milestone 2 (follow-up) — write tools + confirmation chips

Thread `dispatch_write` + `dispatch_side` + the confirmation-chip sink so the LLM can
create / route / fanout / pause through confirmation chips, plus create-workspace-from-
description via the Maestro. Out of scope for this spec; gets its own spec → plan cycle.

## Components touched (Milestone 1)

| Component | Change |
|---|---|
| `crates/agent-host` (bin) | New `mcp-bridge --socket <path>` subcommand: dumb bidirectional byte copy between stdio and the UDS. |
| `crates/core/src/maestro/mcp.rs` | `MaestroMcpServer` gains Core handles (read-tool subsystems); `call_tool` routes to async `dispatch_read`; a `serve` entry that binds + accept-loops the dedicated UDS. |
| `crates/core/src/maestro/mod.rs` | `.mcp.json` writer (bridge command + abs agent-host path + socket path). |
| `crates/core/src/boot.rs` | Ensure system workspace/workarea + maestro chat; bind MCP socket + accept loop; write `.mcp.json`; spawn the Maestro session; degrade-to-inert path. |
| `crates/core/src/maestro/handle.rs` | Maestro-specific spawn (creates `kind='maestro'` chat, binds session); `default_workspace_id` honors the wire `workspace_id` hint. |
| Persistence / system-workspace helper | Ensure-sentinel-workspace/workarea; UI list queries filter the sentinel. |
| `crates/proto/.../maestro.proto` | Add optional `workspace_id` to `MaestroMessageRequest`; regenerate. |
| `apps/desktop/src-tauri/src/rpc.rs` | Formalize the live Maestro.* dispatch arms; pass `workspace_id`. |
| `apps/desktop/src` | Pass the active workspace id into `SendToMaestro`; filter the sentinel workspace from lists. |

## Data flow (the live loop, Milestone 1)

```
Desktop composer
  │  SendToMaestro { text, workspace_id }
  ▼
Tauri shell rpc.rs ──gRPC──▶ Core MaestroService.SendToMaestro
  │
  ├─ routing pre-parser (408): "@composer …"? ──▶ deterministic route (scoped by workspace_id)
  │
  └─ else freeform ──▶ MaestroHandle.forward_freeform ──▶ supervisor.send_input(maestro_session_id)
                                                              │ (PTY stdin to the Claude CLI)
                                                              ▼
                                            Claude CLI ──spawns──▶ agent-host mcp-bridge ──UDS──▶ Core maestro-mcp.sock
                                                              │                                       │
                                                              │  MCP JSON-RPC (list_tools/call_tool)  │
                                                              ▼                                       ▼
                                                    --strict-mcp-config           serve_maestro_mcp → MaestroMcpServer
                                                    (only 18 tools visible)        → dispatch_read (live, handle-bearing)
                                                              │                                       │
                                                              ◀───────────── tool result ────────────┘
                                            Claude streams reply ──PTY──▶ supervisor ──▶ maestro.events ──▶ desktop
```

## Error handling

- **No CLI provider** → Maestro stays inert; `GetState.inert_reason` surfaces it; boot
  does not fail. No panic.
- **MCP socket bind fails** → boot logs + leaves Maestro inert (don't crash the Core).
- **Bridge can't reach the socket** → the CLI sees the MCP server as unavailable; tools
  fail loudly via MCP error; chat still functions for non-tool turns.
- **Write/side tools called in M1** → the existing typed-unimplemented MCP error (no
  panic, no empty-success) — unchanged.
- **Budget exhausted / disabled-by-policy** → existing `guard_llm` inert path.

## Testing

- **Bridge unit test:** bytes written to one end appear at the other, both directions;
  clean EOF on socket close.
- **MCP server wiring test:** a connected MCP client over the dedicated UDS sees all 18
  tools in `list_tools`, and a read tool (e.g. `list_workspaces`) returns live Core data
  (not the typed-unimplemented error). This is the test that would have caught seam 1b.
- **Boot integration test:** with a fake/echo CLI provider, boot spawns a Maestro
  session bound to the system workarea; `maestro_session_id()` resolves it.
- **Routing scope test:** `@composer` with a `workspace_id` hint resolves in that
  workspace; a name absent there but present elsewhere yields the existing
  `NoSuchWorkarea` (not a silent cross-workspace match).
- **End-to-end (the missing test):** drive `SendToMaestro` → assert a freeform turn
  reaches the session and a read-tool turn returns live data. Uses a scripted fake CLI
  provider so CI needs no real Claude binary.
- **Manual (Tier-3):** the real desktop app + the real Claude CLI — chat, `@route`,
  `/digest` — the gate that caught these seams in the first place.

## Non-goals

- Write MCP tools + confirmation chips (Milestone 2).
- Create-workspace-from-description via the Maestro (Milestone 2).
- Codex/Gemini/Direct-API backends beyond what already resolves via `select_provider`.
- Any schema change to `sessions` / `workspaces` / `workareas`.
- Multi-workspace ask-with-chips routing (design/08 §3.5 cross-workspace branch).
