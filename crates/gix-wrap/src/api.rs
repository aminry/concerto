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

/// Progress of an in-flight [`prewarm_blobs_in_cone`] materialization.
///
/// Mirrors the on-wire `concerto.v1.PrewarmProgress` shape (Task 304 lock,
/// `PHASE3_PLANNING §4.6`): `blobs_fetched` / `blobs_total` are object
/// counts, `done` is set on the terminal event. The Rust struct is the
/// single source the Core's `RepoManager` re-shapes onto the gRPC stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrewarmProgressEvent {
    /// Blobs materialized so far (cumulative across cone chunks).
    pub blobs_fetched: u64,
    /// Total in-cone blob OIDs discovered at `commit`.
    pub blobs_total: u64,
    /// True on the terminal event after the last chunk is materialized
    /// (or after a clean cancellation).
    pub done: bool,
}

/// File count + disk-size estimate for a sparse cone, read from the git
/// index (Task 305, `design/02 §3.2`/`§5.1`, the `ConeProbe → gix` arrow in
/// `§6`).
///
/// Mirrors the on-wire `concerto.v1.ConeStats` shape. `file_count` is the
/// number of tracked file entries the cone would materialize; `disk_size_bytes`
/// is the sum of those entries' recorded sizes in the index. See
/// [`cone_index_stats`] for the (FROZEN) estimate basis.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConeStats {
    /// Tracked file entries under the cone prefixes (or all tracked files
    /// when `cone_paths` is empty). Collapsed sparse-index directory entries
    /// (out-of-cone trees) are NOT counted — only true file (blob) entries.
    pub file_count: u64,
    /// Sum of the counted entries' recorded sizes from the index, in bytes.
    /// An order-of-magnitude estimate: a blobless clone's not-yet-fetched
    /// blobs carry a recorded size of 0, so this is a lower bound until the
    /// blobs are materialized (`design/02 §3.2` wants order-of-magnitude).
    pub disk_size_bytes: u64,
}

/// Read the git index at `repo_dir` and compute the [`ConeStats`] for the
/// file entries under `cone_paths` (Task 305, `design/02 §3.2`/`§5.1`).
///
/// **Reads the index, not the filesystem** (`design/02 §3.2`: "computes
/// (file count, disk size) from the git index"). Opens the repo via `gix`,
/// decodes the (possibly sparse) index, and for every tracked **file** entry
/// whose repo-root-relative path falls under one of `cone_paths`:
/// counts it and adds its recorded `entry.stat.size`.
///
/// **Cone prefix semantics.** A `cone_paths` entry is a directory prefix
/// (forward-slash, repo-root-relative, leading/trailing `/` tolerated); an
/// entry matches when its path equals the prefix or begins with
/// `<prefix>/`. An empty `cone_paths` slice counts **every** tracked file
/// entry (the caller resolves the cone-defaults fall-back before calling).
///
/// **Sparse-index honesty (`design/02 §3.2`, the task's sparse note).** On a
/// sparse-cone repo the index is a *sparse* index: out-of-cone trees are
/// collapsed to single directory entries (`entry.mode` is a tree, not a
/// blob). Those collapsed directory entries are skipped — only true file
/// (blob/symlink/commit-gitlink) entries are counted, so the count reflects
/// "files the cone materializes," independent of which blobs are currently
/// fetched. (A cone broader than what is materialized still counts correctly
/// because the in-cone file entries are present in the index regardless of
/// blob fetch state.)
///
/// **FROZEN estimate basis.** `disk_size_bytes` is summed from the index's
/// recorded per-entry `stat.size`. For a blobless clone a not-yet-fetched
/// blob's recorded size is 0, so the byte total is a lower bound — the
/// picker (Task 322) wants an order-of-magnitude, not an exact footprint
/// (`design/02 §3.2`). This basis is FROZEN.
///
/// Runs the blocking `gix` index decode on a worker thread. A repo that is
/// not a git repository, or whose index cannot be decoded, surfaces as
/// [`Error::Git`].
pub async fn cone_index_stats(repo_dir: &Path, cone_paths: &[String]) -> Result<ConeStats> {
    let repo_dir = repo_dir.to_path_buf();
    let cones = cone_paths.to_vec();
    tokio::task::spawn_blocking(move || cone_index_stats_blocking(&repo_dir, &cones))
        .await
        .map_err(|e| Error::Git(format!("cone_index_stats: join error: {e}")))?
}

fn cone_index_stats_blocking(repo_dir: &Path, cone_paths: &[String]) -> Result<ConeStats> {
    let repo = gix::open(repo_dir).map_err(|e| Error::Git(format!("gix::open: {e}")))?;
    let index = repo
        .open_index()
        .map_err(|e| Error::Git(format!("open_index: {e}")))?;

    // Normalize cone prefixes once: strip leading/trailing `/`, drop empties
    // (an empty/`.` prefix means "the whole tree", which we model as "no
    // filter" below). Done once so the per-entry loop stays cheap on a large
    // index.
    let prefixes: Vec<&str> = cone_paths
        .iter()
        .map(|c| c.trim_start_matches('/').trim_end_matches('/'))
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    // An explicit empty cone (or a cone of only `.`/`/`) means the whole
    // tree: when the caller passed paths but they all normalized away, that
    // is "top-level + everything", so treat it as no filter too.
    let count_all = prefixes.is_empty();

    let mut file_count: u64 = 0;
    let mut disk_size_bytes: u64 = 0;
    for entry in index.entries() {
        // Skip sparse-index collapsed directory entries + submodule gitlinks
        // — only count true file-content entries (`design/02 §3.2`: count the
        // files the cone would materialize, not the collapsed out-of-cone
        // trees). A regular index entry is `FILE`, `FILE_EXECUTABLE`, or
        // `SYMLINK`; `DIR` is the sparse-collapsed tree and `COMMIT` is a
        // submodule gitlink.
        use gix::index::entry::Mode;
        if !matches!(
            entry.mode,
            Mode::FILE | Mode::FILE_EXECUTABLE | Mode::SYMLINK
        ) {
            continue;
        }
        let path = entry.path(&index);
        let path = match std::str::from_utf8(path) {
            Ok(p) => p,
            // A non-UTF-8 path can't match a forward-slash cone prefix; count
            // it only in the no-filter case (it is still a tracked file).
            Err(_) => {
                if count_all {
                    file_count += 1;
                    disk_size_bytes = disk_size_bytes.saturating_add(u64::from(entry.stat.size));
                }
                continue;
            }
        };
        if count_all || path_under_any_prefix(path, &prefixes) {
            file_count += 1;
            disk_size_bytes = disk_size_bytes.saturating_add(u64::from(entry.stat.size));
        }
    }

    Ok(ConeStats {
        file_count,
        disk_size_bytes,
    })
}

/// True iff `path` (a forward-slash, repo-root-relative index path) sits
/// under one of `prefixes` — i.e. equals a prefix exactly or begins with
/// `<prefix>/`. The `/`-boundary check stops `app` from matching `apple/…`.
fn path_under_any_prefix(path: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| {
        path == *prefix
            || (path.len() > prefix.len()
                && path.as_bytes()[prefix.len()] == b'/'
                && path.starts_with(prefix))
    })
}

/// List the IMMEDIATE (non-recursive) children of `path` at `git_ref` in
/// the repository at `repo_dir` (design/02 §3.2). Backs the browsable
/// repo-tree picker: the desktop "Choose directories for the sparse
/// checkout" step lazily expands one directory at a time, so this lists
/// only direct children rather than recursing the whole tree.
///
/// Returns `(full_path, is_dir)` pairs where `full_path` is the
/// repo-root-relative path (`git ls-tree`'s `%(path)` is already the full
/// path) and `is_dir` is true for a `tree` objecttype. A `blob` (file) and
/// a `commit` (submodule gitlink) are both reported as leaves
/// (`is_dir=false`) — cone mode selects directories, so files/submodules
/// are shown for context only.
///
/// `path` is repo-root-relative; `""` lists the root entries (the pathspec
/// is omitted so `git ls-tree` returns the top-level tree). For a non-empty
/// `path` a trailing `/` is appended to the pathspec so `git ls-tree`
/// descends into that directory and lists its children (rather than echoing
/// the directory entry itself). An empty `git_ref` falls back to `"HEAD"`.
///
/// Entries are returned trees-first, then blobs/submodules, each
/// alphabetical by full path — a stable order the renderer can show without
/// re-sorting.
///
/// A repo that is not a git repository, an unknown ref, or an unreadable
/// tree surfaces as [`Error::Git`].
pub async fn list_tree(repo_dir: &Path, git_ref: &str, path: &str) -> Result<Vec<(String, bool)>> {
    let r#ref = if git_ref.is_empty() { "HEAD" } else { git_ref };
    // Normalize the directory prefix: strip surrounding slashes so we build
    // a clean `<dir>/` pathspec. An empty/`.`/`/` path lists the root.
    let dir = path.trim_start_matches('/').trim_end_matches('/');

    // `--format=%(objecttype)<TAB>%(path)` keeps only the type + the full
    // repo-root-relative path. `git ls-tree` without `-r` lists only the
    // immediate children of the addressed tree.
    let mut args: Vec<String> = vec![
        "ls-tree".to_string(),
        "--format=%(objecttype)%x09%(path)".to_string(),
        r#ref.to_string(),
    ];
    if !dir.is_empty() && dir != "." {
        // A trailing-slash pathspec descends into the directory so we get
        // its children (`git ls-tree <ref> -- <dir>/`), not the dir entry.
        args.push("--".to_string());
        args.push(format!("{dir}/"));
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let listing = cmd::run(&arg_refs, repo_dir).await?;

    let mut entries: Vec<(String, bool)> = Vec::new();
    for line in listing.stdout.lines() {
        if line.is_empty() {
            continue;
        }
        // `<objecttype>\t<path>`.
        let Some((objtype, full_path)) = line.split_once('\t') else {
            continue;
        };
        let is_dir = objtype == "tree";
        entries.push((full_path.to_string(), is_dir));
    }

    // Trees first, then leaves; each alphabetical by full path.
    entries.sort_by(|a, b| match (a.1, b.1) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.0.cmp(&b.0),
    });
    Ok(entries)
}

/// Materialize the blobs reachable in `cone_paths` at `commit` for a
/// blobless clone at `repo_dir` (Task 304, `design/02 §3.3`/`§5.1`).
///
/// A blobless clone (`--filter=blob:none`, Task 301) leaves blob objects
/// *lazy* — present as promised-but-absent entries that git fetches
/// on-demand the first time something reads them. This helper does that
/// fetch ahead of agent need, scoped to the in-cone tree:
///
/// 1. `git ls-tree -r <commit> -- <cone_paths>` enumerates the blob OIDs
///    reachable under the cone (empty `cone_paths` = the whole tree).
/// 2. The OIDs are fed in bounded chunks to `git cat-file --batch-check`,
///    which forces git's partial-clone machinery to fetch any missing
///    blob from `origin`. `--batch-check` reads each object's header
///    (which requires the object to be present), so a missing blob is
///    transparently materialized without writing its content to stdout.
///
/// **Cancellable.** `should_cancel` is polled before every chunk; when it
/// returns `true` the materialization stops promptly (between chunks, so
/// at most one in-flight `cat-file` batch outlives the signal) and the
/// call returns `Ok(blobs_fetched_so_far)` with the terminal `done`
/// progress event already emitted. This is the `§6.3` "cancellable if
/// user activity resumes" contract — the scheduler wires `should_cancel`
/// to its `CancellationToken`.
///
/// `progress`, when supplied, receives a [`PrewarmProgressEvent`] after
/// every chunk plus a terminal `done` event. Sends are best-effort
/// (`try_send`) so a slow consumer never blocks the fetch.
///
/// Errors (a corrupt repo, an unreachable `origin`) surface as
/// [`Error::Git`]; the scheduler treats a single repo's failure as
/// non-fatal and moves on to the next.
pub async fn prewarm_blobs_in_cone<C>(
    repo_dir: &Path,
    commit: &str,
    cone_paths: &[String],
    should_cancel: C,
    progress: Option<mpsc::Sender<PrewarmProgressEvent>>,
) -> Result<u64>
where
    C: Fn() -> bool + Send + Sync,
{
    // How many OIDs to hand a single `cat-file --batch-check` invocation.
    // Bounds per-chunk memory + keeps the cancellation check frequent.
    const CHUNK: usize = 256;

    // 1. Enumerate in-cone blob OIDs at `commit`. `ls-tree -r` recurses;
    // restricting with `-- <paths>` scopes to the cone (no paths = whole
    // tree). `--format` keeps only the object name + type so we can filter
    // to blobs without parsing the columnar default output.
    let mut args: Vec<&str> = vec![
        "ls-tree",
        "-r",
        "--format=%(objecttype) %(objectname)",
        commit,
    ];
    if !cone_paths.is_empty() {
        args.push("--");
        for p in cone_paths {
            args.push(p.as_str());
        }
    }
    let listing = cmd::run(&args, repo_dir).await?;
    let oids: Vec<String> = listing
        .stdout
        .lines()
        .filter_map(|l| l.strip_prefix("blob "))
        .map(|oid| oid.trim().to_string())
        .collect();

    let blobs_total = oids.len() as u64;
    let mut blobs_fetched: u64 = 0;

    let emit = |fetched: u64, done: bool| {
        if let Some(sink) = &progress {
            let _ = sink.try_send(PrewarmProgressEvent {
                blobs_fetched: fetched,
                blobs_total,
                done,
            });
        }
    };

    // Nothing to do (e.g. an empty cone, or a tree with no blobs) — emit
    // the terminal event and return.
    if oids.is_empty() {
        emit(0, true);
        return Ok(0);
    }

    // 2. Force-materialize in bounded chunks, checking cancellation first.
    for chunk in oids.chunks(CHUNK) {
        if should_cancel() {
            // Prompt cancellation: stop between chunks and emit terminal.
            emit(blobs_fetched, true);
            return Ok(blobs_fetched);
        }
        // `cat-file --batch-check` reads each object's header, which in a
        // partial clone fetches a missing blob from origin. Feeding the
        // OIDs on stdin avoids an argv-length blow-up on large cones.
        let stdin = chunk
            .iter()
            .map(|o| o.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        cmd::run_with_stdin(&["cat-file", "--batch-check"], repo_dir, &stdin).await?;
        blobs_fetched += chunk.len() as u64;
        emit(blobs_fetched, false);
    }

    emit(blobs_fetched, true);
    Ok(blobs_fetched)
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

/// Rename branch `old` to `new` in the repository at `repo_dir` (Task 312,
/// `design/03 §3.6`).
///
/// Wraps `git branch -m <old> <new>`. Branch ops are git-authoritative
/// (`design/02 §3.1`), so this is a shell-out (mirroring [`worktree_add`])
/// rather than a `gix` ref edit. `git branch -m` updates the branch ref, moves
/// any reflog, and re-points HEAD + the checked-out worktree at the new name in
/// one atomic operation — the exact "rename the branch the worktree is on"
/// semantics the branch-rename hook needs, identical on the win/linux CI lanes
/// (Task 113).
///
/// A failure (e.g. `new` already exists locally, or `old` does not) surfaces as
/// [`Error::Git`]; the caller (the cross-repo rename loop) treats a single
/// repo's failure as non-fatal and continues with the others.
pub async fn rename_branch(repo_dir: &Path, old: &str, new: &str) -> Result<()> {
    cmd::run(&["branch", "-m", old, new], repo_dir)
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
