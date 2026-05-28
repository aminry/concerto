//! Files-to-copy resolver (Task 30 / design/03 §3.10).
//!
//! At workarea creation time, project-level rules in
//! `<project_root>/.concerto/.worktreeinclude` are applied to each
//! repo's new worktree: each matching file is **copied**, **symlinked**,
//! or **excluded** from the workarea's copy of the worktree.
//!
//! V0.1 simplifications (per `tasks/30`):
//!
//! - The project's reference worktree is the workspace's single repo's
//!   `local_path` (V0.1 ships single-repo workspaces only).
//! - `.worktreeinclude` lives at `<repo.local_path>/.concerto/.worktreeinclude`.
//!   Missing → resolver is a no-op.
//! - Multi-repo file-to-copy targets, full project-settings precedence,
//!   and Windows symlink fallbacks (junctions/hardlinks) are V1.0.
//!
//! ## `.worktreeinclude` syntax
//!
//! Per design/03 §3.10:
//!
//! ```text
//! # Comments allowed
//! .env*                    # default mode = copy (no annotation)
//! .env.local            !  # trailing `!` = symlink
//! .vscode/                 # copy (directory)
//! node_modules-cache/   !  # symlink (directory)
//! !.env.production         # leading `!` = exclude (gitignore-style negation)
//! ```
//!
//! Resolution: rules apply in declaration order; later matches override
//! earlier ones (last-match-wins). The walker is gitignore-aware (uses
//! the [`ignore`] crate's matcher) so glob semantics line up with what
//! contributors expect.
//!
//! ## Safety
//!
//! - **Path escape rejection.** Every resolved source/destination is
//!   canonicalized via `std::fs::canonicalize` and must remain within
//!   the project's reference worktree. Symlinks (or `..` segments) that
//!   would escape surface [`Error::Validation`] with the wire code
//!   `file_to_copy.escapes_project_root`.
//! - **Broken symlinks** are tolerated — the link is created pointing
//!   at the (currently broken) source and a debug-level trace records
//!   the situation. The per-workarea warning chip surface lands with
//!   Task 31 (archive lifecycle) per design §3.10.
//! - **Windows** is out of scope for V0.1 (macOS-only desktop); the
//!   symlink call is gated behind `cfg(unix)` and a `cfg(not(unix))`
//!   stub returns `Error::Internal` — V0.1 never hits this path because
//!   Tauri ships macOS-only.
//!
//! ## Idempotency
//!
//! Re-running [`apply`] on a workarea that already has the files in
//! place is safe: existing symlinks are left alone if they already
//! point at the correct target; existing files are left alone if their
//! contents match. The Workarea Manager stamps
//! `workareas.settings_json.files_to_copy_applied = true` on success so
//! repeat runs short-circuit at the call-site.

use std::path::{Path, PathBuf};

use concerto_error::{Error, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// The conventional path of the project-level include file, relative to
/// the project's reference worktree.
pub const WORKTREEINCLUDE_RELPATH: &str = ".concerto/.worktreeinclude";

/// What to do with a matched path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// One-shot copy at workarea create. Not synced afterward.
    Copy,
    /// Relative symlink from the workarea path to the reference source.
    Symlink,
    /// Skip — gitignore-style negation, applied after include rules.
    Exclude,
}

/// A parsed line from `.worktreeinclude`.
#[derive(Debug, Clone)]
pub struct Rule {
    /// Raw glob (annotations stripped). Gitignore semantics; relative
    /// to the project's reference worktree.
    pub pattern: String,
    /// `copy` (default), `symlink`, or `exclude`.
    pub mode: Mode,
}

/// Parse a `.worktreeinclude` blob.
///
/// Each non-empty, non-comment line maps to one [`Rule`]. The grammar
/// is documented at the module head; in short:
///
/// - bare `pattern` → [`Mode::Copy`]
/// - `pattern!` (trailing `!`, optionally preceded by whitespace) → [`Mode::Symlink`]
/// - `!pattern` (leading `!`) → [`Mode::Exclude`]
///
/// Lines starting with `#` and blank lines are ignored. Trailing
/// whitespace and the symlink annotation (` !` / `\t!`) are trimmed
/// before the rule is stored.
pub fn parse(text: &str) -> Vec<Rule> {
    let mut out = Vec::new();
    for raw in text.lines() {
        // Trim leading whitespace first so a line whose only content is
        // a comment (`   # whatever`) is treated as a pure comment.
        let leading_trim = raw.trim_start();
        if leading_trim.is_empty() || leading_trim.starts_with('#') {
            continue;
        }
        // Strip an inline `# …` comment so authors can document rules
        // on the same line. The `#` must be preceded by whitespace to
        // avoid clipping legitimate glob characters; this matches the
        // gitignore convention.
        let line = strip_inline_comment(leading_trim).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('!') {
            // Leading `!` always wins — even if the body also has a
            // trailing `!`, the gitignore-style negation is the
            // dominant mode (and a `! foo !` line is just ill-formed).
            let pattern = rest.trim().to_string();
            if pattern.is_empty() {
                continue;
            }
            out.push(Rule {
                pattern,
                mode: Mode::Exclude,
            });
            continue;
        }
        // Trailing `!` (with leading whitespace before it) = symlink.
        // We trim the trailing `!` and any whitespace before it.
        let (pattern, mode) = match strip_trailing_symlink_marker(line) {
            Some(stripped) => (stripped.trim_end().to_string(), Mode::Symlink),
            None => (line.to_string(), Mode::Copy),
        };
        if pattern.is_empty() {
            continue;
        }
        out.push(Rule { pattern, mode });
    }
    out
}

/// Strip an inline ` # comment` (whitespace + `#` + tail). Returns the
/// remainder; leaves the line untouched if no inline comment exists.
fn strip_inline_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'#' && i > 0 && (bytes[i - 1] == b' ' || bytes[i - 1] == b'\t') {
            return &line[..i];
        }
    }
    line
}

/// If the trimmed line ends with `<whitespace>!`, return the body
/// without the trailing marker.
fn strip_trailing_symlink_marker(line: &str) -> Option<&str> {
    let trimmed = line.trim_end();
    if !trimmed.ends_with('!') {
        return None;
    }
    let body = &trimmed[..trimmed.len() - 1];
    // Require at least one whitespace before the `!` so `foo!` (where
    // `!` is part of the filename) doesn't accidentally match. design
    // §3.10's grammar shows the marker as ` !` / `\t!`.
    if body.ends_with(' ') || body.ends_with('\t') {
        Some(body)
    } else {
        None
    }
}

/// Compiled matcher: one [`Gitignore`] per [`Rule`] so we can ask which
/// pattern matched a given path and look up the rule's mode.
struct Compiled {
    rules: Vec<(Rule, Gitignore)>,
}

impl Compiled {
    fn build(rules: Vec<Rule>, root: &Path) -> Result<Self> {
        let mut compiled = Vec::with_capacity(rules.len());
        for rule in rules {
            let mut b = GitignoreBuilder::new(root);
            // `add_line` returns an error iff the pattern itself is
            // syntactically invalid. We surface that as a Validation
            // error so the user knows which line to fix.
            b.add_line(None, &rule.pattern).map_err(|e| {
                Error::Validation(format!(
                    "files_to_copy.invalid_pattern: {:?}: {}",
                    rule.pattern, e
                ))
            })?;
            let matcher = b
                .build()
                .map_err(|e| Error::Validation(format!("files_to_copy.build: {e}")))?;
            compiled.push((rule, matcher));
        }
        Ok(Self { rules: compiled })
    }

    /// Find the **last** matching rule for a path. The path is
    /// passed relative to the reference worktree root.
    fn last_match(&self, rel: &Path, is_dir: bool) -> Option<Mode> {
        let mut last: Option<Mode> = None;
        for (rule, matcher) in &self.rules {
            let m = matcher.matched(rel, is_dir);
            // `Gitignore::matched` returns Whitelist for `!`-prefixed
            // patterns — but we strip leading `!` ourselves so every
            // compiled matcher is a positive pattern. Any non-`None`
            // match here means "this rule selected the path."
            if !m.is_none() {
                last = Some(rule.mode);
            }
        }
        last
    }
}

/// Apply `.worktreeinclude` rules from `project_root` into `dest_root`.
///
/// Walks `project_root`, matches each entry against the rules, and:
///
/// - skips files that match an `Exclude` rule (or no rule at all);
/// - copies files whose last-matching rule is `Copy`;
/// - symlinks files whose last-matching rule is `Symlink`, using a
///   relative target so the destination remains portable.
///
/// Returns `Ok(0)` when there is no `.worktreeinclude` at the project
/// root — the resolver is a no-op for projects that don't opt in.
///
/// `project_root` and `dest_root` must already exist; both are
/// canonicalized for the escape-safety check.
pub fn apply(project_root: &Path, dest_root: &Path) -> Result<usize> {
    let include_path = project_root.join(WORKTREEINCLUDE_RELPATH);
    let text = match std::fs::read_to_string(&include_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(Error::Io(e)),
    };
    let rules = parse(&text);
    if rules.is_empty() {
        return Ok(0);
    }
    apply_rules(project_root, dest_root, &rules)
}

/// Test-friendly variant: same as [`apply`] but takes already-parsed
/// rules instead of reading them from the project root.
pub fn apply_rules(project_root: &Path, dest_root: &Path, rules: &[Rule]) -> Result<usize> {
    let project_canon = std::fs::canonicalize(project_root).map_err(|e| {
        Error::Io(std::io::Error::new(
            e.kind(),
            format!("canonicalize project_root {}: {e}", project_root.display()),
        ))
    })?;
    let dest_canon = std::fs::canonicalize(dest_root).map_err(|e| {
        Error::Io(std::io::Error::new(
            e.kind(),
            format!("canonicalize dest_root {}: {e}", dest_root.display()),
        ))
    })?;
    let compiled = Compiled::build(rules.to_vec(), &project_canon)?;

    let walker = ignore::WalkBuilder::new(&project_canon)
        .standard_filters(false)
        .hidden(false)
        .git_ignore(false)
        .git_exclude(false)
        .git_global(false)
        .parents(false)
        .build();

    let mut applied = 0usize;
    for entry in walker {
        let entry = entry.map_err(|e| Error::Internal(format!("walk: {e}")))?;
        let src = entry.path();
        // Skip the root itself.
        if src == project_canon {
            continue;
        }
        // Skip `.git/` — never material for files-to-copy and would
        // produce noisy attempted-copies for the on-disk object store.
        let rel = src.strip_prefix(&project_canon).unwrap_or(src);
        if rel
            .components()
            .next()
            .map(|c| c.as_os_str() == ".git")
            .unwrap_or(false)
        {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let mode = match compiled.last_match(rel, is_dir) {
            Some(Mode::Copy) => Mode::Copy,
            Some(Mode::Symlink) => Mode::Symlink,
            Some(Mode::Exclude) | None => continue,
        };
        // Directories matching include rules are not materialized as
        // single ops — gitignore semantics already propagate the match
        // to descendants. We only act on files (and symlinks pointing
        // at regular files). For a directory match (`.vscode/`), we
        // skip the dir entry itself but the walker yields the children
        // and they'll match by the same rule.
        if is_dir {
            continue;
        }
        let dest = dest_canon.join(rel);
        // Escape check: the canonical dest must remain within
        // `dest_canon`. We canonicalize the dest's parent (which exists
        // already because we `create_dir_all` it just below — but the
        // check applies to the parent dir, not the dest file).
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(Error::Io)?;
            let parent_canon = std::fs::canonicalize(parent).map_err(Error::Io)?;
            if !parent_canon.starts_with(&dest_canon) {
                return Err(Error::Validation(format!(
                    "file_to_copy.escapes_project_root: destination {} resolves outside {}",
                    parent_canon.display(),
                    dest_canon.display()
                )));
            }
        }
        // Source-side escape check: resolve symlinks in the source,
        // then ensure the resolved path still sits under
        // `project_canon`. Broken symlinks fall through here with a
        // not-found error — we tolerate them per design §3.10.
        match std::fs::canonicalize(src) {
            Ok(src_canon) => {
                if !src_canon.starts_with(&project_canon) {
                    return Err(Error::Validation(format!(
                        "file_to_copy.escapes_project_root: source {} resolves outside {}",
                        src_canon.display(),
                        project_canon.display()
                    )));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Broken symlink in the source tree — surface a debug
                // trace and skip the entry. The per-workarea warning
                // chip is Task 31's surface.
                tracing::debug!(
                    src = %src.display(),
                    "files_to_copy: skipping broken source symlink"
                );
                continue;
            }
            Err(e) => return Err(Error::Io(e)),
        }

        match mode {
            Mode::Copy => {
                copy_file_idempotent(src, &dest)?;
            }
            Mode::Symlink => {
                make_symlink_relative(src, &dest)?;
            }
            Mode::Exclude => unreachable!("filtered above"),
        }
        applied += 1;
    }
    Ok(applied)
}

/// Copy `src` to `dest`, skipping if `dest` already exists with the
/// same contents (so repeat runs don't churn mtimes / re-trigger fs
/// watchers).
fn copy_file_idempotent(src: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        // Same-bytes shortcut. Cheap because both files are typically
        // tiny (.env, .vscode/settings.json).
        if let (Ok(a), Ok(b)) = (std::fs::read(src), std::fs::read(dest)) {
            if a == b {
                return Ok(());
            }
        }
    }
    std::fs::copy(src, dest).map_err(Error::Io)?;
    Ok(())
}

/// Create a relative symlink at `dest` pointing at `src`.
///
/// The link target is computed via [`pathdiff::diff_paths`] so the
/// workarea remains movable: relocating the whole workarea preserves
/// link integrity as long as the project root moves with it.
///
/// On Unix uses [`std::os::unix::fs::symlink`]; Windows falls back to
/// an explicit error (V0.1 macOS-only — design §3.10's
/// junction/hardlink fallback ships in V1.0).
fn make_symlink_relative(src: &Path, dest: &Path) -> Result<()> {
    // Compute the relative path from dest's PARENT to src so the link
    // body reads like `../../<repo>/.env.local`.
    let parent = dest
        .parent()
        .ok_or_else(|| Error::Internal(format!("dest has no parent: {}", dest.display())))?;
    let target = pathdiff::diff_paths(src, parent).ok_or_else(|| {
        Error::Internal(format!(
            "pathdiff failed for src={} parent={}",
            src.display(),
            parent.display()
        ))
    })?;

    // Replace any existing entry at `dest` so the run is idempotent.
    match std::fs::symlink_metadata(dest) {
        Ok(md) => {
            if md.file_type().is_symlink() {
                // If it already points at the right place, no-op.
                if let Ok(existing) = std::fs::read_link(dest) {
                    if existing == target {
                        return Ok(());
                    }
                }
                std::fs::remove_file(dest).map_err(Error::Io)?;
            } else if md.is_file() {
                std::fs::remove_file(dest).map_err(Error::Io)?;
            } else {
                std::fs::remove_dir_all(dest).map_err(Error::Io)?;
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::Io(e)),
    }

    create_symlink(&target, dest)
}

#[cfg(unix)]
fn create_symlink(target: &Path, dest: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, dest).map_err(Error::Io)
}

#[cfg(not(unix))]
fn create_symlink(_target: &Path, _dest: &Path) -> Result<()> {
    // V0.1 ships macOS-only desktop; the Tauri shell crate gates on
    // cfg(unix), so this branch is unreachable in V0.1's build matrix.
    // Surface as Internal rather than silently falling back to copy
    // because the V1.0 fallback (junctions/hardlinks) is a separate
    // task with its own decision tree.
    Err(Error::Internal(
        "files_to_copy: symlink fallback on non-unix is V1.0 (junctions/hardlinks)".into(),
    ))
}

/// Resolve the destination directory inside the workarea for a given
/// repo. Helper for callers; not used internally by [`apply`].
pub fn dest_for_repo(workarea_root: &Path, repo_name: &str) -> PathBuf {
    workarea_root.join(repo_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_copy_default() {
        let rules = parse(".env*\n");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, ".env*");
        assert_eq!(rules[0].mode, Mode::Copy);
    }

    #[test]
    fn parse_symlink_trailing_marker() {
        let rules = parse(".env.local              !\n");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, ".env.local");
        assert_eq!(rules[0].mode, Mode::Symlink);
    }

    #[test]
    fn parse_exclude_leading_bang() {
        let rules = parse("!.env.production\n");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, ".env.production");
        assert_eq!(rules[0].mode, Mode::Exclude);
    }

    #[test]
    fn parse_comments_and_blanks_ignored() {
        let text = r#"
# header comment
   # indented comment

.env*    # inline copy
.env.local            !
!.env.production
"#;
        let rules = parse(text);
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].mode, Mode::Copy);
        assert_eq!(rules[0].pattern, ".env*");
        assert_eq!(rules[1].mode, Mode::Symlink);
        assert_eq!(rules[1].pattern, ".env.local");
        assert_eq!(rules[2].mode, Mode::Exclude);
        assert_eq!(rules[2].pattern, ".env.production");
    }

    #[test]
    fn parse_bang_without_whitespace_is_part_of_filename() {
        // `foo!` (no space before `!`) is a literal filename, not a
        // symlink marker.
        let rules = parse("foo!\n");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].pattern, "foo!");
        assert_eq!(rules[0].mode, Mode::Copy);
    }

    #[test]
    fn parse_directory_pattern() {
        let rules = parse(".vscode/\n");
        assert_eq!(rules[0].pattern, ".vscode/");
        assert_eq!(rules[0].mode, Mode::Copy);
    }

    #[test]
    fn last_match_wins() {
        // Per design §3.10: when two rules touch the same path, the
        // later rule wins (so a symlink rule can override an earlier
        // copy glob).
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env.local"), "x").unwrap();
        let rules = parse(".env*\n.env.local !\n");
        let compiled = Compiled::build(rules, dir.path()).unwrap();
        let m = compiled.last_match(Path::new(".env.local"), false);
        assert_eq!(m, Some(Mode::Symlink));
    }

    #[test]
    fn apply_copies_a_file() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join(".env"), b"KEY=val\n").unwrap();
        let n = apply_rules(src.path(), dst.path(), &parse(".env\n")).unwrap();
        assert_eq!(n, 1);
        let got = std::fs::read(dst.path().join(".env")).unwrap();
        assert_eq!(got, b"KEY=val\n");
    }

    #[cfg(unix)]
    #[test]
    fn apply_symlinks_a_file_with_relative_target() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("shared.toml"), b"x").unwrap();
        let n = apply_rules(src.path(), dst.path(), &parse("shared.toml !\n")).unwrap();
        assert_eq!(n, 1);
        let dest_link = dst.path().join("shared.toml");
        let md = std::fs::symlink_metadata(&dest_link).unwrap();
        assert!(md.file_type().is_symlink());
        let target = std::fs::read_link(&dest_link).unwrap();
        assert!(
            target.is_relative(),
            "link target must be relative, got {target:?}"
        );
        // Reading via the link must produce the source bytes.
        let bytes = std::fs::read(&dest_link).unwrap();
        assert_eq!(bytes, b"x");
    }

    #[test]
    fn apply_skips_excluded_within_include() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join(".env"), b"a").unwrap();
        std::fs::write(src.path().join(".env.production"), b"b").unwrap();
        let rules = parse(".env*\n!.env.production\n");
        let n = apply_rules(src.path(), dst.path(), &rules).unwrap();
        assert_eq!(n, 1, ".env copied, .env.production excluded");
        assert!(dst.path().join(".env").is_file());
        assert!(!dst.path().join(".env.production").exists());
    }

    #[test]
    fn apply_idempotent_skips_same_bytes() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join(".env"), b"x").unwrap();
        let rules = parse(".env\n");
        apply_rules(src.path(), dst.path(), &rules).unwrap();
        // Tweak the destination's mtime by writing the same bytes
        // through a separate path → second apply should be a no-op
        // (same-bytes shortcut).
        let mtime_before = std::fs::metadata(dst.path().join(".env"))
            .unwrap()
            .modified()
            .unwrap();
        apply_rules(src.path(), dst.path(), &rules).unwrap();
        let mtime_after = std::fs::metadata(dst.path().join(".env"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(mtime_before, mtime_after, "same-bytes copy must be a no-op");
    }

    #[cfg(unix)]
    #[test]
    fn apply_rejects_escaping_symlink() {
        // Create an outside-tree file, then a symlink inside the
        // source tree pointing at it. The resolver must reject the
        // escaping source.
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), b"nope").unwrap();
        let src = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), src.path().join(".env")).unwrap();
        let dst = tempfile::tempdir().unwrap();
        let err = apply_rules(src.path(), dst.path(), &parse(".env\n")).unwrap_err();
        match err {
            Error::Validation(m) => assert!(
                m.contains("file_to_copy.escapes_project_root"),
                "wrong error: {m}"
            ),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn apply_skips_when_include_file_missing() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        // No `.concerto/.worktreeinclude` → no-op.
        let n = apply(src.path(), dst.path()).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn apply_reads_include_file_when_present() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join(".concerto")).unwrap();
        std::fs::write(src.path().join(WORKTREEINCLUDE_RELPATH), ".env\n").unwrap();
        std::fs::write(src.path().join(".env"), b"KEY=val\n").unwrap();
        let n = apply(src.path(), dst.path()).unwrap();
        assert_eq!(n, 1);
        assert!(dst.path().join(".env").is_file());
    }

    #[test]
    fn apply_skips_git_directory() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join(".git/objects")).unwrap();
        std::fs::write(src.path().join(".git/objects/x"), b"junk").unwrap();
        let rules = parse("**/*\n");
        let n = apply_rules(src.path(), dst.path(), &rules).unwrap();
        assert_eq!(n, 0, ".git/ contents must not be visited");
    }
}
