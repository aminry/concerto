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
- [ ] Verification commands pass.
- [ ] Echo session works end-to-end.
- [ ] Claude session works (manual test; documented in Handoff if `claude` not installed in your environment).
- [ ] No zombie processes after Stop.
- [ ] Resize works (drag the desktop window; terminal reflows).
- [ ] No `TODO` / `FIXME` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

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
- **Drift from plan:** —
- **Open questions for next task:** —
- **Deliberate debt:** no chat-style sub-tab, no tool-approval UI, no diff view. V0.1 is terminal-only.
- **Smoke-gate state:** unchanged.
