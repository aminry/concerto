# Task 21 — `concerto-agent-host` Helper Binary

| Field | Value |
|---|---|
| Phase | 2 |
| Size | medium (1–3d) |
| Depends on | 11, 12 |
| Touches subsystem(s) | 04 (Agent Supervisor), 01 (Runtime) |
| Smoke gate | unchanged |

## Goal
Build the standalone `concerto-agent-host` binary — a tiny helper process the Core spawns, then detaches, to own a PTY and run an agent CLI as its child. After this task, the Core can spawn an agent host with `echo hello` as the wrapped command, see the output stream back over the host-bridge UDS, and watch the host survive a Core restart.

## Inputs to read before starting
- `design/04_Agent_Supervisor.md` §3.9 (entire section — the host design, detachment, bridge protocol, ring buffer, cookie, exit info), §6.1 (spawn sequence), §6.4 (host adoption).
- `design/01_Core_Daemon_Runtime.md` §3.1 (no daemonization in Core), §6.3 (the surviving-host invariant Core depends on).
- `tasks/12-supervision-tree.md` → "Handoff Notes".

## Scope — in
Implement `crates/agent-host/`:

- `src/main.rs` binary that parses argv:
  - `--agent-bin <path>` (the agent CLI to run, e.g., `claude`).
  - `--agent-arg <s>` (repeatable; passed to the agent).
  - `--cwd <path>` (working directory — workarea root).
  - `--socket <path>` (UDS to bind for the host-bridge).
  - `--cookie <hex32>` (32-byte cookie the Core uses on `Hello`).
  - `--resume-jsonl <path>` (optional — passed through to agent's `--resume`).
  - `--final-info <path>` (where to write exit info on shutdown).
- Process flow:
  1. Detach: on Unix, fork once then `setsid()` (parent exits 0); on Windows, the spawn-with-`DETACHED_PROCESS` is done by the parent (Core), so the binary just runs.
  2. Open a PTY via `portable-pty`.
  3. Spawn the agent CLI as PTY child.
  4. Bind the UDS at `--socket` with permissions `0600`.
  5. Loop:
     - Read PTY stdout/stderr → append to a 1 MiB ring buffer; broadcast to connected Core (if any).
     - Read from the UDS connection → write to PTY stdin (for input forwarded by Core).
     - Watch for PTY child exit; if exited, write `--final-info` JSON (exit code, last 100 lines, agent's external session ID if discoverable) and shut down the UDS.
  6. Frame protocol: length-prefixed CBOR per `design/04 §3.9` `HostFrame` enum. Add the `serde_cbor` (or `ciborium`) dep.
- Cookie verification: on `Hello`, compare with `--cookie`; if mismatch, send a typed `CookieMismatch` error frame and close the connection.
- Single-connection model: at most one Core can be connected at a time. Reject the second `Hello`.
- Reconnect semantics: when the connection drops, the host keeps running and accepts a new `Hello`. The ring buffer survives; the new Core sends its last-acked `seq` in `Hello` and the host replays past that point.
- Add integration test (in `crates/agent-host/tests/`):
  - Spawn host with `--agent-bin echo --agent-arg hello`.
  - Connect to its socket, send `Hello`, receive `Ready`, receive `StdoutBytes` containing `"hello"`, receive `AgentExited`.
  - Verify `--final-info` JSON exists with the right exit code.

## Scope — out
- Cold-resume orchestration (Task 37 — the Core side calls into host with `--resume-jsonl`).
- Hot-reconnect ack semantics in detail (Task 36 builds the Core side).
- ConPTY support on Windows in V0.1 (macOS only — note in Handoff).
- Audit-log integration (Task 44).

## Public interface this task locks
- Binary CLI: `concerto-agent-host --agent-bin <path> [--agent-arg <s>]... --cwd <p> --socket <p> --cookie <hex32> [--resume-jsonl <p>] --final-info <p>`.
- Wire format: length-prefixed CBOR over UDS; `HostFrame` enum as specified in `design/04 §3.9`. Field numbers / variant ordering FROZEN.
- Final-info JSON schema:
  ```json
  {
    "exit_code": 0,
    "signal": null,
    "last_lines": ["..."],
    "external_session_id": null,
    "exited_at_unix_ms": 1716800000123
  }
  ```
- UDS permissions: `0600`.

## Implementation notes
- `portable-pty` (`portable-pty = "0.8"` or current stable) provides cross-platform PTY. Use `PtySize { rows: 24, cols: 120, ... }` as initial size; resize via `Resize` frames.
- Detach pattern on Unix: `fork()` via `nix`, parent exits; child calls `setsid()`. Watch out: tokio runtime cannot be initialized before `fork()` — initialize Tokio after the fork in the child. Easiest pattern: do the fork in a `#[no_mangle] extern "C" fn` before `main`; or use a separate detach helper crate. A simpler approach: have the Core call `posix_spawn`-equivalent with the setsid flag, and `concerto-agent-host` doesn't fork itself. This shifts complexity to the Core side. **Recommended:** the Core handles detachment via `tokio::process::Command` + `unsafe { libc::setsid() }` in a `pre_exec` callback. The host binary then runs as a normal process. Document this choice in Handoff Notes.
- Frame encoding: `ciborium` is the maintained CBOR option (`serde_cbor` is unmaintained). Use 4-byte big-endian length prefix.
- Ring buffer: `VecDeque<u8>` capped at 1 MiB. Use a `tokio::sync::Mutex` (low contention).
- Send `Pong` in response to `Ping` to support heartbeat.
- Use the `concerto-error` crate; map errors via `From` impls.

## Verification
1. `cargo build -p concerto-agent-host` → succeeds.
2. `cargo test -p concerto-agent-host` → integration test passes.
3. `cargo clippy -p concerto-agent-host -- -D warnings` → clean.
4. Manual: spawn via shell with `--agent-bin echo --agent-arg hello`; connect with `socat - UNIX-CONNECT:/tmp/test.sock` — note that `socat` won't speak CBOR; use a small Rust test client. The integration test IS this verification path.
5. Verify the host survives parent exit: spawn from a shell, `exit` the shell, observe the host still running (`ps aux | grep concerto-agent-host`). On Unix-only.
6. `cargo deny check` → clean (verify `portable-pty` and `ciborium` license-compatible).
7. `scripts/smoke.sh` still passes.

## Definition of Done
- [ ] Verification commands pass.
- [ ] Integration test covers spawn → Hello → Ready → output → Exit.
- [ ] Cookie verification verified (test sends wrong cookie; gets CookieMismatch).
- [ ] Process survives parent exit (manual or scripted check).
- [ ] `--final-info` JSON written correctly on exit.
- [ ] No `TODO` / `FIXME` / `unimplemented!()` in new code.
- [ ] Smoke gate still green.
- [ ] Single commit created.

## Outputs
- `crates/agent-host/Cargo.toml` (modified — portable-pty, ciborium, nix or libc, clap)
- `crates/agent-host/src/main.rs` (modified)
- `crates/agent-host/src/bridge.rs` (new — CBOR frame protocol)
- `crates/agent-host/src/ring.rs` (new — 1 MiB ring buffer)
- `crates/agent-host/src/exit.rs` (new — final-info writer)
- `crates/agent-host/src/api.rs` (new — public types: HostFrame)
- `crates/agent-host/tests/echo_round_trip.rs` (new)
- `docs/interfaces/rust-api.md` (regenerated)

## Commit message
```
phase-2: concerto-agent-host helper binary

Standalone binary that owns a PTY for an agent CLI and bridges I/O
back to the Core over a UDS with a 32-byte cookie. CBOR HostFrame
protocol per design/04 §3.9. 1 MiB ring buffer survives Core
restart-reconnect. Tested end-to-end with `echo`.

Refs: tasks/21-agent-host-binary.md
```

## Handoff Notes (fill in when finishing)
- **Drift from plan:** —
- **Open questions for next task:** chosen detachment strategy (Core-side pre_exec vs in-host fork)?
- **Deliberate debt:** Windows ConPTY support deferred (V1.0 — Windows port).
- **Smoke-gate state:** unchanged.
