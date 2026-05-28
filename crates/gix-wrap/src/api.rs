//! Public surface of `concerto-gix-wrap` (Task 18).
//!
//! Per the convention from Task 04, the interface generator scrapes this
//! file. The crate exposes a small set of V0.1 git operations:
//!
//! - [`clone_full`] — full clone (no sparse, no blobless) via shell-out.
//! - [`fetch`] — incremental fetch via shell-out.
//! - [`list_branches`] — local + remote refs via `gix`.
//! - [`rev_parse_head`] — HEAD commit OID via `gix`.
//! - [`worktree_add`] — `git worktree add` shell-out.
//!
//! Internal routing follows `design/02 §3.1`: clone / worktree-add go
//! through shell-out (where git's own subcommand semantics are
//! authoritative); the read-only fast paths (`rev-parse`, `list branches`)
//! use `gix`. The hybrid stack is the same one design/02 specifies.
//!
//! Errors flow through [`concerto_error::Error::Git`] — see
//! `crates/error/src/api.rs`. The `Git(String)` variant intentionally
//! flattens `gix`'s deep error tree into a stringified message; the
//! shape is appropriate for a CLI/UI surface and avoids leaking unstable
//! `gix` types across the crate boundary.

use std::path::{Path, PathBuf};

use concerto_error::{Error, Result};
use tokio::sync::mpsc;

use crate::cmd;

/// Channel sender used to surface clone progress upstream.
///
/// Bounded at 32 by callers — under backpressure the clone path drops
/// the oldest events rather than blocking the subprocess drainer. See
/// `crates/core/src/repo_manager` for the RPC plumbing that wires this
/// sender to the gRPC `Repositories.Clone` stream.
pub type ProgressSink = mpsc::Sender<CloneProgressEvent>;

/// One progress event derived from `git clone`'s stderr.
///
/// Mirrors the on-wire `concerto.v1.CloneProgress` shape (V0.1 lock).
/// Tasks downstream may add fields without breaking the type alias here —
/// the gRPC layer is the single canonical source for wire compatibility.
#[derive(Debug, Clone, Default)]
pub struct CloneProgressEvent {
    /// Short label parsed from the leading text of the progress line,
    /// e.g. `"receiving objects"`, `"resolving deltas"`, `"updating files"`.
    pub phase: String,
    /// Count of objects so far (parsed from `N/T`). Zero when unparsed.
    pub objects_received: u64,
    /// Total objects in the operation (parsed from `N/T`). Zero when unparsed.
    pub total_objects: u64,
    /// Best-effort byte count from a `KiB`/`MiB`/`GiB` suffix on the line.
    /// Zero when not present.
    pub bytes_received: u64,
    /// True on the synthetic terminal event emitted after the subprocess
    /// exits cleanly.
    pub done: bool,
}

/// One branch reference returned by [`list_branches`].
///
/// V0.1 returns local + remote refs that point at a commit OID. The
/// `is_remote` flag separates the two; the caller (Workspace Mgr) uses
/// it to default the workarea's default branch from `origin/HEAD` when
/// the local HEAD is detached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchRef {
    pub name: String,
    pub commit: String,
    pub is_remote: bool,
}

/// Summary of a [`fetch`] call.
///
/// V0.1 reports nothing beyond success — the field is reserved for
/// future telemetry (objects fetched, bytes received). Kept as a
/// struct so adding fields stays additive at the Rust API surface.
#[derive(Debug, Clone, Default)]
pub struct FetchReport {
    /// Whether any refs were updated. False when the local copy was
    /// already at the remote tip.
    pub updated: bool,
}

/// Full clone of `url` into `dest`.
///
/// Wraps `git clone <url> <dest>`. Progress is streamed to `progress`
/// when supplied; missing senders simply discard the events. The clone
/// completes with `dest/.git` populated and a checked-out worktree at
/// `dest/` itself — this matches `design/02 §4`'s layout for V0.1 (we
/// keep the worktree out of the path for now; sparse / bare layouts
/// arrive with Tasks 28+ and V1.0).
///
/// `GIT_TERMINAL_PROMPT=0` is set on the subprocess env so missing
/// credentials surface as a clean error instead of blocking on `tty`.
pub async fn clone_full(url: &str, dest: &Path, progress: Option<ProgressSink>) -> Result<()> {
    // `git clone` accepts the destination as an argument; the working
    // directory must exist for the spawn but we tolerate `dest` not
    // existing yet — git creates it.
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| Error::Git(format!("create_dir_all({}): {e}", parent.display())))?;
    }
    let cwd = dest.parent().unwrap_or(Path::new("."));

    let dest_str = dest.to_string_lossy().into_owned();
    let args = &["clone", "--progress", url, dest_str.as_str()];

    if let Some(sink) = progress {
        // Bounded raw-line channel between the subprocess drain and the
        // progress parser. 32 matches the bound documented on the public
        // `ProgressSink` alias.
        let (raw_tx, mut raw_rx) = mpsc::channel::<String>(32);
        let parser_handle = tokio::spawn(async move {
            while let Some(line) = raw_rx.recv().await {
                if let Some(event) = progress::parse_line(&line) {
                    // Best-effort: drop the event if the downstream
                    // receiver is slow.
                    let _ = sink.try_send(event);
                }
            }
            // Synthetic terminal event so consumers know the stream is
            // finished. Best-effort send.
            let _ = sink.try_send(CloneProgressEvent {
                phase: "done".to_string(),
                done: true,
                ..Default::default()
            });
        });

        let result = cmd::run_streaming(args, cwd, raw_tx).await;
        // Drop the raw_tx (held inside run_streaming) closes the channel;
        // wait for the parser task to drain before returning.
        let _ = parser_handle.await;
        result.map(|_| ())
    } else {
        cmd::run(args, cwd).await.map(|_| ())
    }
}

/// Incremental fetch on an existing repo at `repo_dir`.
///
/// Wraps `git fetch --all --prune`. V0.1 always uses shell-out per
/// `design/02 §3.1`'s routing table footnote ("gix when available, git
/// shell-out as fallback") — the gix path is the V1.0 follow-on.
pub async fn fetch(repo_dir: &Path) -> Result<FetchReport> {
    let out = cmd::run(&["fetch", "--all", "--prune"], repo_dir).await?;
    // `git fetch` writes a non-empty stderr when refs change; empty
    // stderr means already up-to-date. Good enough for V0.1's
    // `FetchReport.updated`.
    let updated = !out.stderr.trim().is_empty();
    Ok(FetchReport { updated })
}

/// List local + remote branch refs at `repo_dir` via `gix`.
///
/// Refs that don't point at a commit (tags, annotated tag refs) are
/// skipped silently — `BranchRef.commit` only carries a 40-char OID.
pub async fn list_branches(repo_dir: &Path) -> Result<Vec<BranchRef>> {
    let repo_dir = repo_dir.to_path_buf();
    // `gix::open` is blocking; run on a worker so we don't park the
    // tokio scheduler on a slow disk.
    tokio::task::spawn_blocking(move || list_branches_blocking(&repo_dir))
        .await
        .map_err(|e| Error::Git(format!("list_branches: join error: {e}")))?
}

fn list_branches_blocking(repo_dir: &Path) -> Result<Vec<BranchRef>> {
    let repo = gix::open(repo_dir).map_err(|e| Error::Git(format!("gix::open: {e}")))?;
    let platform = repo
        .references()
        .map_err(|e| Error::Git(format!("references: {e}")))?;

    let mut out = Vec::new();
    // Local heads.
    let locals = platform
        .local_branches()
        .map_err(|e| Error::Git(format!("local_branches: {e}")))?;
    for r in locals {
        let r = r.map_err(|e| Error::Git(format!("local branch iter: {e}")))?;
        let name = r.name().shorten().to_string();
        let oid = match r.try_id() {
            Some(id) => id.to_string(),
            None => continue,
        };
        out.push(BranchRef {
            name,
            commit: oid,
            is_remote: false,
        });
    }
    // Remote-tracking heads.
    let remotes = platform
        .remote_branches()
        .map_err(|e| Error::Git(format!("remote_branches: {e}")))?;
    for r in remotes {
        let r = r.map_err(|e| Error::Git(format!("remote branch iter: {e}")))?;
        let name = r.name().shorten().to_string();
        let oid = match r.try_id() {
            Some(id) => id.to_string(),
            None => continue,
        };
        out.push(BranchRef {
            name,
            commit: oid,
            is_remote: true,
        });
    }

    // Deterministic order for callers + tests.
    out.sort_by(|a, b| (a.is_remote, &a.name).cmp(&(b.is_remote, &b.name)));
    Ok(out)
}

/// Return the OID at HEAD (`gix`-backed `rev-parse HEAD`).
///
/// Errors when HEAD is unborn (fresh repo with no commits) or when the
/// path is not a git repository at all.
pub async fn rev_parse_head(repo_dir: &Path) -> Result<String> {
    let repo_dir: PathBuf = repo_dir.to_path_buf();
    tokio::task::spawn_blocking(move || rev_parse_head_blocking(&repo_dir))
        .await
        .map_err(|e| Error::Git(format!("rev_parse_head: join error: {e}")))?
}

fn rev_parse_head_blocking(repo_dir: &Path) -> Result<String> {
    let repo = gix::open(repo_dir).map_err(|e| Error::Git(format!("gix::open: {e}")))?;
    let head = repo.head().map_err(|e| Error::Git(format!("head: {e}")))?;
    let id = head
        .into_peeled_id()
        .map_err(|e| Error::Git(format!("head peel: {e}")))?;
    Ok(id.to_string())
}

/// Create a worktree at `dest` checked out to `branch`, using `repo_dir`'s
/// shared object database.
///
/// Wraps `git worktree add -B <branch> <dest>`. The `-B` form creates
/// the branch if it does not exist and resets it if it does — the
/// design intent is "give me a worktree on this branch, creating as
/// needed", matching Workspace Manager's needs.
pub async fn worktree_add(repo_dir: &Path, branch: &str, dest: &Path) -> Result<()> {
    let dest_str = dest.to_string_lossy().into_owned();
    cmd::run(
        &["worktree", "add", "-B", branch, dest_str.as_str()],
        repo_dir,
    )
    .await
    .map(|_| ())
}

/// Stderr-line → [`CloneProgressEvent`] parser.
///
/// `git clone`'s progress format is stable enough for V0.1 — the lines
/// we care about look like:
///
/// ```text
/// Receiving objects:  42% (210/500), 12.34 MiB | 5.67 MiB/s
/// Resolving deltas: 100% (37/37), done.
/// Updating files: 100% (123/123), done.
/// ```
///
/// Anything we can't parse is silently dropped (returns `None`). The
/// consumer treats progress as best-effort.
pub mod progress {
    use super::CloneProgressEvent;

    /// Try to parse a single stderr line.
    pub fn parse_line(line: &str) -> Option<CloneProgressEvent> {
        let trimmed = line.trim();
        // Phase label is everything before the first `:` — git uses
        // that format consistently.
        let (phase_raw, rest) = trimmed.split_once(':')?;
        let phase = phase_raw.trim().to_lowercase();
        if !is_known_phase(&phase) {
            return None;
        }
        let rest = rest.trim_start();

        // Pull `(N/T)` if present.
        let (objects_received, total_objects) = parse_n_over_t(rest).unwrap_or((0, 0));
        // Pull a size suffix like `12.34 MiB` if present.
        let bytes_received = parse_bytes(rest).unwrap_or(0);

        Some(CloneProgressEvent {
            phase,
            objects_received,
            total_objects,
            bytes_received,
            done: false,
        })
    }

    fn is_known_phase(s: &str) -> bool {
        matches!(
            s,
            "receiving objects"
                | "resolving deltas"
                | "updating files"
                | "counting objects"
                | "compressing objects"
                | "remote"
                | "writing objects"
                | "checking out files"
        )
    }

    fn parse_n_over_t(rest: &str) -> Option<(u64, u64)> {
        // Find the substring between `(` and `)`. Could appear multiple
        // times; we want the first.
        let open = rest.find('(')?;
        let close = rest[open + 1..].find(')')?;
        let inner = &rest[open + 1..open + 1 + close];
        let (n, t) = inner.split_once('/')?;
        let n = n.trim().parse::<u64>().ok()?;
        let t = t.trim().parse::<u64>().ok()?;
        Some((n, t))
    }

    fn parse_bytes(rest: &str) -> Option<u64> {
        // Scan for a number followed by a unit. Naive but adequate
        // for git's well-formed output.
        let mut tokens = rest.split(|c: char| c.is_whitespace() || c == ',');
        while let Some(tok) = tokens.next() {
            if let Ok(value) = tok.parse::<f64>() {
                if let Some(unit) = tokens.next() {
                    let mult = match unit.trim_end_matches('|') {
                        "B" => 1u64,
                        "KiB" => 1024,
                        "MiB" => 1024 * 1024,
                        "GiB" => 1024 * 1024 * 1024,
                        _ => continue,
                    };
                    return Some((value * mult as f64) as u64);
                }
            }
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_receiving_objects_line() {
            let ev = parse_line("Receiving objects:  42% (210/500), 12.34 MiB | 5.67 MiB/s")
                .expect("parsed");
            assert_eq!(ev.phase, "receiving objects");
            assert_eq!(ev.objects_received, 210);
            assert_eq!(ev.total_objects, 500);
            assert!(ev.bytes_received > 0);
            assert!(!ev.done);
        }

        #[test]
        fn parses_resolving_deltas_line() {
            let ev = parse_line("Resolving deltas: 100% (37/37), done.").expect("parsed");
            assert_eq!(ev.phase, "resolving deltas");
            assert_eq!(ev.objects_received, 37);
            assert_eq!(ev.total_objects, 37);
        }

        #[test]
        fn drops_unknown_phase() {
            assert!(parse_line("warning: something").is_none());
        }
    }
}
