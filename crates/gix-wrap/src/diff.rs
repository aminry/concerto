//! `git diff` hot-path implementation (Task 29).
//!
//! V0.1 shells out to `git diff` and parses the unified-diff output into
//! a structured [`DiffPayload`]. The locked Rust signatures
//! ([`diff_head`], [`diff_to_main`]) stay valid if a future task
//! re-implements the body on top of `gix::diff` — the surface is the
//! contract.

use std::path::{Path, PathBuf};

use concerto_error::Result;

use crate::cmd;

/// One file's worth of diff information.
///
/// Mirrors the proto `concerto.v1.FileDiff` message shape so the handler
/// conversion is mechanical.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// Path relative to the worktree root.
    pub path: PathBuf,
    /// What kind of change. See [`DiffKind`].
    pub kind: DiffKind,
    /// Original path on rename (`R` lines in `git diff --name-status`).
    pub old_path: Option<PathBuf>,
    /// Per-file hunks. Empty for renames-without-edits, mode-only changes,
    /// and binary files.
    pub hunks: Vec<DiffHunk>,
}

/// Coarse-grained classification of a file diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffKind {
    Added,
    Deleted,
    Modified,
    Renamed,
}

/// One unified-diff hunk.
///
/// Field naming mirrors the proto `concerto.v1.DiffHunk` shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub old_start: i32,
    pub old_lines: i32,
    pub new_start: i32,
    pub new_lines: i32,
    /// Unified-diff body — the `+`/`-`/space lines between this hunk's
    /// header and the next one.
    pub body: String,
}

/// Aggregate diff payload — one entry per changed file.
///
/// Mirrors `concerto.v1.DiffPayload`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffPayload {
    pub files: Vec<FileDiff>,
}

/// Diff the worktree at `worktree_path` against its `HEAD`.
///
/// Shell-out path: runs `git diff HEAD --name-status -z` to discover the
/// kind of each change + original path on rename, then `git diff HEAD --
/// <path>` per file to capture the hunks. The two-step approach keeps
/// rename detection (which lives in `--name-status -M`) separate from
/// the unified-diff body, which is what consumers actually render.
pub async fn diff_head(worktree_path: &Path) -> Result<DiffPayload> {
    diff_against(worktree_path, "HEAD").await
}

/// Diff the worktree at `worktree_path` against `branch`.
///
/// Shell-out form: `git diff <branch>`. The `branch` argument is passed
/// through verbatim — callers building this from user input should
/// validate it first.
pub async fn diff_to_main(worktree_path: &Path, branch: &str) -> Result<DiffPayload> {
    diff_against(worktree_path, branch).await
}

async fn diff_against(worktree_path: &Path, rev: &str) -> Result<DiffPayload> {
    // Phase 1: per-file classification.
    let name_status = cmd::run(&["diff", "--name-status", "-M", "-z", rev], worktree_path).await?;
    let entries = parse_name_status(&name_status.stdout);

    // Phase 2: per-file unified diff.
    let mut files = Vec::with_capacity(entries.len());
    for (kind, path, old_path) in entries {
        // Skip the diff body fetch for pure renames-without-edits — the
        // proto representation carries the kind + paths, no hunks needed.
        let hunks = if matches!(kind, DiffKind::Renamed) && old_path.as_ref().is_some() {
            // Still try the body — rename-with-edits is the common case
            // and produces useful hunks. Empty output is harmless.
            let path_str = path.to_string_lossy().into_owned();
            let body = cmd::run(&["diff", "-U3", rev, "--", &path_str], worktree_path)
                .await
                .ok()
                .map(|o| o.stdout)
                .unwrap_or_default();
            parse_hunks(&body)
        } else {
            let path_str = path.to_string_lossy().into_owned();
            match cmd::run(&["diff", "-U3", rev, "--", &path_str], worktree_path).await {
                Ok(o) => parse_hunks(&o.stdout),
                Err(_) => Vec::new(),
            }
        };
        files.push(FileDiff {
            path,
            kind,
            old_path,
            hunks,
        });
    }
    Ok(DiffPayload { files })
}

/// Parse `git diff --name-status -M -z` output into a list of
/// (kind, new_path, optional old_path) tuples.
///
/// Records are NUL-delimited; rename records consume two extra chunks
/// (the original then the new path), so we use a peekable iterator.
fn parse_name_status(stdout: &str) -> Vec<(DiffKind, PathBuf, Option<PathBuf>)> {
    let mut out = Vec::new();
    let mut chunks = stdout.split('\0').filter(|c| !c.is_empty());
    while let Some(code) = chunks.next() {
        // The first byte is the code letter. Rename codes are `R100`,
        // `R090` etc.; the leading char is what we switch on.
        let first = code.chars().next().unwrap_or(' ');
        match first {
            'A' => {
                if let Some(p) = chunks.next() {
                    out.push((DiffKind::Added, PathBuf::from(p), None));
                }
            }
            'D' => {
                if let Some(p) = chunks.next() {
                    out.push((DiffKind::Deleted, PathBuf::from(p), None));
                }
            }
            'M' | 'T' => {
                if let Some(p) = chunks.next() {
                    out.push((DiffKind::Modified, PathBuf::from(p), None));
                }
            }
            'R' | 'C' => {
                // Rename / copy: code chunk, then orig, then new path.
                let from = chunks.next().map(PathBuf::from);
                let to = chunks.next().map(PathBuf::from);
                if let Some(to) = to {
                    out.push((DiffKind::Renamed, to, from));
                }
            }
            // Unknown letter: skip the path chunk if present so we
            // stay aligned.
            _ => {
                let _ = chunks.next();
            }
        }
    }
    out
}

/// Parse the unified-diff body emitted by `git diff -U3 <rev> -- <path>`
/// into a list of [`DiffHunk`]s.
///
/// Hunk headers look like `@@ -OLDSTART,OLDLINES +NEWSTART,NEWLINES @@`.
/// Lines until the next header (or EOF) make up the body. Headers without
/// a comma carry an implicit line count of 1.
pub(crate) fn parse_hunks(body: &str) -> Vec<DiffHunk> {
    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut current: Option<DiffHunk> = None;
    for line in body.lines() {
        if let Some((old_start, old_lines, new_start, new_lines)) = parse_hunk_header(line) {
            if let Some(h) = current.take() {
                hunks.push(h);
            }
            current = Some(DiffHunk {
                old_start,
                old_lines,
                new_start,
                new_lines,
                body: String::new(),
            });
        } else if let Some(h) = current.as_mut() {
            // Append the body line verbatim (with the trailing newline
            // the input may or may not carry — `lines()` drops it).
            if !h.body.is_empty() {
                h.body.push('\n');
            }
            h.body.push_str(line);
        }
        // Lines before the first hunk header (file headers, etc.) are
        // dropped — they aren't useful for the consumer.
    }
    if let Some(h) = current {
        hunks.push(h);
    }
    hunks
}

/// Parse a unified-diff hunk header.
///
/// Returns `(old_start, old_lines, new_start, new_lines)` or `None` for
/// anything that isn't a hunk header.
fn parse_hunk_header(line: &str) -> Option<(i32, i32, i32, i32)> {
    let rest = line.strip_prefix("@@ ")?;
    // `-old,len +new,len @@ ...`
    let (old_part, rest) = rest.split_once(' ')?;
    let (new_part, _trailer) = rest.split_once(' ')?;
    let old = old_part.strip_prefix('-')?;
    let new = new_part.strip_prefix('+')?;
    let (old_start, old_lines) = parse_range(old)?;
    let (new_start, new_lines) = parse_range(new)?;
    Some((old_start, old_lines, new_start, new_lines))
}

fn parse_range(s: &str) -> Option<(i32, i32)> {
    if let Some((start, lines)) = s.split_once(',') {
        Some((start.parse().ok()?, lines.parse().ok()?))
    } else {
        // Implicit count of 1.
        Some((s.parse().ok()?, 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_status_records() {
        // A\0added.txt\0M\0changed.txt\0D\0gone.txt\0
        let stdout = "A\0added.txt\0M\0changed.txt\0D\0gone.txt\0";
        let entries = parse_name_status(stdout);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].0, DiffKind::Added);
        assert_eq!(entries[0].1, PathBuf::from("added.txt"));
        assert_eq!(entries[1].0, DiffKind::Modified);
        assert_eq!(entries[2].0, DiffKind::Deleted);
    }

    #[test]
    fn parses_rename_record() {
        let stdout = "R100\0old.txt\0new.txt\0";
        let entries = parse_name_status(stdout);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, DiffKind::Renamed);
        assert_eq!(entries[0].1, PathBuf::from("new.txt"));
        assert_eq!(entries[0].2.as_deref(), Some(Path::new("old.txt")));
    }

    #[test]
    fn parses_hunk_header() {
        let h = parse_hunk_header("@@ -1,3 +1,4 @@ context").expect("parsed");
        assert_eq!(h, (1, 3, 1, 4));
    }

    #[test]
    fn parses_hunk_header_implicit_count() {
        let h = parse_hunk_header("@@ -10 +20 @@").expect("parsed");
        assert_eq!(h, (10, 1, 20, 1));
    }

    #[test]
    fn parses_unified_diff_body() {
        let body = "diff --git a/x b/x\n\
index abc..def 100644\n\
--- a/x\n\
+++ b/x\n\
@@ -1,2 +1,3 @@\n\
 line one\n\
+inserted\n\
 line two\n";
        let hunks = parse_hunks(body);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].old_lines, 2);
        assert_eq!(hunks[0].new_start, 1);
        assert_eq!(hunks[0].new_lines, 3);
        assert!(hunks[0].body.contains("+inserted"));
    }
}
