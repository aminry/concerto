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
- [x] Verification commands pass.
- [x] Integration test covers spawn → Hello → Ready → output → Exit.
- [x] Cookie verification verified (test sends wrong cookie; gets CookieMismatch).
- [x] Process survives parent exit (manual or scripted check).
- [x] `--final-info` JSON written correctly on exit.
- [x] No `TODO` / `FIXME` / `unimplemented!()` in new code.
- [x] Smoke gate still green.
- [x] Single commit created.

## Outputs
- `crates/agent-host/Cargo.toml` (modified — portable-pty, ciborium, nix or libc, clap)
- `crates/agent-host/src/main.rs` (modified)
- `crates/agent-host/src/lib.rs` (new — exposes `api`, `bridge`, `ring`, `exit` modules so the integration test can link against them directly; the `[lib]` table was added to `Cargo.toml` for the same reason)
- `crates/agent-host/src/bridge.rs` (new — CBOR frame protocol)
- `crates/agent-host/src/ring.rs` (new — 1 MiB ring buffer)
- `crates/agent-host/src/exit.rs` (new — final-info writer)
- `crates/agent-host/src/api.rs` (new — public types: HostFrame, AgentKind, FinalInfo)
- `crates/agent-host/tests/echo_round_trip.rs` (new)
- `docs/interfaces/rust-api.md` (regenerated)
- `.github/workflows/ci.yml` (modified — added `--exclude concerto-agent-host` to the Windows row, matching the existing Tauri / smoke-client / test-harness exclusion pattern; per the task header drift note)
- `Cargo.lock` (modified — automatic from new direct deps: portable-pty, ciborium, clap, hex, subtle)
- `tasks/21-agent-host-binary.md` (modified — DoD ticks + Handoff Notes, per the standard task workflow)

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
- **Drift from plan:**
  - **Detachment strategy: Core-side `pre_exec` + `setsid()`, NOT in-host fork.** The task's Implementation notes called this out as the recommended choice and the binary is written accordingly: `main.rs` is a normal Tokio binary with no `fork()` of its own. The Core's Agent Supervisor (future Task 22 / Phase 3 §3.7) is responsible for arranging session-leader status via `tokio::process::Command::pre_exec(|| { unsafe { libc::setsid(); } Ok(()) })`. The "process survives parent exit" check was verified manually: `bash -c '<host> &'`; the host's PPID becomes 1 (init/launchd) after the bash subshell exits, satisfying the surviving-host invariant from `design/01 §6.3`.
  - **Library + binary, not binary-only.** The Cargo manifest declares both `[[bin]]` and `[lib]` so the integration test can `use concerto_agent_host::api::HostFrame`/`bridge::*` directly instead of duplicating the CBOR encoder. The interface generator picks up `crates/agent-host/src/api.rs` per the workspace convention; `rust-api.md` now lists `HostFrame`, `AgentKind`, and `FinalInfo`.
  - **`HostFrame::StdinBytes` has no `seq` field.** The task notes mention "common variants" including `StdinBytes { data }`; the design doc §3.9 sketch shows `StdinBytes { seq, data }`. I went with the task's variant (no seq) because stdin is Core → host only and the Core doesn't replay stdin on reconnect — the wire savings are marginal but the variant set matches the task prompt verbatim. If the Phase 3 Core integration finds it actually needs the seq, that's a wire-format break needing a follow-on task.
  - **`AgentExited` field names are `exit_code` + `signal`, not `code` + `signal` as in the design-doc sketch.** The task spec's final-info JSON keys (`exit_code`, `signal`) and the design-doc CBOR variant (`code`, `signal`) disagreed; I went with `exit_code` everywhere for consistency between the wire frame and the on-disk JSON. The design doc's exact names aren't load-bearing — they're a sketch — and a single name across both surfaces is easier for the Core to consume.
  - **`Ack { seq }` is part of the locked variant set.** Task notes listed common variants without `Ack`; design/04 §3.9 names it explicitly. The host implements it as a ring-buffer prune trigger so Task 36's hot-reconnect work can wire the Core side without a wire-format change.
  - **Added `AlreadyConnected` frame.** The single-connection invariant needed a distinct frame so the Core can tell "two Cores were spawned" (admin error) apart from "wrong cookie" (impersonation attempt). Task notes called for "send `CookieMismatch`-equivalent error" — `AlreadyConnected` is that equivalent and is documented in `crate::api`.
  - **Post-exit grace window of 30s with early termination.** The host doesn't tear down the bridge socket the instant the PTY child exits; that would race a Core that connected after the agent finished and force a final-info JSON disk read for what should be an in-process drain. The accept loop honors a 30s grace post-child-exit during which it still accepts new connections. The grace ends early once the connected Core has actually received the `AgentExited` frame (tracked via `delivered_exit` flag + `Notify`). The integration test relies on this grace because `echo hello` finishes ~10 ms after spawn, well before the harness can connect.
  - **PTY I/O runs on blocking threads with the Tokio handle threaded through.** `portable-pty` returns synchronous `Read`/`Write` handles. A `tokio::task::spawn_blocking` wraps the supervisor; inside that the reader thread is a plain `std::thread::spawn` that needs an explicit `tokio::runtime::Handle::clone()` to call back into async primitives (ring buffer `Mutex`, `Notify`). Capturing `Handle::current()` from inside `spawn_blocking` would panic — the blocking pool isn't a Tokio runtime context — so I capture it at the outer task site and pass it down. Without that the integration test silently saw empty stdout (the reader thread early-returned on `Handle::try_current().is_err()`).
  - **No stderr frames emitted in V0.1.** `portable-pty` merges stderr into the PTY master (that's how a PTY works), so the host never sees a separate stderr stream. The `HostFrame::StderrBytes` variant is locked in the wire format for V1.0 but never emitted in V0.1. Documented inline on the variant.
  - **External session ID detection is not implemented.** The `FinalInfo::external_session_id` and `HostFrame::Ready::external_session_id` fields are wired but always `None` in V0.1 — parsing Claude/Codex preamble for the session token is Task 37's job (cold-resume from JSONL). Slot is locked here so Task 37 only adds the parser, not a wire-format change.
  - **CI Windows-exclude list extended to include `concerto-agent-host`.** The binary uses `#[cfg(unix)]` for the entire implementation and falls through to a "Windows ConPTY support is V1.0" error on Windows. The CI matrix would otherwise fail to compile the lib on Windows; excluding it matches the existing Tauri / smoke-client / test-harness pattern.
- **Open questions for next task:**
  - **Chosen detachment strategy: Core-side `pre_exec` + `setsid()`** (explicit answer to the task's stated open question). Task 22 (`spawn agent CLI from agent-host`) will need to add `pre_exec` glue to the Agent Supervisor when it actually spawns the host. The host binary itself is intentionally inert about detachment — that decoupling keeps the host testable from a normal cargo test runner without `setsid` games.
  - **Single connection is enforced inside `run_connection`** via `state.connection_active`. If two Cores race past `Hello`, only the first claims the slot; the second gets `AlreadyConnected` and disconnects. The check is async-mutex-protected so it's race-free even with concurrent accepts.
  - **Ring buffer prune semantics for `Ack` are intentionally permissive.** The host prunes through `ack_seq` immediately on receipt; Task 36's hot-reconnect work may want a "keep last N regardless" floor so a fast-acking Core can't accidentally lose the buffer just before a disconnect. The hook is in `RingBuffer::prune_through` and is the right place to add such a policy.
  - **`portable-pty` keeps the master open through an `Arc<Mutex<_>>` for resize.** The blocking writer/resize threads exit only when their channels close. The current `main` drops the senders after `pty_handle.await` returns, which is correct, but if Task 22's host-spawn loop tries to feed stdin/resize concurrently with a still-running child it must hold the senders until child exit. The integration test does this trivially (echo exits before any stdin arrives).
  - **Final-info file is best-effort.** A write failure logs a warning but doesn't change the host's exit code. That's intentional: by the time the host writes final-info the agent has already exited, so a propagated I/O error would lose more information than it gains. If a future task needs hard guarantees here (e.g. audit-log Task 44), it can add a non-zero exit on write failure as a follow-on.
- **Deliberate debt:** Windows ConPTY support deferred to V1.0 (per `design/04 §3.9` phasing). The Windows path is a clean `eprintln + exit(2)`; no `TODO`/`FIXME` markers. External-session-ID parsing is a stub per Task 37; field slot is locked, parser is not.
- **Smoke-gate state:** unchanged. The smoke gate is still v1 (Core boot + `GetServerCapabilities`); Task 27 promotes it to v2 once the agent-spawn end-to-end path is wired through the Core. `scripts/smoke.sh` still exits 0 and prints "Smoke gate v1: PASSED".
