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

// Re-export the Task 29 status/diff surface so the interface generator
// picks it up as part of `crates/gix-wrap/src/api.rs`.
pub use crate::diff::{diff_head, diff_to_main, DiffHunk, DiffKind, DiffPayload, FileDiff};
pub use crate::status::{status, StatusEntry, StatusReport, StatusState};

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

/// Clone strategy (Task 301, `design/02 §2`/`§3.1`).
///
/// Serializes to the existing `repositories.clone_strategy` TEXT values
/// (`full | blobless | treeless`, migration 0001) via [`Self::as_str`] /
/// [`std::str::FromStr`] / [`std::fmt::Display`]. An unknown string is a
/// hard error ([`Error::Git`]), never a silent fall-back to `Full`.
///
/// Filter-flag mapping (applied by [`clone_with_strategy`]):
/// - `Full` → no `--filter`
/// - `Blobless` → `--filter=blob:none` (commits + trees on disk, blobs lazy)
/// - `Treeless` → `--filter=tree:0` (commits on disk, trees + blobs lazy)
///
/// `Treeless` is hidden from every UI/recommendation surface for V1.0
/// (`design/02 §12 R-1`); it is reachable only when a caller passes it
/// explicitly. The size→strategy recommendation in [`estimate_repo_size`]
/// never returns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloneStrategy {
    Full,
    Blobless,
    Treeless,
}

impl CloneStrategy {
    /// Lowercase SQL/wire form — exactly the values the
    /// `repositories.clone_strategy` column accepts.
    pub fn as_str(self) -> &'static str {
        match self {
            CloneStrategy::Full => "full",
            CloneStrategy::Blobless => "blobless",
            CloneStrategy::Treeless => "treeless",
        }
    }

    /// The `--filter=…` argument for this strategy, or `None` for `Full`.
    fn filter_arg(self) -> Option<&'static str> {
        match self {
            CloneStrategy::Full => None,
            CloneStrategy::Blobless => Some("--filter=blob:none"),
            CloneStrategy::Treeless => Some("--filter=tree:0"),
        }
    }
}

impl std::str::FromStr for CloneStrategy {
    type Err = Error;

    /// Parse a `repositories.clone_strategy` TEXT value. An empty string
    /// maps to `Full` so V0.1 wire callers (who never set the field) keep
    /// their full-clone behaviour; any other unrecognized value is an
    /// `Error::Git` rather than a silent default.
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "" | "full" => Ok(CloneStrategy::Full),
            "blobless" => Ok(CloneStrategy::Blobless),
            "treeless" => Ok(CloneStrategy::Treeless),
            other => Err(Error::Git(format!(
                "unknown clone strategy {other:?} (expected one of: full, blobless, treeless)"
            ))),
        }
    }
}

impl std::fmt::Display for CloneStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Result of [`estimate_repo_size`] — the repo-level pre-clone probe.
///
/// Mirrors the on-wire `concerto.v1.SizeReport` shape (Task 301 lock). The
/// caller (the New-Project dialog, via the `EstimateRepoSize` RPC) renders
/// `recommended` + `recommend_sparse` and lets the user override.
///
/// `recommended` is one of `Full` / `Blobless` — `Treeless` is never
/// recommended (`design/02 §12 R-1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizeReport {
    /// Estimated total on-disk size of the remote's object database, in
    /// bytes. See [`estimate_repo_size`]'s doc-comment for the (FROZEN)
    /// approximation used when the remote does not advertise a true size.
    pub size_bytes: u64,
    /// Estimated reachable object count from the default branch HEAD.
    pub object_count: u64,
    /// Number of `refs/heads/*` branches advertised by `git ls-remote`.
    pub branch_count: u32,
    /// The strategy recommended by the `design/02 §3.5` heuristic.
    pub recommended: CloneStrategy,
    /// Whether the recommendation pairs the strategy with sparse checkout
    /// (the `> 10 GB → Blobless + Sparse` tier).
    pub recommend_sparse: bool,
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
///
/// Task 301: the body now delegates to the private [`clone_inner`] (shared
/// with [`clone_with_strategy`]); the public signature and observable
/// behaviour are unchanged.
pub async fn clone_full(url: &str, dest: &Path, progress: Option<ProgressSink>) -> Result<()> {
    let dest_str = dest.to_string_lossy().into_owned();
    let args = vec!["clone", "--progress", url, dest_str.as_str()];
    clone_inner(args, dest, progress).await
}

/// Clone `url` into `dest` with an explicit [`CloneStrategy`] and optional
/// sparse-checkout flags (Task 301, `design/02 §3.1`/`§7.1`).
///
/// Filter flags follow [`CloneStrategy::filter_arg`]:
/// `Blobless → --filter=blob:none`, `Treeless → --filter=tree:0`,
/// `Full → ` (none). When `with_sparse` is set the clone also gets
/// `--sparse --no-checkout` so the worktree lands empty — **Task 302**
/// runs `git sparse-checkout init --cone` + `set` into it; 301 only emits
/// the flags. `--progress` is always appended (the stderr parser keys off
/// it) and `GIT_TERMINAL_PROMPT=0` is inherited from [`cmd`].
///
/// `clone_full` is the `Full` + non-sparse case; this fn does not delegate
/// to it (both share the private [`clone_inner`] plumbing) so `clone_full`
/// stays byte-for-byte unchanged for back-compat.
pub async fn clone_with_strategy(
    url: &str,
    dest: &Path,
    strategy: CloneStrategy,
    with_sparse: bool,
    progress: Option<ProgressSink>,
) -> Result<()> {
    let dest_str = dest.to_string_lossy().into_owned();
    let mut args = vec!["clone", "--progress"];
    if let Some(filter) = strategy.filter_arg() {
        args.push(filter);
    }
    if with_sparse {
        // Empty worktree for Task 302's `sparse-checkout init --cone` to
        // populate (per the §7.1 first-time-clone sequence diagram).
        args.push("--sparse");
        args.push("--no-checkout");
    }
    args.push(url);
    args.push(dest_str.as_str());
    clone_inner(args, dest, progress).await
}

/// Shared clone plumbing for [`clone_full`] + [`clone_with_strategy`].
///
/// `args` is the full `git` argument vector (including the leading
/// `"clone"`). Progress, when supplied, is parsed off stderr and a
/// synthetic terminal `done` event is sent after a clean exit — identical
/// to the V0.1 `clone_full` behaviour this factored out.
async fn clone_inner(args: Vec<&str>, dest: &Path, progress: Option<ProgressSink>) -> Result<()> {
    // `git clone` accepts the destination as an argument; the working
    // directory must exist for the spawn but we tolerate `dest` not
    // existing yet — git creates it.
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| Error::Git(format!("create_dir_all({}): {e}", parent.display())))?;
    }
    let cwd = dest.parent().unwrap_or(Path::new("."));

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

        let result = cmd::run_streaming(&args, cwd, raw_tx).await;
        // Drop the raw_tx (held inside run_streaming) closes the channel;
        // wait for the parser task to drain before returning.
        let _ = parser_handle.await;
        result.map(|_| ())
    } else {
        cmd::run(&args, cwd).await.map(|_| ())
    }
}

/// Repo-level pre-clone size probe (Task 301, `design/02 §3.5`/`§7.1`).
///
/// Runs two cheap network round-trips against `url`, never a full clone:
///
/// 1. `git ls-remote --heads <url>` — counts `refs/heads/*` branches
///    (`branch_count`) and resolves the default-branch HEAD when
///    `ls-remote --symref` advertises it (falls back to the first head).
/// 2. A bare, blobless, single-branch, depth-bounded fetch into a throwaway
///    temp dir (`git clone --filter=blob:none --bare --no-checkout
///    --single-branch --depth=1`) followed by `git rev-list --objects
///    --count --all` over what landed, to count commit + tree objects.
///
/// **FROZEN approximation:** a true remote byte count is not cheaply
/// obtainable (git does not advertise repo size over the smart protocol),
/// so `size_bytes` is estimated as
/// `object_count * AVG_OBJECT_SIZE_BYTES` with `AVG_OBJECT_SIZE_BYTES =
/// 4096`. This is deliberately coarse — its only job is to bucket the repo
/// into the three `design/02 §3.5` size tiers; the real `< 30 s p50` clone
/// number is the Phase-3 Tier-3 checklist's job. Callers needing a precise
/// number measure post-clone via `git count-objects -v`.
///
/// The `design/02 §3.5` heuristic (FROZEN): `< 1 GB → Full`,
/// `1–10 GB → Blobless`, `> 10 GB → Blobless + Sparse`. `Treeless` is
/// never recommended (`§12 R-1`). A probe failure (private repo, offline)
/// surfaces as [`Error::Git`] — the caller falls back to a manual strategy
/// pick, never a default recommendation.
pub async fn estimate_repo_size(url: &str) -> Result<SizeReport> {
    // 1 GB / 10 GB tier boundaries, in bytes.
    const ONE_GB: u64 = 1024 * 1024 * 1024;
    const TEN_GB: u64 = 10 * ONE_GB;
    // FROZEN average-object-size constant for the byte estimate (see the
    // fn doc-comment). Only used to bucket into the size tiers.
    const AVG_OBJECT_SIZE_BYTES: u64 = 4096;

    // --- branch count + default-branch ref (ls-remote, one round-trip) ---
    let ls = cmd::run(&["ls-remote", "--symref", "--heads", url], Path::new(".")).await?;
    let branch_count = ls
        .stdout
        .lines()
        .filter(|l| l.contains("refs/heads/"))
        .filter(|l| !l.trim_start().starts_with("ref:"))
        .count() as u32;

    // --- object count: cheap blobless bare probe into a temp dir ---
    // `tempfile` is a dev-dep only; use a deterministic per-call dir under
    // the OS temp root so the probe leaves nothing behind on the happy path.
    let probe_dir = std::env::temp_dir().join(format!("concerto-size-probe-{}", uuid_v7_short()));
    let probe_str = probe_dir.to_string_lossy().into_owned();

    let object_count = {
        let probe = async {
            cmd::run(
                &[
                    "clone",
                    "--filter=blob:none",
                    "--bare",
                    "--no-checkout",
                    "--single-branch",
                    "--depth=1",
                    url,
                    probe_str.as_str(),
                ],
                Path::new("."),
            )
            .await?;
            let count_out =
                cmd::run(&["rev-list", "--objects", "--count", "--all"], &probe_dir).await?;
            count_out
                .stdout
                .trim()
                .parse::<u64>()
                .map_err(|e| Error::Git(format!("rev-list --count parse: {e}")))
        }
        .await;
        // Best-effort cleanup regardless of probe success.
        let _ = tokio::fs::remove_dir_all(&probe_dir).await;
        probe?
    };

    let size_bytes = object_count.saturating_mul(AVG_OBJECT_SIZE_BYTES);

    // --- design/02 §3.5 heuristic (FROZEN) ---
    let (recommended, recommend_sparse) = if size_bytes < ONE_GB {
        (CloneStrategy::Full, false)
    } else if size_bytes <= TEN_GB {
        (CloneStrategy::Blobless, false)
    } else {
        (CloneStrategy::Blobless, true)
    };

    Ok(SizeReport {
        size_bytes,
        object_count,
        branch_count,
        recommended,
        recommend_sparse,
    })
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

// ---------------------------------------------------------------------------
// Task 34: checkpoint plumbing — commit the worktree to a tree, store it as
// a commit, point a namespaced ref at it, and (on revert) hard-reset the
// worktree to a ref. Signatures FROZEN by `tasks/34 §"Public interface".
//
// All four helpers shell out to `git`. Going through gix's tree-builder /
// commit-creation API requires re-implementing the parent / author /
// committer plumbing that `git commit-tree` already does correctly, and the
// checkpoint code path is rare enough that the subprocess cost is invisible.
// The split helpers below mirror the design doc's revert sequence one-for-one:
//
//   commit_index → write the worktree as a tree + commit object, return OID
//   update_ref   → point `refs/concerto/checkpoints/...` at that OID
//   hard_reset   → reset the branch + worktree to a checkpoint ref
//   ref_exists   → cheap probe used by tests + the revert path
// ---------------------------------------------------------------------------

/// Snapshot the worktree at `repo_dir` as a commit and return its OID.
///
/// Implementation: `git add -A` (stage every tracked + untracked file)
/// inside a temporary index file so the visible HEAD-relative index is
/// untouched, `git write-tree` to materialize the tree, then
/// `git commit-tree <tree> -p HEAD -m <message>` to wrap it in a commit
/// object. HEAD is never moved.
///
/// The temp-index approach matches `git stash create`'s pattern (`man
/// git-stash` §"Discussion") — we get a commit-shaped snapshot without
/// disturbing the index the user (or another process) is editing.
///
/// `author` / `committer` are forced to a deterministic identity
/// (`Concerto <concerto@local>`) so checkpoint commits are
/// content-addressed (the same worktree state produces the same OID
/// modulo wall-clock) and stable across machines. The Unix-epoch
/// `GIT_*_DATE` envs avoid `git`'s local-time default which would make
/// commit OIDs depend on the user's TZ.
pub async fn commit_index(repo_dir: &Path, message: &str) -> Result<String> {
    // Use a per-call tempfile inside the repo's `.git` so it never
    // collides with the user's index. `git` resolves the path against
    // the working dir, so an absolute path is safest.
    let unique = uuid_v7_short();
    let tmp_index = repo_dir.join(format!(".git/concerto-checkpoint-index-{unique}"));
    let tmp_index_str = tmp_index.to_string_lossy().into_owned();

    // Read the current HEAD's tree into the temp index so `add -A` only
    // records changes relative to HEAD. `git read-tree` is the same
    // mechanism `git stash` uses for its snapshot.
    let head_oid = match cmd::run_with_env(
        &["rev-parse", "HEAD"],
        repo_dir,
        &[("GIT_INDEX_FILE", tmp_index_str.as_str())],
    )
    .await
    {
        Ok(out) => Some(out.stdout.trim().to_string()),
        // No HEAD yet (unborn branch) — fall through to a no-parent
        // commit. V0.1 workareas always have HEAD, but keep this branch
        // sound for the test harness's empty-init repos.
        Err(_) => None,
    };
    if let Some(head) = head_oid.as_deref() {
        cmd::run_with_env(
            &["read-tree", head],
            repo_dir,
            &[("GIT_INDEX_FILE", tmp_index_str.as_str())],
        )
        .await
        .map_err(|e| Error::Git(format!("checkpoint read-tree: {e}")))?;
    }

    // Stage everything (tracked changes + untracked) into the temp index.
    cmd::run_with_env(
        &["add", "-A"],
        repo_dir,
        &[("GIT_INDEX_FILE", tmp_index_str.as_str())],
    )
    .await
    .map_err(|e| Error::Git(format!("checkpoint add -A: {e}")))?;

    // Materialize the tree.
    let tree_out = cmd::run_with_env(
        &["write-tree"],
        repo_dir,
        &[("GIT_INDEX_FILE", tmp_index_str.as_str())],
    )
    .await
    .map_err(|e| Error::Git(format!("checkpoint write-tree: {e}")))?;
    let tree_oid = tree_out.stdout.trim().to_string();

    // Wrap the tree in a commit with HEAD as parent (when present).
    let env: &[(&str, &str)] = &[
        ("GIT_AUTHOR_NAME", "Concerto"),
        ("GIT_AUTHOR_EMAIL", "concerto@local"),
        ("GIT_COMMITTER_NAME", "Concerto"),
        ("GIT_COMMITTER_EMAIL", "concerto@local"),
    ];
    let commit_oid = if let Some(head) = head_oid.as_deref() {
        let out = cmd::run_with_env(
            &["commit-tree", &tree_oid, "-p", head, "-m", message],
            repo_dir,
            env,
        )
        .await
        .map_err(|e| Error::Git(format!("checkpoint commit-tree: {e}")))?;
        out.stdout.trim().to_string()
    } else {
        let out = cmd::run_with_env(&["commit-tree", &tree_oid, "-m", message], repo_dir, env)
            .await
            .map_err(|e| Error::Git(format!("checkpoint commit-tree (root): {e}")))?;
        out.stdout.trim().to_string()
    };

    // Best-effort: remove the temp index file. `git` doesn't auto-clean
    // and on Linux the file lives on disk forever otherwise.
    let _ = tokio::fs::remove_file(&tmp_index).await;

    Ok(commit_oid)
}

/// Point `ref_name` at `commit_oid` in the repository at `repo_dir`.
///
/// Wraps `git update-ref <name> <oid>`. The ref name SHOULD live under
/// `refs/concerto/...` so it never collides with user-facing porcelain
/// (`git branch`, `git tag` do not enumerate these by default).
pub async fn update_ref(repo_dir: &Path, ref_name: &str, commit_oid: &str) -> Result<()> {
    cmd::run(&["update-ref", ref_name, commit_oid], repo_dir)
        .await
        .map(|_| ())
}

/// `git reset --hard <ref_name>` at `repo_dir`. Resets the branch HEAD
/// and worktree to the named ref.
///
/// Used by the Agent Supervisor's `revert_to_checkpoint` path. The caller
/// is responsible for stopping any live agent session on the repo first
/// — a hard reset under a running agent corrupts the agent's expectation
/// of the worktree state.
pub async fn hard_reset(repo_dir: &Path, ref_name: &str) -> Result<()> {
    cmd::run(&["reset", "--hard", ref_name], repo_dir)
        .await
        .map(|_| ())
}

/// True iff `ref_name` resolves at `repo_dir`.
///
/// Wraps `git rev-parse --verify --quiet <ref_name>`. The `--quiet` flag
/// causes `git` to exit non-zero without writing to stderr for a missing
/// ref; we map that into `Ok(false)`. Any other failure (corrupt repo,
/// bad path) propagates as `Error::Git`.
pub async fn ref_exists(repo_dir: &Path, ref_name: &str) -> Result<bool> {
    match cmd::run(&["rev-parse", "--verify", "--quiet", ref_name], repo_dir).await {
        Ok(_) => Ok(true),
        // The shell-out helper folds non-zero exits into `Error::Git`.
        // A missing ref is the only expected non-zero, so treat the
        // string match defensively.
        Err(Error::Git(_)) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Tiny UUIDv7-derived suffix for ephemeral on-disk filenames. Lives
/// here so we don't have to pull `uuid` into `gix-wrap` for one call
/// site; the suffix only needs to be probabilistically unique within a
/// single checkpoint operation.
fn uuid_v7_short() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

// ---------------------------------------------------------------------------
// Task 28: fsmonitor + maintenance + performance config.
//
// All four helpers shell out to `git`. The function signatures are FROZEN
// by tasks/28 §"Public interface this task locks". Errors surface via
// `concerto_error::Error::Git`; the caller (Repository Manager) treats
// fsmonitor failures as "not supported on this filesystem" and disables
// the daemon for the repo gracefully — see
// `crates/core/src/repo_manager/fsmonitor.rs`.

/// Apply the four locked performance settings from `design/00 §6.3` to
/// the repo at `repo_dir`:
///
/// - `core.fsmonitor = true`
/// - `core.untrackedCache = true`
/// - `feature.manyFiles = true`
/// - `core.commitGraph = true`
///
/// Each setting goes through `git config <key> <value>` as a sequential
/// invocation. Using the shell-out path (rather than gix's config API)
/// keeps the operator-facing semantics 1:1 with what `git config -l`
/// reports — the same surface a human would touch.
///
/// Failure on any key short-circuits the call; the partial-application
/// case is acceptable because the supervisor retries on the next cycle.
pub async fn apply_perf_config(repo_dir: &Path) -> Result<()> {
    const KEYS: &[(&str, &str)] = &[
        ("core.fsmonitor", "true"),
        ("core.untrackedCache", "true"),
        ("feature.manyFiles", "true"),
        ("core.commitGraph", "true"),
    ];
    for (key, value) in KEYS {
        cmd::run(&["config", key, value], repo_dir).await?;
    }
    Ok(())
}

/// Start `git fsmonitor--daemon` for the repo at `repo_dir` and return
/// the daemon's PID.
///
/// The implementation spawns `git fsmonitor--daemon start`, which
/// daemonizes and exits cleanly once the worker is listening on its IPC
/// socket. We then ask `git fsmonitor--daemon status --json` for the
/// running PID — that subcommand is the documented way to recover the
/// daemon's PID after a start (the start command itself doesn't print
/// it).
///
/// If the daemon refuses to start (filesystem too exotic — NFS, certain
/// tmpfs, sandboxed FUSE mounts), the call surfaces an `Error::Git`. The
/// supervisor catches that error and disables fsmonitor for the repo per
/// `design/02 §8`.
pub async fn start_fsmonitor(repo_dir: &Path) -> Result<u32> {
    // `start` is idempotent — git short-circuits if a daemon is already
    // running for the repo. We tolerate its non-zero exits to surface the
    // stderr unchanged.
    cmd::run(&["fsmonitor--daemon", "start"], repo_dir).await?;

    let out = cmd::run(&["fsmonitor--daemon", "status"], repo_dir).await?;
    parse_fsmonitor_pid(&out.stdout).ok_or_else(|| {
        Error::Git(format!(
            "git fsmonitor--daemon status: could not parse PID from output: {}",
            out.stdout.trim()
        ))
    })
}

/// Probe whether `pid` refers to a live `git fsmonitor--daemon` process.
///
/// Uses the same `kill(pid, 0)` ESRCH probe as `crates/core/src/pid_file.rs`.
/// A live PID returns `true`; ESRCH (process gone) and any other errno
/// return `false`. EPERM is treated as "alive" because a permission
/// error proves the PID belongs to a real process.
#[cfg(unix)]
pub fn is_fsmonitor_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    // SAFETY: `kill(pid, 0)` with a positive PID is a no-op probe.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    // EPERM (1): the process exists but we lack permission to signal it.
    // Treat as alive — the PID is real.
    errno == libc::EPERM
}

/// Windows stub. V0.1 ships Unix only; the supervisor never reaches
/// this branch in the CI matrix. Returns `false` so an accidental call
/// disables the daemon gracefully.
#[cfg(not(unix))]
pub fn is_fsmonitor_alive(_pid: u32) -> bool {
    false
}

/// Stop the `git fsmonitor--daemon` for `repo_dir` if one is running.
///
/// Wraps `git fsmonitor--daemon stop`. Idempotent: when no daemon is
/// running, git exits non-zero with a benign "no daemon" message. We
/// treat that path as success because the post-condition (no daemon) is
/// already satisfied.
pub async fn stop_fsmonitor(repo_dir: &Path) -> Result<()> {
    match cmd::run(&["fsmonitor--daemon", "stop"], repo_dir).await {
        Ok(_) => Ok(()),
        // The caller's contract is "after this returns Ok, no daemon is
        // running". A failure-because-no-daemon is the same post-state.
        Err(Error::Git(_)) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Register OS-level scheduled maintenance via `git maintenance start`.
///
/// This installs a launchd plist on macOS (or a cron entry on Linux /
/// a scheduled task on Windows) that runs `git maintenance run` in the
/// background. Idempotent — safe to call on every Core start.
///
/// Errors are downgraded to a debug-level trace and the function still
/// returns `Ok(())`. The maintenance integration is non-essential to
/// correctness (it's a long-term optimisation), and CI environments
/// often lack the scheduler (`launchctl`, `crontab`) so a failure here
/// would otherwise force every test environment to special-case the
/// missing scheduler.
pub async fn register_maintenance(repo_dir: &Path) -> Result<()> {
    if let Err(e) = cmd::run(&["maintenance", "start"], repo_dir).await {
        tracing::debug!(
            error = %e,
            repo_dir = %repo_dir.display(),
            "git maintenance start failed; continuing without scheduled maintenance"
        );
    }
    Ok(())
}

/// Parse a PID out of `git fsmonitor--daemon status` output.
///
/// The exact output format varies across git versions:
///
/// - `"fsmonitor-daemon is watching '/path' (pid=12345)"` (most builds)
/// - `"fsmonitor-daemon is watching '/path' pid: 12345"` (older)
/// - `"daemon running (pid 12345)"` (variant)
///
/// We look for `pid` followed by `:`, `=`, or whitespace, and capture
/// the digits. Returns `None` when no PID is found.
fn parse_fsmonitor_pid(stdout: &str) -> Option<u32> {
    for line in stdout.lines() {
        let lower = line.to_lowercase();
        let mut idx = 0;
        while let Some(found) = lower[idx..].find("pid") {
            let after = &line[idx + found + 3..];
            // Skip the separator after `pid` (`:`, `=`, space, or `(`).
            let after = after.trim_start_matches([':', '=', ' ', '(', ')', '\t']);
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(pid) = digits.parse::<u32>() {
                if pid > 0 {
                    return Some(pid);
                }
            }
            idx = idx + found + 3;
        }
    }
    None
}

#[cfg(test)]
mod fsmonitor_tests {
    use super::*;

    #[test]
    fn parses_pid_with_equals() {
        assert_eq!(
            parse_fsmonitor_pid("fsmonitor-daemon is watching '/p' (pid=12345)"),
            Some(12345)
        );
    }

    #[test]
    fn parses_pid_with_colon() {
        assert_eq!(
            parse_fsmonitor_pid("fsmonitor-daemon is watching '/p' pid: 99"),
            Some(99)
        );
    }

    #[test]
    fn parses_pid_with_space() {
        assert_eq!(parse_fsmonitor_pid("daemon running (pid 4242)"), Some(4242));
    }

    #[test]
    fn rejects_no_pid() {
        assert_eq!(parse_fsmonitor_pid("nothing interesting here"), None);
    }
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
