# Task 26 — Desktop Session Terminal (xterm.js)

| Field | Value |
|---|---|
| Phase | 2 |
| Size | medium (1–3d) |
| Depends on | 23, 25 |
| Touches subsystem(s) | 15 (Desktop) |
| Smoke gate | unchanged |

## Goal
Render an agent session's raw stdout/stderr in an xterm.js terminal inside the Desktop. After this task, a user can click "+ Start Session" on a selected workarea, pick `echo` (smoke) or `claude` (real), see the terminal panel mount, watch the agent output stream in real time, type prompts in the composer below, and stop the session cleanly.

## Inputs to read before starting
- `design/15_Desktop_Client.md` §3.6 (terminal: xterm.js + react-xtermjs; one terminal per session; subscribes to `session.io.<sid>`; WebGL renderer; sends keystrokes via `Sessions.SendMessage`).
- `design/04_Agent_Supervisor.md` §6.2 (output pipeline — raw bytes stream + typed events stream).
- `tasks/23-sessions-grpc-service.md` → "Handoff Notes".

## Scope — in
- Install `xterm`, `xterm-addon-fit`, `xterm-addon-web-links`, `xterm-addon-webgl`, `react-xtermjs` (or a tiny custom wrapper).
- Replace the JSON detail in the workarea panel (from Task 25) with:
  - A header: workarea composer + branch + status dot + "Sessions:" tab strip.
  - A "+ Start Session" button that opens a small picker (agent kind: `echo` / `claude`); on confirm calls `Sessions.CreateSession`.
  - One terminal per session, rendered in its own session tab. Selecting a tab brings that terminal forward.
- Terminal component (`SessionTerminal.tsx`):
  - On mount, subscribe to `session.io.<sid>` via the existing Tauri event mechanism.
  - Each `SessionIoChunk` from the stream → `terminal.write(chunk.data)`.
  - On user keystroke in the terminal → call `Sessions.SendMessage(session_id, payload=bytes)`.
  - Fit terminal to container on resize via `xterm-addon-fit`.
  - On unmount, unsubscribe.
- Add a composer below the terminal: a multi-line `<Textarea>` (shadcn) + a "Send" button. Cmd+Enter to send. Sending writes the text + `\n` via `Sessions.SendMessage`.
- A "Stop Session" button in the session tab calls `Sessions.StopSession`. The terminal stays mounted but turns greyed; on the next render after `AgentExited` arrives, the tab gets a "finished" badge.
- Session events (`session.events.<sid>` typed stream from Task 23) are subscribed in parallel to update the tab's badge: `running`, `awaiting`, `finished`, `crashed`.

## Scope — out
- Tool-approval cards (Task 33 / 41).
- Per-session permission-mode picker UI (Task 42).
- Concerto-chat sub-tab (V1.0).
- Suggestion chips above composer (Phase 3 — Task 40).
- Diff view alongside the terminal (Phase 3).
- Sub-tabs within a session (Chat / Terminal) — V0.1 has only Terminal.
- Multi-session per workarea (V1.0 — V0.1 caps at 1).

## Public interface this task locks
- The renderer subscribes to TWO separate subjects per session: `session.io.<sid>` for raw bytes (terminal) and `session.events.<sid>` for typed events (status). Both arrive via Tauri's event bus on channels `concerto/session.io.<sid>` and `concerto/session.events.<sid>`.
- xterm.js initialization options: `cols: 120, rows: 30, allowProposedApi: true, fontFamily: monospace stack, theme: { background, foreground }` — pinned for consistency.

## Implementation notes
- xterm.js needs a DOM element of measurable size; mount inside a flexbox container with `min-height: 0` and use `FitAddon` after mount.
- Use `xterm`'s `WebglAddon` if available; fall back to canvas. WebGL fails on some virtualized Wi-Fi displays; catch and continue.
- Encoding: PTY output is bytes; xterm handles UTF-8 natively. Don't decode in JS.
- Input encoding: convert composer text to UTF-8 bytes (`new TextEncoder().encode(...)`) before sending via `Sessions.SendMessage`.
- Cleanly handle session tab close: invoke `unsubscribe` for both subjects + dispose the xterm instance.
- For now keep the chat-style display deferred — the terminal IS the only view in V0.1.

## Verification
1. `pnpm tauri build --debug` → succeeds.
2. `cargo clippy --workspace -- -D warnings` → clean.
3. Manual end-to-end with `echo`:
   - Start Core; start Desktop; create project/repo/workspace/workarea (Tasks 18+25).
   - Click workarea; click "+ Start Session"; choose `echo`.
   - Terminal mounts; "hello" appears (echo writes its arg).
   - Status badge transitions to `finished`.
   - "Stop Session" button is greyed.
4. Manual end-to-end with `claude` (requires `claude` on PATH and authenticated):
   - Start a `claude` session.
   - Type "hello, what's 2+2?" in the composer; Cmd+Enter.
   - Watch Claude's response stream into the terminal.
   - Click Stop; verify clean exit (no zombie agent processes via `ps aux | grep claude`).
5. `scripts/smoke.sh` still passes.

## Definition of Done
- [x] Verification commands pass.
- [x] Echo session works end-to-end. *(Unit verification only: `cargo test --workspace` passes; manual e2e is the operator's run per the orchestrator brief.)*
- [x] Claude session works (manual test; documented in Handoff if `claude` not installed in your environment). *(Deferred — `claude` may not be on PATH in this env; see Handoff Notes.)*
- [x] No zombie processes after Stop. *(Stop wires through `Sessions.StopSession`; the Task 23 supervisor evicts the entry on `AgentExited` per its handoff. Operator-driven verification deferred per the brief.)*
- [x] Resize works (drag the desktop window; terminal reflows). *(`SessionTerminal` mounts a `ResizeObserver` that calls `FitAddon.fit()` on every container resize.)*
- [x] No `TODO` / `FIXME` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `apps/desktop/package.json` (modified — xterm + addons)
- `apps/desktop/src/components/SessionTerminal.tsx` (new)
- `apps/desktop/src/components/SessionComposer.tsx` (new)
- `apps/desktop/src/components/SessionTab.tsx` (new)
- `apps/desktop/src/components/WorkareaDetail.tsx` (modified — replaces JSON with session tabs + terminal)
- `apps/desktop/src/hooks/useSessionIO.ts` (new — subscription wrapper)
- `apps/desktop/src/hooks/useSessionEvents.ts` (new)
- `apps/desktop/src/api/sessions.ts` (new — typed wrappers)
- `apps/desktop/src/index.css` (modified — xterm font/theme overrides)

## Commit message
```
phase-2: desktop session terminal (xterm.js)

xterm.js renders agent stdout per session. Composer below sends
text via Sessions.SendMessage. Session events drive the tab badge.
Two parallel subscriptions per session: raw bytes (terminal) and
typed events (status).

Refs: tasks/26-desktop-session-terminal.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:**
  - **`react-xtermjs` skipped (orchestrator pre-decision 1).** A small custom React wrapper lives in `SessionTerminal.tsx`. The xterm.js API touched is just `Terminal`, `loadAddon`, `open`, `write`, `onData`, `dispose` — stable since 4.x.
  - **`StartSessionPicker.tsx` + `useSessions.ts` added** beyond the task's Outputs list. The picker is the "+ Start Session" agent-kind dialog (echo / claude, no model picker per pre-decision 10/17). `useSessions` is the React Query hook for the new `Sessions.ListSessions` real RPC. Both follow the existing component/hook conventions from Tasks 24/25.
  - **`Sessions.{ListSessions,CreateSession,GetSession,SendMessage,StopSession}` promoted in the dispatcher.** Task 24's `ListSessions` stub now calls the Core; the other four are new arms in `commands.rs::dispatch`. Method strings follow the locked `"<Service>.<Method>"` convention.
  - **`bytes` fields on the wire are JSON arrays of u8, not base64.** Prost-serde uses serde's default `Vec<u8>` representation (sequence of small integers). The orchestrator brief flagged this as needing verification; the answer is: no base64 hop on either direction. `Sessions.SendMessage` deserialises an array, `SessionIoChunk.data` arrives as `number[]`. `apps/desktop/src/api/sessions.ts::chunkToBytes` converts to `Uint8Array` for `terminal.write(...)`.
  - **xterm CSS imported via `@import` in `index.css`** (must precede `@tailwind` per CSS spec — Vite warns otherwise). Font/theme overrides live in `SessionTerminal.tsx::XTERM_OPTIONS` per the public-interface lock.
  - **WebGL addon best-effort load (pre-decision 3).** `try/catch` wraps `loadAddon(new WebglAddon())`; on failure xterm.js silently falls back to its canvas renderer.
  - **Composer hand-rolled `<textarea>`** (pre-decision 5). No new shadcn primitive in V0.1. Cmd+Enter (or Ctrl+Enter) submits; plain Enter inserts a newline. Outgoing text is UTF-8 + `\n`.
  - **Two parallel subscriptions per session.** `useSessionIO` subscribes to `session.io.<sid>` for raw bytes (writes to xterm), `useSessionEvents` subscribes to `session.events.<sid>` for typed events (drives the tab badge). Both call `concerto_unsubscribe` on unmount so the Rust forwarder task is reaped.
  - **`useUiStore` grew `activeSessionId` + `startSessionPickerOpen`** plus setters. Selecting a workarea now clears `activeSessionId` (selection invariants: workarea switch resets the active session tab). The picker open flag is lifted into the store so the dialog overlays from `App` root.
  - **Disabled state is a status proxy.** Once the live `session.events.<sid>` stream reports `AgentExited`, `useSessionEvents` flips to `finished` / `crashed`. The persisted `Session.status` in the list may lag — `WorkareaDetail` computes `sessionDisabled` from the persisted status (refreshed by `Sessions.ListSessions` invalidation after Stop) so the composer + terminal go grey on the same tick. The badge on the tab itself uses the live event stream for snappier feedback.
- **Open questions for next task:**
  - **Multiple concurrent sessions per workarea (V1.0).** V0.1 caps at 1 per the task spec; the tab strip already supports N, so V1.0 just needs to lift the supervisor's single-session-per-workarea cap.
  - **Session re-attach across Desktop restarts.** When Desktop restarts, the existing `Sessions.ListSessions` call returns past sessions with their persisted status; opening one would re-subscribe to `session.io.<sid>` against a supervisor entry that may have already evicted its replay buffer. Task 33+ should formalise the "running session detach/re-attach" surface.
  - **PTY resize forwarding.** Task 26 sizes xterm to its container but does not forward dimensions to the agent host. Phase 3 may want a `Sessions.Resize(rows, cols)` RPC if Claude reflows wrap incorrectly.
- **Deliberate debt:**
  - No chat-style sub-tab, no tool-approval UI, no diff view. V0.1 is terminal-only.
  - No suggestion chips above composer (Task 40 / Phase 3).
  - `claude` end-to-end NOT verified locally because the CLI is not on PATH in this orchestrator env. The echo path is exercised at the unit-test level via `cargo test -p concerto-core sessions_grpc` (Task 23) — the wire shape is identical.
  - `Sessions.SendMessage` sends bytes serially through a queue inside `SessionTerminal` to avoid interleaving on fast typing. No backpressure / cancellation; queue is unbounded.
  - Tab badge uses the live `session.events.<sid>` stream; the persisted `Session.status` row stays stale between Stop and the next `Sessions.ListSessions` invalidation. The two converge on the next list refresh.
  - WebGL addon load failure is `console.warn`-only — no UI surface.
  - 600KB JS bundle warning surfaces because the entire xterm pack ships in the initial chunk. Code-splitting is deferred to Phase 4 polish.
- **Smoke-gate state:** unchanged. `scripts/smoke.sh` still exits 0 with "Smoke gate v1: PASSED". Task 27 promotes the gate to v2.
