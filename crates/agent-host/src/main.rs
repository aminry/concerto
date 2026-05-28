//! `concerto-agent-host` — per-session PTY helper binary.
//!
//! Spawned by the Core's Agent Supervisor, then detached (see Task 21
//! Handoff Notes for the chosen Unix detachment strategy: Core-side
//! `pre_exec` + `setsid()`, not in-host fork). The host owns a PTY,
//! runs the user's agent CLI inside it, and exposes a UDS bridge speaking
//! the CBOR `HostFrame` protocol locked in `crate::api`.
//!
//! V0.1 surface (locked by Task 21):
//!
//! * CLI: `concerto-agent-host --agent-bin <p> [--agent-arg <s>]...`
//!   `--cwd <p> --socket <p> --cookie <hex32> [--resume-jsonl <p>] --final-info <p>`
//! * UDS permissions: `0600`.
//! * Single connected Core at a time. Second `Hello` gets
//!   `AlreadyConnected` and is closed.
//! * Ring buffer: 1 MiB, evicts oldest chunks on overflow.
//! * Exit: writes `--final-info` JSON, broadcasts `AgentExited`, unbinds
//!   the socket.
//!
//! Windows is intentionally a hard-fail: the binary prints a
//! "Windows ConPTY support is V1.0" message and exits with status 2.

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use clap::Parser;
    use concerto_agent_host::api::{AgentKind, FinalInfo, HostFrame};
    use concerto_agent_host::bridge::{read_frame, write_frame, FrameError};
    use concerto_agent_host::exit::{tail_lines, write_final_info};
    use concerto_agent_host::ring::RingBuffer;
    use portable_pty::{CommandBuilder, PtySize};
    use std::io::{Read as _, Write as _};
    use subtle::ConstantTimeEq;
    use tokio::fs;
    use tokio::net::{UnixListener, UnixStream};
    use tokio::sync::{Mutex, Notify};
    use tokio::task::JoinHandle;
    use tracing::{debug, error, info, warn};

    /// CLI surface locked by Task 21. See module docs for the rationale.
    #[derive(Parser, Debug)]
    #[command(
        name = "concerto-agent-host",
        about = "Per-session PTY helper for the Concerto Core.",
        version
    )]
    pub struct Cli {
        /// Path to the agent CLI binary (e.g. `claude`, `codex`, `echo`
        /// in tests).
        #[arg(long)]
        agent_bin: PathBuf,
        /// Argument forwarded to the agent CLI. Repeatable; order is
        /// preserved.
        #[arg(long = "agent-arg")]
        agent_arg: Vec<String>,
        /// Working directory the agent CLI is launched in (a workarea
        /// worktree root in production).
        #[arg(long)]
        cwd: PathBuf,
        /// UDS path the host binds. Created with mode `0600`. Removed on
        /// shutdown.
        #[arg(long)]
        socket: PathBuf,
        /// 32-byte cookie encoded as 64 lowercase hex characters. The
        /// host verifies the Core's `Hello` against this value in
        /// constant time.
        #[arg(long)]
        cookie: String,
        /// Optional resume token forwarded to the agent CLI as
        /// `--resume <token>` (Claude/Codex semantics). When absent, no
        /// resume flag is passed.
        #[arg(long)]
        resume_jsonl: Option<PathBuf>,
        /// JSON file the host writes on PTY child exit. See
        /// [`FinalInfo`] for the schema.
        #[arg(long)]
        final_info: PathBuf,
    }

    /// Identifier for the "agent kind" surfaced in `Ready` frames. V0.1
    /// recognises Claude and Codex by basename; anything else is
    /// reported as `Other(basename)` so logs stay useful.
    fn agent_kind_from_bin(bin: &std::path::Path) -> AgentKind {
        let name = bin
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match name.as_str() {
            "claude" => AgentKind::Claude,
            "codex" => AgentKind::Codex,
            other => AgentKind::Other(other.to_string()),
        }
    }

    /// Shared state mutated by the PTY reader, the connection writer,
    /// and the child waiter. The mutex is held for nanoseconds per
    /// operation (a push or a clone-and-replay), so contention is not a
    /// concern at V0.1 throughput.
    struct State {
        ring: Mutex<RingBuffer>,
        /// Notifies the connection writer task that a new chunk is in
        /// the ring buffer (or that the child exited and it should
        /// drain).
        notify: Notify,
        /// Tail of stdout used to populate `FinalInfo::last_lines`.
        /// Capped at ~32 KiB to keep memory bounded independent of
        /// total agent runtime.
        tail: Mutex<Vec<u8>>,
        /// Set to true when the PTY child has exited. The connection
        /// writer drains remaining ring contents then sends
        /// `AgentExited` and tears the connection down.
        child_exited: Mutex<Option<(Option<i32>, Option<i32>)>>,
        /// Single-connection guard. `true` while a Core is connected.
        connection_active: Mutex<bool>,
        /// Set to true once a connected Core has successfully received
        /// the `AgentExited` frame. The accept loop uses this to end the
        /// post-exit grace window early — the surviving-host invariant
        /// is satisfied as soon as the Core has heard "the agent ended".
        delivered_exit: Mutex<bool>,
        /// Notifies the accept loop that `delivered_exit` flipped.
        exit_delivered_notify: Notify,
    }

    impl State {
        fn new() -> Self {
            Self {
                ring: Mutex::new(RingBuffer::default()),
                notify: Notify::new(),
                tail: Mutex::new(Vec::with_capacity(32 * 1024)),
                child_exited: Mutex::new(None),
                connection_active: Mutex::new(false),
                delivered_exit: Mutex::new(false),
                exit_delivered_notify: Notify::new(),
            }
        }
    }

    /// Append PTY output to both the ring buffer and the tail-of-stdout
    /// buffer used by [`FinalInfo`]. The tail is capped so a long-running
    /// agent doesn't grow it without bound.
    async fn record_chunk(state: &State, data: Vec<u8>) {
        {
            let mut ring = state.ring.lock().await;
            ring.push(data.clone());
        }
        {
            let mut tail = state.tail.lock().await;
            tail.extend_from_slice(&data);
            const CAP: usize = 32 * 1024;
            if tail.len() > CAP {
                let drop = tail.len() - CAP;
                tail.drain(..drop);
            }
        }
        state.notify.notify_waiters();
    }

    /// Validate `cookie_hex` is exactly 64 hex chars and decode it into a
    /// fixed 32-byte array. Bad cookies cause an early exit — there is
    /// no recoverable path because the Core would reject anything we
    /// produced.
    fn decode_cookie(cookie_hex: &str) -> Result<[u8; 32], String> {
        let bytes = hex::decode(cookie_hex).map_err(|e| format!("--cookie not valid hex: {e}"))?;
        if bytes.len() != 32 {
            return Err(format!("--cookie must be 32 bytes (got {})", bytes.len()));
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        Ok(out)
    }

    /// Bind the UDS and `chmod 0600` it. The bind path is removed first
    /// so a stale socket from a prior crash doesn't fail us.
    async fn bind_socket(path: &std::path::Path) -> std::io::Result<UnixListener> {
        if path.exists() {
            fs::remove_file(path).await.ok();
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).await.ok();
            }
        }
        let listener = UnixListener::bind(path)?;
        let mut perms = fs::metadata(path).await?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms).await?;
        Ok(listener)
    }

    /// PTY supervision task. Owns the PTY master, the spawned child, and
    /// the reader half of the master. Runs until the child exits.
    /// Returns the (exit_code, signal) pair on exit.
    fn spawn_pty_task(
        cli: &Cli,
        state: Arc<State>,
        stdin_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
        resize_rx: tokio::sync::mpsc::UnboundedReceiver<(u16, u16)>,
    ) -> JoinHandle<(Option<i32>, Option<i32>)> {
        let agent_bin = cli.agent_bin.clone();
        let agent_args = cli.agent_arg.clone();
        let cwd = cli.cwd.clone();
        let resume = cli.resume_jsonl.clone();
        // Capture the current runtime handle so the blocking helper
        // threads spawned via `std::thread::spawn` (which inherit nothing
        // from Tokio's thread-locals) can still call back into async
        // primitives like the ring buffer's `tokio::sync::Mutex`.
        let rt = tokio::runtime::Handle::current();

        tokio::task::spawn_blocking(move || {
            run_pty(
                agent_bin, agent_args, cwd, resume, state, stdin_rx, resize_rx, rt,
            )
        })
    }

    /// Body of the PTY supervisor. Runs on a blocking thread because
    /// `portable_pty` returns synchronous `Read`/`Write` handles for the
    /// master.
    #[allow(clippy::too_many_arguments)]
    fn run_pty(
        agent_bin: PathBuf,
        agent_args: Vec<String>,
        cwd: PathBuf,
        resume: Option<PathBuf>,
        state: Arc<State>,
        mut stdin_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
        mut resize_rx: tokio::sync::mpsc::UnboundedReceiver<(u16, u16)>,
        rt: tokio::runtime::Handle,
    ) -> (Option<i32>, Option<i32>) {
        let pty_system = portable_pty::native_pty_system();
        let pair = match pty_system.openpty(PtySize {
            rows: 24,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "openpty failed");
                return (None, None);
            }
        };

        let mut cmd = CommandBuilder::new(&agent_bin);
        for a in &agent_args {
            cmd.arg(a);
        }
        if let Some(r) = &resume {
            cmd.arg("--resume");
            cmd.arg(r);
        }
        cmd.cwd(&cwd);

        let mut child = match pair.slave.spawn_command(cmd) {
            Ok(c) => c,
            Err(e) => {
                error!(error = %e, bin = ?agent_bin, "spawn agent CLI failed");
                return (None, None);
            }
        };
        // Drop slave so the child is the sole owner; closing slave here
        // is required by portable-pty's API for the master read to see
        // EOF when the child exits.
        drop(pair.slave);

        let mut reader = match pair.master.try_clone_reader() {
            Ok(r) => r,
            Err(e) => {
                error!(error = %e, "clone PTY reader failed");
                let _ = child.kill();
                return (None, None);
            }
        };
        let writer = match pair.master.take_writer() {
            Ok(w) => w,
            Err(e) => {
                error!(error = %e, "take PTY writer failed");
                let _ = child.kill();
                return (None, None);
            }
        };
        let master = pair.master;

        // Reader thread: blocking reads from the PTY master, pushes into
        // the ring buffer via a tokio runtime handle.
        let state_for_reader = state.clone();
        let rt_for_reader = rt.clone();
        let reader_handle = std::thread::spawn(move || {
            let rt = rt_for_reader;
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = buf[..n].to_vec();
                        let s = state_for_reader.clone();
                        rt.block_on(async move {
                            record_chunk(&s, chunk).await;
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });

        // Writer thread: pulls stdin from the channel, writes to PTY
        // master synchronously.
        let writer_mutex = Arc::new(std::sync::Mutex::new(writer));
        let writer_for_stdin = writer_mutex.clone();
        let stdin_thread = std::thread::spawn(move || {
            while let Some(data) = stdin_rx.blocking_recv() {
                if let Ok(mut w) = writer_for_stdin.lock() {
                    if w.write_all(&data).is_err() {
                        break;
                    }
                    let _ = w.flush();
                }
            }
        });

        // Resize thread: applies resize requests to the PTY master.
        let master_for_resize = Arc::new(std::sync::Mutex::new(master));
        let master_for_resize_clone = master_for_resize.clone();
        let resize_thread = std::thread::spawn(move || {
            while let Some((rows, cols)) = resize_rx.blocking_recv() {
                if let Ok(m) = master_for_resize_clone.lock() {
                    let _ = m.resize(PtySize {
                        rows,
                        cols,
                        pixel_width: 0,
                        pixel_height: 0,
                    });
                }
            }
        });

        let status = child.wait().ok();
        // Signal the helper threads that the child is done.
        reader_handle.join().ok();
        // Drop master so any resize-thread iterations exit cleanly.
        drop(master_for_resize);
        // Closing stdin/resize senders happens when the connection loop
        // tears down at the same time; force the threads to terminate
        // by closing the channels through dropping our receivers — but
        // we already moved them in. The threads will exit when the
        // senders are dropped by the connection-loop side.
        let _ = stdin_thread;
        let _ = resize_thread;

        let (exit_code, signal) = match status {
            Some(s) => {
                let raw = s.exit_code();
                if s.success() || raw < 128 {
                    (Some(raw as i32), None)
                } else {
                    (None, Some((raw as i32) - 128))
                }
            }
            None => (None, None),
        };

        // Wake the connection writer so it sees the exit flag.
        let s = state.clone();
        rt.block_on(async move {
            *s.child_exited.lock().await = Some((exit_code, signal));
            s.notify.notify_waiters();
        });

        (exit_code, signal)
    }

    /// Drive a single accepted Core connection from the post-`Hello`
    /// point to disconnect. Returns when the connection drops; the
    /// outer accept loop is then free to take the next one.
    async fn run_connection(
        stream: UnixStream,
        state: Arc<State>,
        expected_cookie: [u8; 32],
        agent_kind: AgentKind,
        stdin_tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
        resize_tx: tokio::sync::mpsc::UnboundedSender<(u16, u16)>,
    ) {
        let (read_half, write_half) = stream.into_split();
        let read_half = Arc::new(Mutex::new(read_half));
        let write_half = Arc::new(Mutex::new(write_half));

        // 1. Expect Hello.
        let hello = {
            let mut r = read_half.lock().await;
            match read_frame(&mut *r).await {
                Ok(f) => f,
                Err(e) => {
                    debug!(error = %e, "connection closed before Hello");
                    return;
                }
            }
        };
        let (core_version, peer_cookie, last_seq) = match hello {
            HostFrame::Hello {
                core_version,
                expected_cookie,
                last_seq,
            } => (core_version, expected_cookie, last_seq),
            other => {
                warn!(?other, "first frame was not Hello; closing");
                return;
            }
        };

        // Constant-time compare.
        if expected_cookie.ct_eq(&peer_cookie).unwrap_u8() == 0 {
            warn!("cookie mismatch; closing connection");
            let mut w = write_half.lock().await;
            let _ = write_frame(&mut *w, &HostFrame::CookieMismatch).await;
            return;
        }

        // Claim the single-connection slot. A second concurrent Hello
        // gets `AlreadyConnected`.
        {
            let mut active = state.connection_active.lock().await;
            if *active {
                let mut w = write_half.lock().await;
                let _ = write_frame(&mut *w, &HostFrame::AlreadyConnected).await;
                return;
            }
            *active = true;
        }
        info!(core_version = %core_version, "Core connected");

        // Replay anything past last_seq, then send Ready with the
        // current high-water mark.
        let host_last_seq = {
            let ring = state.ring.lock().await;
            ring.last_seq()
        };
        let ready = HostFrame::Ready {
            agent_kind: agent_kind.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            external_session_id: None,
            last_seq: host_last_seq,
        };
        {
            let mut w = write_half.lock().await;
            if write_frame(&mut *w, &ready).await.is_err() {
                *state.connection_active.lock().await = false;
                return;
            }
        }
        let replay = {
            let ring = state.ring.lock().await;
            ring.replay_past(last_seq)
        };
        for chunk in replay {
            let frame = HostFrame::StdoutBytes {
                seq: chunk.seq,
                data: chunk.data,
            };
            let mut w = write_half.lock().await;
            if write_frame(&mut *w, &frame).await.is_err() {
                *state.connection_active.lock().await = false;
                return;
            }
        }

        // Writer task: pushes new ring chunks + drains on exit.
        let state_w = state.clone();
        let write_half_w = write_half.clone();
        let writer = tokio::spawn(async move {
            let mut watermark = host_last_seq;
            loop {
                // Snapshot the current state under the lock.
                let (chunks, exited) = {
                    let ring = state_w.ring.lock().await;
                    let chunks = ring.replay_past(watermark);
                    let exited = *state_w.child_exited.lock().await;
                    (chunks, exited)
                };
                for chunk in chunks {
                    watermark = chunk.seq.max(watermark);
                    let frame = HostFrame::StdoutBytes {
                        seq: chunk.seq,
                        data: chunk.data,
                    };
                    let mut w = write_half_w.lock().await;
                    if write_frame(&mut *w, &frame).await.is_err() {
                        return;
                    }
                }
                if let Some((exit_code, signal)) = exited {
                    let mut w = write_half_w.lock().await;
                    let send_result =
                        write_frame(&mut *w, &HostFrame::AgentExited { exit_code, signal }).await;
                    if send_result.is_ok() {
                        *state_w.delivered_exit.lock().await = true;
                        state_w.exit_delivered_notify.notify_waiters();
                    }
                    return;
                }
                state_w.notify.notified().await;
            }
        });

        // Reader task: pulls StdinBytes / Resize / Ping / Ack from the
        // Core and forwards them to the PTY or responds inline.
        let state_r = state.clone();
        let read_half_r = read_half.clone();
        let write_half_r = write_half.clone();
        let reader = tokio::spawn(async move {
            loop {
                let frame = {
                    let mut r = read_half_r.lock().await;
                    match read_frame(&mut *r).await {
                        Ok(f) => f,
                        Err(FrameError::Eof) => return,
                        Err(e) => {
                            debug!(error = %e, "read_frame failed; closing");
                            return;
                        }
                    }
                };
                match frame {
                    HostFrame::StdinBytes { data } => {
                        let _ = stdin_tx.send(data);
                    }
                    HostFrame::Resize { rows, cols } => {
                        let _ = resize_tx.send((rows, cols));
                    }
                    HostFrame::Ping => {
                        let mut w = write_half_r.lock().await;
                        if write_frame(&mut *w, &HostFrame::Pong).await.is_err() {
                            return;
                        }
                    }
                    HostFrame::Ack { seq } => {
                        let mut ring = state_r.ring.lock().await;
                        ring.prune_through(seq);
                    }
                    other => {
                        debug!(?other, "ignoring unexpected frame from Core");
                    }
                }
            }
        });

        let _ = tokio::join!(writer, reader);
        *state.connection_active.lock().await = false;
        info!("Core disconnected");
    }

    pub async fn main(cli: Cli) -> std::io::Result<i32> {
        // Decode cookie up front; bail fast on bad input.
        let cookie = decode_cookie(&cli.cookie)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

        let state = Arc::new(State::new());

        // Channels feeding the PTY supervisor thread.
        let (stdin_tx, stdin_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let (resize_tx, resize_rx) = tokio::sync::mpsc::unbounded_channel::<(u16, u16)>();

        // Start the PTY before binding the socket: if the agent fails to
        // spawn we want to surface the error in the host's exit code,
        // not silently sit on an empty UDS.
        let agent_kind = agent_kind_from_bin(&cli.agent_bin);
        let pty_handle = spawn_pty_task(&cli, state.clone(), stdin_rx, resize_rx);

        // Bind the UDS at 0600.
        let listener = bind_socket(&cli.socket).await?;
        info!(socket = ?cli.socket, "host bridge listening");

        // Accept loop. Runs concurrently with the PTY supervisor and
        // exits once the child is gone AND no Core is connected.
        let state_for_accept = state.clone();
        let stdin_tx_for_accept = stdin_tx.clone();
        let resize_tx_for_accept = resize_tx.clone();
        let agent_kind_for_accept = agent_kind.clone();
        let cookie_for_accept = cookie;
        let accept_loop = tokio::spawn(async move {
            // After the PTY child exits the host stays bound long enough
            // for a still-disconnected Core to land its first Hello and
            // drain the ring buffer (including the synthetic
            // `AgentExited` frame). The grace window ends early if a
            // Core actually receives `AgentExited` — the
            // surviving-host invariant is satisfied at that point.
            // While the loop is in grace it still accepts new
            // connections (a Core that connects late should still get
            // the buffered output).
            const POST_EXIT_GRACE: std::time::Duration = std::time::Duration::from_secs(30);
            let mut grace_deadline: Option<tokio::time::Instant> = None;
            loop {
                // Short-circuit: child exited AND a Core has acked
                // `AgentExited` → no further reason to stay bound.
                if grace_deadline.is_some() && *state_for_accept.delivered_exit.lock().await {
                    break;
                }
                let timeout = match grace_deadline {
                    Some(d) => tokio::time::sleep_until(d),
                    None => tokio::time::sleep(std::time::Duration::from_secs(3600)),
                };
                tokio::pin!(timeout);
                tokio::select! {
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((stream, _addr)) => {
                                let s = state_for_accept.clone();
                                let st = stdin_tx_for_accept.clone();
                                let rt = resize_tx_for_accept.clone();
                                let kind = agent_kind_for_accept.clone();
                                tokio::spawn(async move {
                                    run_connection(stream, s, cookie_for_accept, kind, st, rt).await;
                                });
                            }
                            Err(e) => {
                                warn!(error = %e, "accept failed");
                            }
                        }
                    }
                    _ = wait_for_child_exit(state_for_accept.clone()), if grace_deadline.is_none() => {
                        grace_deadline = Some(tokio::time::Instant::now() + POST_EXIT_GRACE);
                    }
                    _ = state_for_accept.exit_delivered_notify.notified(), if grace_deadline.is_some() => {
                        // Next loop iteration will see delivered_exit and break.
                    }
                    _ = &mut timeout, if grace_deadline.is_some() => {
                        break;
                    }
                }
            }
        });

        // Wait for the PTY supervisor to return.
        let (exit_code, signal) = match pty_handle.await {
            Ok(pair) => pair,
            Err(e) => {
                error!(error = %e, "pty supervisor join failed");
                (None, None)
            }
        };

        // Make sure the accept loop has noticed the child is gone and
        // stopped. The child_exited flag is set by the supervisor
        // before it returns, so the wait_for_child_exit guard fires.
        let _ = accept_loop.await;

        // Drop senders so the PTY supervisor's helper threads tear down.
        drop(stdin_tx);
        drop(resize_tx);

        // Write the final-info JSON.
        let last_lines = {
            let tail = state.tail.lock().await;
            tail_lines(&tail, 100)
        };
        let exited_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let info = FinalInfo {
            exit_code,
            signal,
            last_lines,
            external_session_id: None,
            exited_at_unix_ms,
        };
        if let Err(e) = write_final_info(&cli.final_info, &info).await {
            warn!(error = %e, path = ?cli.final_info, "write final-info failed");
        }

        // Best-effort socket cleanup.
        let _ = fs::remove_file(&cli.socket).await;

        Ok(0)
    }

    /// Resolves once the PTY child has been observed exiting. Used by
    /// the accept loop to break out without polling.
    async fn wait_for_child_exit(state: Arc<State>) {
        loop {
            {
                let guard = state.child_exited.lock().await;
                if guard.is_some() {
                    return;
                }
            }
            state.notify.notified().await;
        }
    }

    pub fn parse_cli() -> Cli {
        Cli::parse()
    }
}

fn main() {
    #[cfg(not(unix))]
    {
        eprintln!("concerto-agent-host: Windows ConPTY support is V1.0");
        std::process::exit(2);
    }
    #[cfg(unix)]
    {
        let cli = unix::parse_cli();
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .with_writer(std::io::stderr)
            .init();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        let code = rt
            .block_on(async { unix::main(cli).await })
            .unwrap_or_else(|e| {
                eprintln!("concerto-agent-host: {e}");
                1
            });
        std::process::exit(code);
    }
}
