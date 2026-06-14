# Maestro Pipe-Mode Spawn — Design

**Date:** 2026-06-13
**Status:** Approved (design phase)
**Branch:** `maestro-live-conversation` (the conversation milestone; pipe-mode is its final enabling piece)

## Problem

The conversation milestone switched the Maestro `claude` session to headless `stream-json` mode. Tier-3 live testing revealed a hard blocker: `claude` refuses to run `stream-json` over a PTY.

- `claude --input-format stream-json` errors with `--input-format=stream-json requires --print` → fixed by adding `--print` (committed).
- `claude --print --input-format stream-json` then errors with `Input must be provided either through stdin or as a prompt argument when using --print`.

Root cause: `claude`'s streaming mode requires its **stdin to be a pipe** (non-TTY). But Concerto's **agent-host always spawns agents in a PTY** (`portable_pty`), so `claude` sees a TTY on stdin and refuses to stream. The session errors and exits on every spawn (exit 1, before any API call — no quota burned).

This is the contingency the conversation spec anticipated: *"If the PTY misbehaves, fall back to a pipe-mode variant in the agent-host."* Live-testing confirmed it is not a fallback — it is **required** for the structured chat to work.

## Key insight (why this is small)

The agent-host's I/O is transport-agnostic. After spawn, `run_pty_session` runs three threads: a **reader** (child stdout → `HostFrame::StdoutBytes`), a **writer** (`stdin_rx` → child stdin), and a **resize** thread. The frames (`StdinBytes`/`StdoutBytes` over the CBOR-UDS channel) are pure byte-forwarding, so **the entire Core side is unchanged** — the read-pump, `MaestroStreamJsonPack`, the events bridge, persistence, and UI all stay exactly as built. A PTY master exposes `Box<dyn Read+Send>` / `Box<dyn Write+Send>` halves (`try_clone_reader`/`take_writer`); a piped child's `ChildStdout`/`ChildStdin` box into the same types. So pipe-mode reuses the same pump logic over piped handles instead of a PTY master.

## Approach (chosen)

**Add a pipe-mode spawn to the agent-host, gated by a CLI flag, selected by the Core only for `AgentKind::Maestro`; share the pump logic between PTY and pipe sessions.**

Rejected: a full duplicate `run_pipe_session` (≈100 lines of drift-prone duplication); a `trait IoBackend` abstraction (over-engineered for two modes); a dedicated Maestro runner outside the agent-host (reinvents host-survival/cold-resume — rejected back in the conversation brainstorm).

## Components

1. **agent-host CLI flag** (`crates/agent-host/src/main.rs`, the `Cli` struct): add an optional `--io-mode <pty|pipe>` defaulting to `pty`. Additive — existing callers pass nothing and get byte-identical PTY behaviour, so the Task-21 "locked Cli" contract holds (nothing existing changes/removes).
2. **`run_pipe_session`** (new, sibling of `run_pty_session`): spawns the child via `std::process::Command` with `stdin = Stdio::piped()`, `stdout = Stdio::piped()`, `stderr = Stdio::piped()`. Runs the shared reader pump (child stdout → `StdoutBytes`) + shared writer pump (`stdin_rx` → child stdin) + a stderr-drain thread + `child.wait()`. Ignores `resize_rx` (pipes don't resize).
3. **Shared pump helpers**: extract the reader-thread body and writer-thread body (which already operate on boxed `Read`/`Write` handles) so both `run_pty_session` and `run_pipe_session` call them. Each helper takes the boxed handle + the existing channel/state. This is the only change to the existing PTY path — it keeps calling the same logic, now via the helper.
4. **Core spawn selection** (the agent-supervisor host-launch site that builds the `concerto-agent-host` argument vector — `crates/core/src/agent_supervisor/spawn.rs` or `actor.rs`): append `--io-mode pipe` to the host args when `agent_kind == AgentKind::Maestro`; all other kinds stay `pty` (omit the flag → default).

## Data flow

Unchanged everywhere except the agent-host's child-stdio source:

```
Core start_session (AgentKind::Maestro) → host args include `--io-mode pipe`
  → concerto-agent-host spawns claude via Command{ stdin=pipe, stdout=pipe, stderr=pipe }
  → claude sees non-TTY stdin → `--print --input-format stream-json` streaming multi-turn, stays alive
  → writer pump: StdinBytes (user-message envelopes) → child stdin pipe
  → reader pump: child stdout pipe → StdoutBytes frames → Core read-pump → MaestroStreamJsonPack → events bridge → maestro.events → UI
  → stderr pump: child stderr pipe → host log + final-info last-lines (diagnostics only)
```

## Error handling

- Child **stderr** → drained on its own thread to the host's log (`tracing`) + folded into the `FinalInfo` last-lines, so the next process-level failure (a flag error, a crash) is diagnosable. NOT surfaced in the chat UI (Q1-B decision; a UI surface is a later nice-to-have).
- **Spawn failure** (binary missing, exec error) → the existing `FinalInfo`/error path the PTY spawn already uses, adapted for `std::process::Command`'s error.
- **Resize** requests in pipe mode → the `resize_rx` is drained and discarded (no-op); no resize thread is started.
- The child-wait + kill path differs by type (`portable_pty::Child` vs `std::process::Child`); each session owns its wait/kill, the shared pumps do not.

## Testing

- **Pipe round-trip integration test** (agent-host): launch the host in `--io-mode pipe` against a scripted stdin→stdout agent (a `cat`-style or small echo bin), write `StdinBytes` over the UDS, assert the bytes return as `StdoutBytes`. Proves the pipe pumps + framing work with no PTY. (Mirror the existing `agent_spawn`/`echo_round_trip` harness.)
- **PTY regression guard**: the existing `agent_spawn`, `cold_resume`, `hot_reconnect` tests (PTY/echo path) must stay green — they exercise the unchanged PTY entry point through the now-shared pump helpers.
- **Tier-3 (real `claude`)**: boot the Core, confirm the Maestro session **stays alive** (a `claude --print --input-format stream-json` process persists), then send a freeform message and get a streamed grounded reply — the live test that has been failing.

## Scope / non-goals

- Pipe mode is gated to `AgentKind::Maestro` only; every other session stays PTY (the interactive terminal UX is unchanged).
- No stderr-in-UI (Q1-B): stderr is log-only.
- No change to host-survival or cold-resume: those ride the agent-host↔Core UDS + `claude --resume`, both independent of the child's stdio transport. (The pump-helper extraction must not alter the PTY path's behaviour — the regression tests are the guard.)
- No change to the Core read-pump, parser, events bridge, persistence, proto, or desktop — all built in the conversation milestone and untouched here.
