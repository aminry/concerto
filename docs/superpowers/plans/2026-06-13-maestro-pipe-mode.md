# Maestro Pipe-Mode Spawn Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the agent-host a non-PTY "pipe mode" spawn, selected by the Core only for `AgentKind::Maestro`, so `claude --print --input-format stream-json` (which refuses a TTY) gets a pipe stdin and the Maestro chat works live.

**Architecture:** Add an optional `--io-mode <pty|pipe>` flag to the agent-host (default `pty`). Extract the reader/writer pump-thread bodies into shared helpers (they already operate on boxed `Read`/`Write`). Add `run_pipe` (spawns via `std::process::Command` with piped stdio, reuses the shared pumps, drains stderr to the log, no resize) beside the unchanged `run_pty`. The Core appends `--io-mode pipe` to the host args for the Maestro session. The `StdinBytes`/`StdoutBytes` frame protocol and the entire Core side are unchanged.

**Tech Stack:** Rust, `std::process::Command` / `std::os::unix::process::ExitStatusExt`, `portable_pty` (existing), `clap`, tokio.

**Reference spec:** `docs/superpowers/specs/2026-06-13-maestro-pipe-mode-design.md`

**Branch:** `maestro-live-conversation` (pipe-mode is the conversation milestone's final enabling piece).

---

## Background facts (verified — read before starting)

- `crates/agent-host/src/main.rs` (inside `mod unix`): `Cli` (flat `clap::Parser`, "locked by Task 21") has `agent_bin`, `agent_arg: Vec<String>`, `cwd`, `socket`, `cookie`, `resume_jsonl: Option<PathBuf>`, `final_info`.
- `spawn_pty_task(cli, state, stdin_rx, resize_rx) -> JoinHandle<(Option<i32>, Option<i32>)>` (≈line 224) clones fields off `cli`, captures `tokio::runtime::Handle::current()`, and `spawn_blocking`s `run_pty(...)`.
- `run_pty(agent_bin, agent_args, cwd, resume, state, stdin_rx, resize_rx, rt) -> (Option<i32>, Option<i32>)` (≈line 250): `openpty` → `CommandBuilder` → `pair.slave.spawn_command` → `try_clone_reader()` (`Box<dyn Read+Send>`) + `take_writer()` (`Box<dyn Write+Send>`). Then: a `chunk_tx`/`chunk_rx` channel + an `rt.spawn` async task draining `chunk_rx` → `record_chunk(&state, chunk)`; a **reader thread** (`reader.read(buf)` → `chunk_tx.send`); a **writer thread** (`stdin_rx.blocking_recv()` → `writer.write_all`+`flush`); a **resize thread**; then `child.wait()`.
- `record_chunk(&state, Vec<u8>)` puts bytes on the ring buffer that feeds BOTH `StdoutBytes` frames AND `final-info` last-lines. (So stderr must NOT go through `record_chunk` — it would corrupt the stream-json stdout the parser reads.)
- The agent-host emits `tracing` (its `info!`/`warn!` already appear in the Core's console output, e.g. "host bridge listening"), so a `warn!`-logged stderr line IS visible during Tier-3.
- `crates/core/src/agent_supervisor/spawn.rs` (≈lines 200-218) builds the `concerto-agent-host` arg vector: `cmd.arg("--agent-bin").arg(...)`, `--cwd`, `--socket`, `--cookie`, `--final-info`, and conditionally `--resume-jsonl`.
- The PTY path is exercised by `crates/core/tests/{agent_spawn,cold_resume,hot_reconnect}.rs` (the Echo agent) — the regression guard.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `crates/agent-host/src/main.rs` | `IoMode` enum + `--io-mode` Cli flag; `spawn_reader_thread`/`spawn_writer_thread` shared helpers; `run_pipe` + `spawn_pipe_task`; dispatch on `cli.io_mode` | Modify |
| `crates/core/src/agent_supervisor/spawn.rs` | append `--io-mode pipe` for `AgentKind::Maestro` | Modify |
| `crates/agent-host/tests/pipe_round_trip.rs` | pipe-mode integration test | Create |

---

## Task 1: `--io-mode` flag

**Files:**
- Modify: `crates/agent-host/src/main.rs` (the `Cli` struct + a new `IoMode` enum)
- Test: in-file `#[cfg(test)]`

- [ ] **Step 1: Add the enum + flag.** In `mod unix`, near `Cli`:
```rust
/// How the agent-host wires the child's stdio. `Pty` (default) runs the agent
/// in a pseudo-terminal (interactive TUI agents). `Pipe` wires stdin/stdout as
/// plain pipes (a non-TTY) — required for `claude --print --input-format
/// stream-json`, which refuses a TTY. Selected by the Core per agent kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum, Default)]
pub enum IoMode {
    #[default]
    Pty,
    Pipe,
}
```
Add to `Cli`:
```rust
/// stdio wiring for the child (default `pty`). The Core passes `pipe` for the
/// Maestro session.
#[arg(long, value_enum, default_value_t = IoMode::Pty)]
io_mode: IoMode,
```

- [ ] **Step 2: Test default + parse.** Add:
```rust
#[test]
fn io_mode_defaults_to_pty_and_parses_pipe() {
    use clap::Parser;
    let base = ["concerto-agent-host", "--agent-bin", "/bin/echo", "--cwd", "/tmp",
        "--socket", "/tmp/s.sock", "--cookie", "00", "--final-info", "/tmp/f.json"];
    let cli = Cli::try_parse_from(base).expect("parse");
    assert_eq!(cli.io_mode, IoMode::Pty);
    let mut withpipe: Vec<&str> = base.to_vec();
    withpipe.extend(["--io-mode", "pipe"]);
    assert_eq!(Cli::try_parse_from(withpipe).unwrap().io_mode, IoMode::Pipe);
}
```
(Adjust the required-arg list to match the real `Cli` required fields — read the struct; the test must supply every non-Option arg.)

- [ ] **Step 3: Run → fail → implement → pass.** `cargo test -p concerto-agent-host io_mode_defaults` (fails: no field), add the code, re-run → pass. `cargo build -p concerto-agent-host`.

- [ ] **Step 4: Commit.**
```bash
git add crates/agent-host/src/main.rs
git commit -m "feat(agent-host): --io-mode pty|pipe flag (default pty)"
```

---

## Task 2: Extract shared pump helpers (no behavior change to PTY)

**Files:**
- Modify: `crates/agent-host/src/main.rs`

Extract the reader-thread and writer-thread bodies so `run_pty` (and the new `run_pipe`) share them. The PTY path keeps identical behavior — the regression tests are the guard.

- [ ] **Step 1: Add the helpers** (module-level in `mod unix`):
```rust
/// Reader pump: blocking-read `reader` in 8 KiB chunks and forward each chunk
/// over `chunk_tx` (an async task records them onto the ring buffer). Returns
/// when the child closes its output (EOF) or the channel drops. Shared by the
/// PTY (master reader) and pipe (`ChildStdout`) sessions.
fn spawn_reader_thread(
    mut reader: Box<dyn std::io::Read + Send>,
    chunk_tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if chunk_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    })
}

/// Writer pump: drain `stdin_rx` and write each payload to `writer` (the child's
/// stdin). Shared by the PTY (master writer) and pipe (`ChildStdin`) sessions.
fn spawn_writer_thread(
    mut writer: Box<dyn std::io::Write + Send>,
    mut stdin_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while let Some(data) = stdin_rx.blocking_recv() {
            if writer.write_all(&data).is_err() {
                break;
            }
            let _ = writer.flush();
        }
    })
}
```

- [ ] **Step 2: Rewire `run_pty` to call them.** Replace the inline reader-thread (the `reader_handle = std::thread::spawn(...)` block) with `let reader_handle = spawn_reader_thread(reader, chunk_tx);`. Replace the inline writer-thread block with `let stdin_thread = spawn_writer_thread(writer, stdin_rx);`. **Remove** the now-unneeded `writer_mutex`/`writer_for_stdin` `Arc<Mutex>` wrap (the writer was only used by the stdin thread; the resize thread uses `master`, a separate object). Keep the `chunk_tx`/`chunk_rx` channel + the `rt.spawn` record task + the resize thread + `child.wait()` exactly as they are.

- [ ] **Step 3: Verify no behavior change.**
- `cargo build -p concerto-agent-host` + `cargo clippy -p concerto-agent-host --all-targets -- -D warnings` → clean.
- `cargo test --workspace --test agent_spawn --test cold_resume --test hot_reconnect` → green (the PTY regression guard — these MUST pass unchanged). Run the whole-workspace form so `CARGO_BIN_EXE_concerto-agent-host` is set.

- [ ] **Step 4: Commit.**
```bash
git add crates/agent-host/src/main.rs
git commit -m "refactor(agent-host): extract shared reader/writer pump helpers"
```

---

## Task 3: `run_pipe` + `spawn_pipe_task`

**Files:**
- Modify: `crates/agent-host/src/main.rs`

- [ ] **Step 1: Add `run_pipe`** (sibling of `run_pty`):
```rust
/// Pipe-mode supervisor: spawn the child with plain piped stdio (a non-TTY) so
/// `claude --print --input-format stream-json` enters streaming multi-turn mode.
/// Reuses the shared reader/writer pumps; stderr is drained to the host log
/// (NOT the ring buffer — that feeds the stream-json stdout the parser reads);
/// resize requests are ignored (pipes don't resize).
#[allow(clippy::too_many_arguments)]
fn run_pipe(
    agent_bin: PathBuf,
    agent_args: Vec<String>,
    cwd: PathBuf,
    resume: Option<PathBuf>,
    state: Arc<State>,
    stdin_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    mut resize_rx: tokio::sync::mpsc::UnboundedReceiver<(u16, u16)>,
    rt: tokio::runtime::Handle,
) -> (Option<i32>, Option<i32>) {
    use std::process::Stdio;

    let mut cmd = std::process::Command::new(&agent_bin);
    cmd.args(&agent_args);
    if let Some(r) = &resume {
        cmd.arg("--resume").arg(r);
    }
    cmd.current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, bin = ?agent_bin, "spawn agent CLI (pipe mode) failed");
            let s = state.clone();
            rt.block_on(async move {
                record_chunk(&s, format!("[concerto] Failed to start agent '{}': {}\n", agent_bin.display(), e).into_bytes()).await;
            });
            return (None, None);
        }
    };

    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    // stdout → ring buffer (same record path as PTY → StdoutBytes frames).
    let (chunk_tx, mut chunk_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let state_for_record = state.clone();
    rt.spawn(async move {
        while let Some(chunk) = chunk_rx.recv().await {
            record_chunk(&state_for_record, chunk).await;
        }
    });
    let reader_handle = spawn_reader_thread(Box::new(stdout), chunk_tx);
    let stdin_thread = spawn_writer_thread(Box::new(stdin), stdin_rx);

    // stderr → host log only (Q1-B). NOT the ring buffer (keeps stdout clean for
    // the stream-json parser). Visible during Tier-3 via the agent-host's tracing.
    let stderr_thread = std::thread::spawn(move || {
        use std::io::BufRead;
        let mut r = std::io::BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match r.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => warn!(target: "concerto::agent_host", stderr = %line.trim_end(), "agent stderr (pipe mode)"),
                Err(_) => break,
            }
        }
    });

    // Pipes don't resize — drain the channel so its sender side never blocks.
    let resize_thread = std::thread::spawn(move || while resize_rx.blocking_recv().is_some() {});

    let status = child.wait().ok();
    reader_handle.join().ok();
    let _ = (stdin_thread, stderr_thread, resize_thread);

    let exit_code = status.as_ref().and_then(|s| s.code());
    let signal = {
        use std::os::unix::process::ExitStatusExt;
        status.as_ref().and_then(|s| s.signal())
    };
    (exit_code, signal)
}
```
(Confirm `error!`/`warn!`/`record_chunk`/`State` are in scope as in `run_pty`. The `(Option<i32>, Option<i32>)` return = `(exit_code, signal)`, matching `run_pty`.)

- [ ] **Step 2: Add `spawn_pipe_task`** (sibling of `spawn_pty_task`):
```rust
fn spawn_pipe_task(
    cli: &Cli,
    state: Arc<State>,
    stdin_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    resize_rx: tokio::sync::mpsc::UnboundedReceiver<(u16, u16)>,
) -> JoinHandle<(Option<i32>, Option<i32>)> {
    let agent_bin = cli.agent_bin.clone();
    let agent_args = cli.agent_arg.clone();
    let cwd = cli.cwd.clone();
    let resume = cli.resume_jsonl.clone();
    let rt = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        run_pipe(agent_bin, agent_args, cwd, resume, state, stdin_rx, resize_rx, rt)
    })
}
```

- [ ] **Step 3:** `cargo build -p concerto-agent-host` + `cargo clippy -p concerto-agent-host --all-targets -- -D warnings` → clean.

- [ ] **Step 4: Commit.**
```bash
git add crates/agent-host/src/main.rs
git commit -m "feat(agent-host): run_pipe — non-PTY piped-stdio session (stderr→log, no resize)"
```

---

## Task 4: Dispatch on `cli.io_mode`

**Files:**
- Modify: `crates/agent-host/src/main.rs` (the `spawn_pty_task` call site)

- [ ] **Step 1: Find + branch.** Grep `spawn_pty_task(` for its single call site (in the connection/session-startup path). Replace it with:
```rust
let session_task = match cli.io_mode {
    IoMode::Pty => spawn_pty_task(&cli, state.clone(), stdin_rx, resize_rx),
    IoMode::Pipe => spawn_pipe_task(&cli, state.clone(), stdin_rx, resize_rx),
};
```
(Match the real surrounding variable names — `cli`, `state`, `stdin_rx`, `resize_rx` — and how the returned `JoinHandle` is bound/awaited. Both arms return the identical `JoinHandle<(Option<i32>, Option<i32>)>`, so only the constructor differs.)

- [ ] **Step 2: Verify.** `cargo build -p concerto-agent-host` + `cargo clippy -p concerto-agent-host --all-targets -- -D warnings` → clean. `cargo test --workspace --test agent_spawn --test cold_resume --test hot_reconnect` → green (PTY path still selected by default, unchanged).

- [ ] **Step 3: Commit.**
```bash
git add crates/agent-host/src/main.rs
git commit -m "feat(agent-host): dispatch pty vs pipe session on --io-mode"
```

---

## Task 5: Core passes `--io-mode pipe` for the Maestro

**Files:**
- Modify: `crates/core/src/agent_supervisor/spawn.rs`

- [ ] **Step 1: Find the kind.** Read `spawn.rs` around lines 200-218 (the `cmd.arg("--agent-bin")…` block). Determine how `AgentKind` is available there (a function parameter, a field on a passed struct/req, or derivable from `agent_bin`/the resolved launch). It is almost certainly already in scope (the spawn resolves the bin per kind). If it is NOT directly available, thread it in from the caller (`resolve_agent_bin`/`start_session` knows `req.agent_kind`).

- [ ] **Step 2: Append the flag.** After the existing `--final-info` arg (and alongside the resume conditional), add:
```rust
// The Maestro runs claude headless `--print --input-format stream-json`, which
// refuses a TTY — so it needs pipe-mode stdio. Every other kind stays PTY.
if agent_kind == AgentKind::Maestro {
    cmd.arg("--io-mode").arg("pipe");
}
```
(Use the real `AgentKind` path/enum value in scope.)

- [ ] **Step 3: Test (if a spawn-arg test exists).** Grep `spawn.rs`/its tests for an existing arg-vector assertion; if present, add a case: a Maestro spawn includes `--io-mode pipe`, a non-Maestro spawn does not. If the arg-building is not unit-testable without a full spawn, rely on the Tier-3 + the integration test (Task 6) — note it.

- [ ] **Step 4: Verify.** `cargo build -p concerto-core` + `cargo clippy -p concerto-core --all-targets --exclude concerto-desktop --exclude concerto-smoke-client --exclude concerto-test-harness --exclude concerto-pair-serve -- -D warnings` → clean. `cargo test -p concerto-core agent_supervisor` → green.

- [ ] **Step 5: Commit.**
```bash
git add crates/core/src/agent_supervisor/spawn.rs
git commit -m "feat(maestro): spawn the Maestro session in agent-host pipe mode"
```

---

## Task 6: Pipe round-trip integration test

**Files:**
- Create: `crates/agent-host/tests/pipe_round_trip.rs`

Prove pipe mode round-trips bytes with no PTY: launch `concerto-agent-host --io-mode pipe` against a stdin→stdout agent (`/bin/cat` echoes stdin to stdout), connect the Core side over the UDS, write `StdinBytes`, and assert the bytes come back as `StdoutBytes`.

- [ ] **Step 1: Write the test.** Mirror the existing `crates/agent-host/tests/echo_round_trip.rs` harness (it already spawns the host + drives the UDS framing). Change: pass `--io-mode pipe` and use `/bin/cat` as `--agent-bin` (a pure stdin→stdout pipe agent — no PTY needed). Write a `StdinBytes` frame carrying `b"ping\n"`, then read frames and assert a `StdoutBytes` frame containing `ping` comes back. Use `assert_cmd::cargo::cargo_bin("concerto-agent-host")` (as the existing host tests do) to locate the bin.
```rust
// (sketch — adapt to the real echo_round_trip.rs harness types/helpers)
// 1. bind a UDS, spawn `concerto-agent-host --io-mode pipe --agent-bin /bin/cat
//    --cwd <tmp> --socket <sock> --cookie <hex> --final-info <tmp>/f.json`
// 2. accept the host connection, send the Hello/cookie handshake
// 3. write HostFrame::StdinBytes { data: b"ping\n".to_vec() } (the real frame
//    type/direction — copy echo_round_trip's send path)
// 4. read frames until a StdoutBytes whose data contains b"ping" arrives
// 5. assert it; then shut down (drop senders / kill).
```
**Executor note:** read `crates/agent-host/tests/echo_round_trip.rs` first and reuse its exact frame-construction + handshake helpers — only the `--io-mode pipe` flag + `/bin/cat` agent + the assertion differ. `/bin/cat` with piped stdin echoes stdin→stdout immediately; in a PTY it would line-buffer/echo differently, so this test also implicitly proves pipe (not PTY) wiring.

- [ ] **Step 2: Run → pass.** `cargo test -p concerto-agent-host --test pipe_round_trip` → green.

- [ ] **Step 3: Commit.**
```bash
git add crates/agent-host/tests/pipe_round_trip.rs
git commit -m "test(agent-host): pipe-mode round-trip (cat echo over piped stdio)"
```

---

## Task 7: Full gate

- [ ] **Step 1: Full workspace gate.**
- `cargo test --workspace` → green (esp. `agent_spawn`/`cold_resume`/`hot_reconnect` PTY regression + the new `pipe_round_trip`). If a flaky timeout appears under heavy parallel load, re-run the specific test (CI uses `--retries 2`).
- `cargo clippy --workspace --all-targets --exclude concerto-desktop --exclude concerto-smoke-client --exclude concerto-test-harness --exclude concerto-pair-serve -- -D warnings` → clean.
- `cargo fmt --all -- --check` → clean (run `cargo fmt --all` to fix).
- `./scripts/regen-interfaces.sh && git diff --exit-code docs/interfaces/` → no drift (no proto change here, so it should be clean).
- `cd apps/desktop && pnpm run typecheck && pnpm run test` → green (untouched, but confirm).

- [ ] **Step 2: Commit any fmt fixes** (if `cargo fmt --all` changed anything):
```bash
git add -A && git commit -m "style(agent-host): rustfmt"
```

---

## Manual verification (Tier-3 — the live test that's been failing)

1. Build: `cargo build -p concerto-core -p concerto-agent-host -p concerto-maestro-bridge` from this branch.
2. `RUST_LOG=info,concerto::maestro=debug ./target/debug/concerto-core` (clean stale sockets first).
3. Confirm the Maestro session **stays alive**: `pgrep -af claude | grep stream-json` returns a process that persists for >10s (NOT exiting immediately); the session resolves in the DB (`SELECT s.id FROM sessions s JOIN chats c ON c.id=s.chat_id WHERE c.kind='maestro' AND s.ended_at IS NULL`). The session's `final-info.json` should NOT contain the `--print`/`Input must be provided` errors.
4. `pnpm tauri dev`; type `what are my workareas doing?` → your user bubble, then a **streamed grounded reply** that used the read tools. Reload → history persists.
5. If `claude` still errors, check the agent-host stderr in the Core console (the `warn!(target:"concerto::agent_host", … "agent stderr (pipe mode)")` lines) for the reason.

---

## Self-Review

**Spec coverage:** `--io-mode` flag (Task 1) ✓ · shared pump helpers (Task 2) ✓ · `run_pipe` w/ piped stdio + stderr→log + no resize (Task 3) ✓ · dispatch (Task 4) ✓ · Core selects pipe for Maestro (Task 5) ✓ · pipe round-trip test + PTY regression guard (Tasks 2/4/6/7) ✓ · Tier-3 (manual) ✓. Error handling: spawn-failure path (Task 3), stderr→log (Task 3), resize-drain (Task 3). All spec sections map.

**Placeholder scan:** code steps carry real code. Three executor notes (Task 4 dispatch call-site, Task 5 `AgentKind` availability in spawn.rs, Task 6 reuse echo_round_trip harness) are genuine "match the local shape" points, not hidden decisions.

**Type consistency:** `IoMode` (T1) used in T4 dispatch + the Cli. `spawn_reader_thread`/`spawn_writer_thread` (T2) consumed by `run_pty` (T2) + `run_pipe` (T3). `run_pipe`/`spawn_pipe_task` (T3) called in T4. Both `spawn_*_task` return the same `JoinHandle<(Option<i32>, Option<i32>)>`. `--io-mode pipe` produced by Core (T5) ↔ parsed by the Cli (T1). Consistent.
