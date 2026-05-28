//! `git status` hot-path implementation (Task 29).
//!
//! Per Task 29's pre-decisions, V0.1 shells out to `git status --porcelain=v1`
//! and parses the output. `gix::status` has an evolving API surface that
//! does not yet match `git status` 1:1 for every edge case; the shell-out
//! path is more stable and still well inside the 100ms target for the
//! 10k-file fixture this task gates on.
//!
//! The locked `pub fn status(...) -> Result<StatusReport>` surface is
//! preserved regardless of implementation — switching to a pure-`gix`
//! backend in a follow-on does not change the public type.

use std::path::{Path, PathBuf};

use concerto_error::Result;

use crate::cmd;

/// One file's status as reported by `git status`.
///
/// V0.1 collapses the staged/unstaged distinction onto a single
/// [`StatusState`]: the workspace-relative status is what the UI needs.
/// A follow-on can add an `index_state` field for staged/unstaged splits
/// without breaking existing call sites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    /// Path relative to the worktree root, as `git status` reports it.
    pub path: PathBuf,
    /// Coarse-grained state. See [`StatusState`].
    pub state: StatusState,
}

/// Coarse-grained workspace-relative status.
///
/// Maps the porcelain v1 letter codes onto an enum the UI can switch on.
/// Status combinations that don't fit (mode conflicts, etc.) round-trip as
/// [`StatusState::Modified`]; the goal is "what kind of change is this"
/// not "exact git porcelain bytes".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusState {
    /// File is new to the index (`A`) or untracked but newly added (`A`,
    /// `AM`, `AD`).
    Added,
    /// File is tracked and modified relative to HEAD or index (`M`, `MM`,
    /// `RM`, etc.).
    Modified,
    /// File is tracked and removed (`D`, `AD`, etc.).
    Deleted,
    /// File is not tracked at all (`??`).
    Untracked,
    /// Rename detected (`R`). `from` carries the original path.
    Renamed { from: PathBuf },
}

/// Aggregate status of a worktree — the surface every higher layer
/// observes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusReport {
    /// One entry per changed file. Empty for a clean worktree.
    pub files: Vec<StatusEntry>,
}

/// Run `git status --porcelain=v1` against the worktree at `worktree_path`
/// and parse the output into a [`StatusReport`].
///
/// Shell-out is the V0.1 implementation (per Task 29 pre-decision 1).
/// The signature is `async` because the underlying subprocess is driven by
/// `tokio::process::Command` — callers from the gRPC layer should still
/// wrap this in `spawn_blocking` if they want to keep tokio worker
/// threads free, but the function itself does not block the runtime.
pub async fn status(worktree_path: &Path) -> Result<StatusReport> {
    // Use porcelain=v1 + `-z` for unambiguous parsing: NUL-terminated
    // records sidestep filename quoting entirely.
    let out = cmd::run(&["status", "--porcelain=v1", "-z"], worktree_path).await?;
    Ok(parse_porcelain_v1(&out.stdout))
}

/// Parse `git status --porcelain=v1 -z` output.
///
/// Format: each record is `XY <path>\0` (plus an extra `<orig>\0` for
/// renames). `XY` is two status letters per the porcelain v1 spec.
pub(crate) fn parse_porcelain_v1(stdout: &str) -> StatusReport {
    let mut files = Vec::new();
    let mut chunks = stdout.split('\0').peekable();
    while let Some(rec) = chunks.next() {
        if rec.is_empty() {
            continue;
        }
        // Each non-rename record is `XY <path>` (a single space between
        // the status code and the path). Rename records (`R `) consume
        // an additional NUL-delimited chunk for the original path.
        if rec.len() < 3 {
            continue;
        }
        let bytes = rec.as_bytes();
        let x = bytes[0] as char;
        let y = bytes[1] as char;
        // Bytes 2..=2 is the separator (space); path follows.
        let path_str = &rec[3..];
        let path = PathBuf::from(path_str);

        let state = match (x, y) {
            // Untracked.
            ('?', '?') => StatusState::Untracked,
            // Rename: porcelain v1 emits the new path first, then a
            // separate NUL-delimited record for the original path.
            ('R', _) | (_, 'R') => {
                let from = chunks
                    .next()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(""));
                StatusState::Renamed { from }
            }
            // Deletion takes priority over the rest.
            ('D', _) | (_, 'D') => StatusState::Deleted,
            // Addition (A in either column).
            ('A', _) | (_, 'A') => StatusState::Added,
            // Anything else with a real letter counts as modified.
            _ => StatusState::Modified,
        };

        files.push(StatusEntry { path, state });
    }
    StatusReport { files }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modified_and_untracked() {
        // Two records: ` M README.md` (modified, unstaged) and
        // `?? new.txt` (untracked).
        let stdout = " M README.md\0?? new.txt\0";
        let report = parse_porcelain_v1(stdout);
        assert_eq!(report.files.len(), 2);
        assert_eq!(report.files[0].path, PathBuf::from("README.md"));
        assert_eq!(report.files[0].state, StatusState::Modified);
        assert_eq!(report.files[1].path, PathBuf::from("new.txt"));
        assert_eq!(report.files[1].state, StatusState::Untracked);
    }

    #[test]
    fn parses_added_and_deleted() {
        let stdout = "A  added.txt\0 D removed.txt\0";
        let report = parse_porcelain_v1(stdout);
        assert_eq!(report.files.len(), 2);
        assert_eq!(report.files[0].state, StatusState::Added);
        assert_eq!(report.files[1].state, StatusState::Deleted);
    }

    #[test]
    fn parses_rename_with_from_path() {
        // Rename: `R  new.txt\0old.txt\0`.
        let stdout = "R  new.txt\0old.txt\0";
        let report = parse_porcelain_v1(stdout);
        assert_eq!(report.files.len(), 1);
        assert_eq!(report.files[0].path, PathBuf::from("new.txt"));
        match &report.files[0].state {
            StatusState::Renamed { from } => assert_eq!(from, &PathBuf::from("old.txt")),
            other => panic!("expected rename, got {other:?}"),
        }
    }

    #[test]
    fn empty_input_is_clean() {
        let report = parse_porcelain_v1("");
        assert!(report.files.is_empty());
    }
}
