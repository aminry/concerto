//! Shell-out helper for the `git` CLI.
//!
//! Centralizes the subprocess-spawn pattern used by every shell-out path
//! in this crate (clone, worktree add, …) per `design/02 §3.1`. The
//! helper:
//!
//! - Disables credential prompts via `GIT_TERMINAL_PROMPT=0` so missing
//!   credentials fail fast instead of blocking on `tty` for input.
//! - Captures stdout and stderr verbatim. Callers that want to stream
//!   stderr to a progress sink (the clone path) use [`run_streaming`]
//!   instead; the simpler [`run`] is good enough for one-shot commands.
//! - Maps non-zero exit codes onto [`concerto_error::Error::Git`] with
//!   the captured stderr embedded so error messages stay actionable.

use std::path::Path;
use std::process::Stdio;

use concerto_error::{Error, Result};
use tokio::io::BufReader;
use tokio::process::Command;
use tokio::sync::mpsc;

/// Output of a one-shot `git` invocation.
#[derive(Debug, Clone)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
}

/// Run `git <args>` with `cwd` as the working directory.
///
/// Stdout and stderr are captured (no streaming). Non-zero exit codes map
/// onto `Error::Git("<command>: exit <code>: <stderr>")` — the embedded
/// stderr is the most useful clue when a git command rejects an input.
///
/// Use [`run_streaming`] when the caller needs each stderr line as it
/// arrives (clone progress).
pub async fn run(args: &[&str], cwd: &Path) -> Result<Output> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let output = cmd.output().await.map_err(|e| {
        Error::Git(format!(
            "git {}: failed to spawn: {e}",
            args.first().copied().unwrap_or("<empty>")
        ))
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if !output.status.success() {
        return Err(Error::Git(format!(
            "git {}: exit {}: {}",
            args.first().copied().unwrap_or("<empty>"),
            output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "<signal>".to_string()),
            stderr.trim()
        )));
    }

    Ok(Output { stdout, stderr })
}

/// Run `git <args>` with extra env vars set on the subprocess.
///
/// Same shape as [`run`] but the caller supplies a slice of `(key,
/// value)` pairs that get applied via `Command::env` before the spawn.
/// Used by the Task 34 checkpoint path to point `git` at a temp index
/// file (`GIT_INDEX_FILE`) and a deterministic author/committer identity
/// without polluting the surrounding shell environment.
pub async fn run_with_env(args: &[&str], cwd: &Path, env_pairs: &[(&str, &str)]) -> Result<Output> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in env_pairs {
        cmd.env(k, v);
    }

    let output = cmd.output().await.map_err(|e| {
        Error::Git(format!(
            "git {}: failed to spawn: {e}",
            args.first().copied().unwrap_or("<empty>")
        ))
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if !output.status.success() {
        return Err(Error::Git(format!(
            "git {}: exit {}: {}",
            args.first().copied().unwrap_or("<empty>"),
            output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "<signal>".to_string()),
            stderr.trim()
        )));
    }

    Ok(Output { stdout, stderr })
}

/// Run `git <args>` feeding `stdin_data` on the subprocess stdin.
///
/// Same capture + non-zero-exit semantics as [`run`], but the child's
/// stdin is a pipe the helper writes `stdin_data` into (then closes) so
/// commands that read a newline-delimited object list off stdin —
/// `git cat-file --batch-check` for the Task 304 prewarm path — don't
/// blow the argv length limit on a large cone.
pub async fn run_with_stdin(args: &[&str], cwd: &Path, stdin_data: &str) -> Result<Output> {
    use tokio::io::AsyncWriteExt;

    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| {
        Error::Git(format!(
            "git {}: failed to spawn: {e}",
            args.first().copied().unwrap_or("<empty>")
        ))
    })?;

    // Write the payload and close stdin so the child sees EOF. Take the
    // handle out so it drops (closing the pipe) before we await output.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(stdin_data.as_bytes())
            .await
            .map_err(|e| Error::Git(format!("git {}: stdin write: {e}", args[0])))?;
        // Explicit drop closes the pipe → child reads EOF.
        drop(stdin);
    }

    let output = child.wait_with_output().await.map_err(|e| {
        Error::Git(format!(
            "git {}: wait: {e}",
            args.first().copied().unwrap_or("<empty>")
        ))
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if !output.status.success() {
        return Err(Error::Git(format!(
            "git {}: exit {}: {}",
            args.first().copied().unwrap_or("<empty>"),
            output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "<signal>".to_string()),
            stderr.trim()
        )));
    }

    Ok(Output { stdout, stderr })
}

/// Run `git <args>` with stderr streamed line-by-line to `progress_tx`.
///
/// Returns an [`Output`] whose `stderr` is the concatenation of every
/// streamed line so callers wanting the post-mortem still have it. The
/// channel is `mpsc::Sender<String>` so the receiver can apply
/// backpressure if it falls behind — [`progress::parse_line`]'s mpsc has
/// a fixed bound of 32, with `try_send` so the clone path drops old
/// progress under load rather than blocking.
pub async fn run_streaming(
    args: &[&str],
    cwd: &Path,
    progress_tx: mpsc::Sender<String>,
) -> Result<Output> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| {
        Error::Git(format!(
            "git {}: failed to spawn: {e}",
            args.first().copied().unwrap_or("<empty>")
        ))
    })?;

    // Drain stderr line-by-line. Git uses CR rather than LF for in-place
    // progress updates (e.g. `Receiving objects:  42% ...`), so we treat
    // both as line boundaries via a small `read_progress_line` helper.
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Git("stderr pipe unavailable".to_string()))?;
    let mut reader = BufReader::new(stderr);
    let mut collected = String::new();
    loop {
        let mut line = String::new();
        match read_progress_line(&mut reader, &mut line).await {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim_end_matches(['\n', '\r']).to_string();
                if !trimmed.is_empty() {
                    // Best-effort: drop the event under backpressure.
                    let _ = progress_tx.try_send(trimmed.clone());
                    collected.push_str(&trimmed);
                    collected.push('\n');
                }
            }
            Err(e) => {
                return Err(Error::Git(format!("stderr read error: {e}")));
            }
        }
    }

    // Drain stdout too so the child can exit cleanly even when it wrote
    // (it usually doesn't for clone/worktree, but be safe).
    let stdout_bytes = if let Some(mut s) = child.stdout.take() {
        let mut buf = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut s, &mut buf).await;
        String::from_utf8_lossy(&buf).into_owned()
    } else {
        String::new()
    };

    let status = child
        .wait()
        .await
        .map_err(|e| Error::Git(format!("git wait: {e}")))?;

    if !status.success() {
        return Err(Error::Git(format!(
            "git {}: exit {}: {}",
            args.first().copied().unwrap_or("<empty>"),
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "<signal>".to_string()),
            collected.trim()
        )));
    }

    Ok(Output {
        stdout: stdout_bytes,
        stderr: collected,
    })
}

/// Read either a `\n`- or `\r`-terminated line from `reader` into `buf`.
///
/// `BufRead::read_line` splits only on `\n`. Git's progress output uses
/// `\r` to overwrite the same terminal line in place, so a pure
/// LF-terminated reader would coalesce every progress update into one
/// gigantic "line". This shim returns at the first occurrence of either
/// terminator. Returns the number of bytes read; 0 means EOF.
async fn read_progress_line<R>(
    reader: &mut BufReader<R>,
    buf: &mut String,
) -> std::io::Result<usize>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut byte_buf = [0u8; 1];
    let mut bytes_read = 0;
    loop {
        use tokio::io::AsyncReadExt;
        let n = reader.read(&mut byte_buf).await?;
        if n == 0 {
            return Ok(bytes_read);
        }
        bytes_read += 1;
        let ch = byte_buf[0] as char;
        buf.push(ch);
        if ch == '\n' || ch == '\r' {
            return Ok(bytes_read);
        }
    }
}
