//! Files-to-copy resolver (Task 30 / design/03 §3.10; multi-repo + Windows
//! fallbacks in Task 309).
//!
//! At workarea creation time, the resolved rule set (a checked-in
//! `.worktreeinclude` at the **reference repo** root, or the workspace's
//! local-DB `files_to_copy_rules`) is applied into **each** repo's new
//! worktree: each matching file is **copied**, **symlinked**, or
//! **excluded** from the workarea's copy of the worktree.
//!
//! ## Reference-worktree rule (FROZEN, Task 309 / design/03 §3.10)
//!
//! - The files-to-copy **source root** is the workarea's **first repo by
//!   `workspace_repos.position`** — the *reference worktree* (`design/03
//!   §3.10` "default: first listed repo"). V1.0 adds no per-project
//!   "designated reference" field (`PHASE3_PLANNING §2` defers it); first
//!   by position is the only selector.
//! - Source patterns resolve **relative to that reference worktree**; the
//!   resolved rule set then applies into **every** repo's worktree at the
//!   matching relative path (a reference-worktree `.env` lands in all
//!   repos' worktrees). If a non-reference repo has its own native match
//!   for the same pattern, that repo's own file is handled per repo when
//!   the caller also runs [`apply_for_repo`] with that repo as the source
//!   — the call site (`workarea.rs`) drives one reference-rooted apply per
//!   destination worktree.
//! - `.worktreeinclude` lives at
//!   `<reference_repo.local_path>/.concerto/.worktreeinclude`. Missing →
//!   the caller falls back to the local-DB `files_to_copy_rules`
//!   (`design/03 §3.13`: a checked-in `.worktreeinclude` **wins** over the
//!   local-DB rules).
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
//! - **Broken symlinks** are tolerated — a broken source symlink is
//!   skipped (never blocks the workarea) and a
//!   [`ApplyWarning::BrokenSymlink`] is recorded so the caller can surface
//!   the "symlink to `<path>` is broken" chip (`design/03 §3.10`).
//! - **Windows / non-Unix fallback** (Task 309). The `symlink` mode no
//!   longer errors off-Unix. On Windows [`create_symlink`] tries a real
//!   symlink first (works under Developer Mode / `SeCreateSymbolicLink`),
//!   then falls back to a directory **junction** (dirs) / **hardlink**
//!   (files), and finally to a plain **copy** with a one-time
//!   [`ApplyWarning::SymlinkUnsupported`] warning when the filesystem
//!   supports none of those. Junctions are **absolute** on Windows (a
//!   known divergence from the Unix relative-symlink invariant — documented
//!   here; the workarea stays movable on Unix, and the Windows hardlink/copy
//!   fallbacks are path-independent). The escape-rejection + relative-target
//!   (Unix) invariants hold on every platform.
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

/// A non-blocking warning produced while applying rules into a worktree
/// (`design/03 §3.10`). Warnings never fail the workarea create — they ride
/// out as `WorkareaEvent` chips. Only a path **escape** is a hard error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyWarning {
    /// A `symlink`-mode source is a broken symlink (its target does not
    /// resolve). The entry is skipped. Surfaces "symlink to `<path>` is
    /// broken". `rel` is the matched path relative to the reference worktree.
    BrokenSymlink { rel: String },
    /// A `symlink`-mode rule fell back to a plain copy because the
    /// destination filesystem has no symlink/junction/hardlink support
    /// (Windows without privilege + cross-volume). Surfaces "symlinks
    /// unsupported here — copied `<path>` instead". `rel` is the matched path.
    SymlinkUnsupported { rel: String },
}

impl ApplyWarning {
    /// The matched path (relative to the reference worktree) this warning
    /// is about — used by the caller to build the chip message.
    pub fn rel(&self) -> &str {
        match self {
            ApplyWarning::BrokenSymlink { rel } | ApplyWarning::SymlinkUnsupported { rel } => rel,
        }
    }

    /// The human-readable chip message (`design/03 §3.10`).
    pub fn message(&self) -> String {
        match self {
            ApplyWarning::BrokenSymlink { rel } => format!("symlink to `{rel}` is broken"),
            ApplyWarning::SymlinkUnsupported { rel } => {
                format!("symlinks unsupported here — copied `{rel}` instead")
            }
        }
    }
}

/// The outcome of an [`apply_for_repo`] / [`apply`] run: how many entries were
/// materialized plus any non-blocking warnings (`design/03 §3.10`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyReport {
    /// Number of files copied / symlinked into the destination worktree.
    pub applied: usize,
    /// Non-blocking warnings (broken symlinks, copy fallbacks).
    pub warnings: Vec<ApplyWarning>,
}

/// Parse the schema-equivalent `files_to_copy_rules` JSON array form
/// (`design/03 §3.10`) into the same [`Rule`] list as [`parse`]. Each element
/// is `{ "pattern": "<glob>", "mode": "copy"|"symlink"|"exclude" }`. The
/// local-DB fallback when no checked-in `.worktreeinclude` exists (the
/// checked-in file wins, `design/03 §3.13`).
///
/// An empty / `[]` array yields an empty `Vec` (a no-op rule set). A
/// structurally-invalid blob or an unknown `mode` surfaces
/// [`Error::Validation`] with the `files_to_copy.invalid_json_rules` code so
/// the offending JSON is named.
pub fn parse_json_rules(json: &str) -> Result<Vec<Rule>> {
    #[derive(serde::Deserialize)]
    struct JsonRule {
        pattern: String,
        mode: String,
    }
    let raw: Vec<JsonRule> = serde_json::from_str(json)
        .map_err(|e| Error::Validation(format!("files_to_copy.invalid_json_rules: {e}")))?;
    let mut out = Vec::with_capacity(raw.len());
    for r in raw {
        let mode = match r.mode.as_str() {
            "copy" => Mode::Copy,
            "symlink" => Mode::Symlink,
            "exclude" => Mode::Exclude,
            other => {
                return Err(Error::Validation(format!(
                    "files_to_copy.invalid_json_rules: unknown mode {other:?} (expected copy|symlink|exclude)"
                )))
            }
        };
        if r.pattern.is_empty() {
            continue;
        }
        out.push(Rule {
            pattern: r.pattern,
            mode,
        });
    }
    Ok(out)
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
/// Returns an empty [`ApplyReport`] when there is no `.worktreeinclude` at the
/// project root — the resolver is a no-op for projects that don't opt in.
///
/// `project_root` and `dest_root` must already exist; both are
/// canonicalized for the escape-safety check.
///
/// This is the single-root convenience wrapper (source == reference root ==
/// `project_root`); the multi-repo call site uses [`apply_for_repo`] to pin
/// the source to the **reference** worktree while iterating destinations.
pub fn apply(project_root: &Path, dest_root: &Path) -> Result<ApplyReport> {
    let include_path = project_root.join(WORKTREEINCLUDE_RELPATH);
    let text = match std::fs::read_to_string(&include_path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(ApplyReport::default()),
        Err(e) => return Err(Error::Io(e)),
    };
    let rules = parse(&text);
    if rules.is_empty() {
        return Ok(ApplyReport::default());
    }
    apply_for_repo(project_root, dest_root, &rules)
}

/// Test-friendly variant: same as [`apply`] but takes already-parsed
/// rules instead of reading them from the project root. Equivalent to
/// [`apply_for_repo`] with `reference_root == project_root`.
pub fn apply_rules(project_root: &Path, dest_root: &Path, rules: &[Rule]) -> Result<ApplyReport> {
    apply_for_repo(project_root, dest_root, rules)
}

/// Apply a resolved rule set, resolving **sources** against the
/// `reference_root` (the workarea's first repo by `workspace_repos.position`)
/// and materializing matches into `repo_worktree` (one destination worktree).
///
/// The multi-repo entry point (Task 309, FROZEN signature). The call site
/// invokes this once per destination worktree with the **same**
/// `reference_root` so a reference-worktree match lands in every repo's
/// worktree (`design/03 §3.10`).
///
/// Walks `reference_root`, matches each entry against `rules`, and:
///
/// - skips files that match an `Exclude` rule (or no rule at all);
/// - copies files whose last-matching rule is `Copy`;
/// - symlinks files whose last-matching rule is `Symlink`, using a relative
///   target on Unix (junction/hardlink/copy fallback off-Unix).
///
/// Escape (`..` / symlink out of the reference or destination root) is a hard
/// [`Error::Validation`]. Broken source symlinks and Windows copy-fallbacks are
/// collected into [`ApplyReport::warnings`] and never fail the create.
///
/// `reference_root` and `repo_worktree` must already exist; both are
/// canonicalized for the escape-safety check.
pub fn apply_for_repo(
    reference_root: &Path,
    repo_worktree: &Path,
    rules: &[Rule],
) -> Result<ApplyReport> {
    let project_canon = std::fs::canonicalize(reference_root).map_err(|e| {
        Error::Io(std::io::Error::new(
            e.kind(),
            format!(
                "canonicalize reference_root {}: {e}",
                reference_root.display()
            ),
        ))
    })?;
    let dest_canon = std::fs::canonicalize(repo_worktree).map_err(|e| {
        Error::Io(std::io::Error::new(
            e.kind(),
            format!(
                "canonicalize repo_worktree {}: {e}",
                repo_worktree.display()
            ),
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

    let mut report = ApplyReport::default();
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
                // Broken symlink in the source tree — skip the entry and
                // record a non-blocking warning chip (`design/03 §3.10`:
                // "symlink to `<path>` is broken"; does not block create).
                tracing::debug!(
                    src = %src.display(),
                    "files_to_copy: skipping broken source symlink"
                );
                report.warnings.push(ApplyWarning::BrokenSymlink {
                    rel: rel.to_string_lossy().into_owned(),
                });
                continue;
            }
            Err(e) => return Err(Error::Io(e)),
        }

        match mode {
            Mode::Copy => {
                copy_file_idempotent(src, &dest)?;
            }
            Mode::Symlink => {
                // Returns whether the symlink fell back to a plain copy
                // (Windows / unsupported FS) so the caller can warn.
                if make_symlink_relative(src, &dest)? == SymlinkOutcome::CopiedFallback {
                    report.warnings.push(ApplyWarning::SymlinkUnsupported {
                        rel: rel.to_string_lossy().into_owned(),
                    });
                }
            }
            Mode::Exclude => unreachable!("filtered above"),
        }
        report.applied += 1;
    }
    Ok(report)
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

/// Whether [`create_symlink`] produced a true link or degraded to a copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymlinkOutcome {
    /// A real symlink / junction / hardlink was created.
    Linked,
    /// The filesystem supported none of those — `dest` is a plain copy
    /// (Windows without privilege + cross-volume). Caller emits a warning.
    CopiedFallback,
}

/// Create a relative symlink at `dest` pointing at `src`.
///
/// The link target is computed via [`pathdiff::diff_paths`] so the
/// workarea remains movable on Unix: relocating the whole workarea
/// preserves link integrity as long as the project root moves with it.
///
/// On Unix uses [`std::os::unix::fs::symlink`] (always [`SymlinkOutcome::Linked`]).
/// Off-Unix (Windows), [`create_symlink`] tries a real symlink first, then a
/// junction (dir) / hardlink (file), then a plain copy
/// ([`SymlinkOutcome::CopiedFallback`]) — see its docs.
fn make_symlink_relative(src: &Path, dest: &Path) -> Result<SymlinkOutcome> {
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
                        return Ok(SymlinkOutcome::Linked);
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

    let is_dir = src.is_dir();
    create_symlink(src, &target, dest, is_dir)
}

/// Unix: a relative symlink, the FROZEN V0.1 behavior. `src` (absolute) is
/// unused on Unix — the relative `target` is the link body so the workarea
/// stays movable.
#[cfg(unix)]
fn create_symlink(
    _src: &Path,
    target: &Path,
    dest: &Path,
    _is_dir: bool,
) -> Result<SymlinkOutcome> {
    std::os::unix::fs::symlink(target, dest).map_err(Error::Io)?;
    Ok(SymlinkOutcome::Linked)
}

/// Windows / non-Unix fallback (`design/03 §3.10`, Task 309).
///
/// Ordering:
/// 1. Try a **real symlink** (relative `target`) via
///    [`std::os::windows::fs::symlink_dir`] / `symlink_file` — succeeds when
///    the process holds `SeCreateSymbolicLinkPrivilege` (Developer Mode / admin),
///    keeping the workarea movable like Unix.
/// 2. On failure, fall back to a **directory junction** (dirs) via the
///    `mklink /J` shell builtin, or a **hardlink** (files) via
///    [`std::fs::hard_link`]. Junctions are **absolute** (a known Windows
///    divergence from the relative-symlink invariant; documented at the module
///    head) so we point them at the absolute `src`.
/// 3. On a cross-volume hardlink failure (or no junction), fall back to a plain
///    **copy** and return [`SymlinkOutcome::CopiedFallback`] so the caller emits
///    the one-time "symlinks unsupported here" warning.
///
/// Never returns `Error::Internal` for the symlink mode — only a genuine IO
/// error on the final copy fallback fails.
#[cfg(windows)]
fn create_symlink(src: &Path, target: &Path, dest: &Path, is_dir: bool) -> Result<SymlinkOutcome> {
    use std::os::windows::fs as winfs;

    // (1) Real symlink with the relative target (movable, Unix-equivalent).
    let real = if is_dir {
        winfs::symlink_dir(target, dest)
    } else {
        winfs::symlink_file(target, dest)
    };
    if real.is_ok() {
        return Ok(SymlinkOutcome::Linked);
    }

    // (2) Junction (dir) / hardlink (file), pointing at the absolute source.
    if is_dir {
        if create_windows_junction(src, dest).is_ok() {
            return Ok(SymlinkOutcome::Linked);
        }
    } else if std::fs::hard_link(src, dest).is_ok() {
        return Ok(SymlinkOutcome::Linked);
    }

    // (3) Final fallback: plain copy (files only; a dir that reached here is
    //     copied recursively). The caller surfaces the one-time warning.
    if is_dir {
        copy_dir_recursive(src, dest)?;
    } else {
        copy_file_idempotent(src, dest)?;
    }
    Ok(SymlinkOutcome::CopiedFallback)
}

/// Create a Windows directory junction at `dest` → `src` (absolute) via the
/// `mklink /J` shell builtin (no extra crate). `cmd /C mklink /J` needs no
/// elevated privilege, unlike a real directory symlink.
#[cfg(windows)]
fn create_windows_junction(src: &Path, dest: &Path) -> Result<()> {
    let src_abs = std::fs::canonicalize(src).unwrap_or_else(|_| src.to_path_buf());
    let status = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(dest)
        .arg(&src_abs)
        .status()
        .map_err(Error::Io)?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Io(std::io::Error::other(format!(
            "mklink /J failed for {} -> {}",
            dest.display(),
            src_abs.display()
        ))))
    }
}

/// Recursively copy `src` dir into `dest` (Windows copy fallback for a
/// symlinked directory). Reuses [`copy_file_idempotent`] per file.
#[cfg(windows)]
fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest).map_err(Error::Io)?;
    for entry in std::fs::read_dir(src).map_err(Error::Io)? {
        let entry = entry.map_err(Error::Io)?;
        let from = entry.path();
        let to = dest.join(entry.file_name());
        if entry.file_type().map_err(Error::Io)?.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            copy_file_idempotent(&from, &to)?;
        }
    }
    Ok(())
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
        assert_eq!(n.applied, 1);
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
        assert_eq!(n.applied, 1);
        assert!(n.warnings.is_empty());
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
        assert_eq!(n.applied, 1, ".env copied, .env.production excluded");
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
        assert_eq!(n.applied, 0);
    }

    #[test]
    fn apply_reads_include_file_when_present() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(src.path().join(".concerto")).unwrap();
        std::fs::write(src.path().join(WORKTREEINCLUDE_RELPATH), ".env\n").unwrap();
        std::fs::write(src.path().join(".env"), b"KEY=val\n").unwrap();
        let n = apply(src.path(), dst.path()).unwrap();
        assert_eq!(n.applied, 1);
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
        assert_eq!(n.applied, 0, ".git/ contents must not be visited");
    }

    #[test]
    fn parse_json_rules_round_trips_section_3_10_form() {
        // The schema-equivalent JSON from design/03 §3.10.
        let json = r#"[
          { "pattern": ".env*",            "mode": "copy"    },
          { "pattern": ".env.local",       "mode": "symlink" },
          { "pattern": ".vscode/",         "mode": "copy"    },
          { "pattern": ".env.production",  "mode": "exclude" }
        ]"#;
        let rules = parse_json_rules(json).unwrap();
        assert_eq!(rules.len(), 4);
        assert_eq!(rules[0].pattern, ".env*");
        assert_eq!(rules[0].mode, Mode::Copy);
        assert_eq!(rules[1].pattern, ".env.local");
        assert_eq!(rules[1].mode, Mode::Symlink);
        assert_eq!(rules[2].pattern, ".vscode/");
        assert_eq!(rules[2].mode, Mode::Copy);
        assert_eq!(rules[3].pattern, ".env.production");
        assert_eq!(rules[3].mode, Mode::Exclude);
    }

    #[test]
    fn parse_json_rules_empty_array_is_noop() {
        assert!(parse_json_rules("[]").unwrap().is_empty());
    }

    #[test]
    fn parse_json_rules_unknown_mode_is_validation_error() {
        let err = parse_json_rules(r#"[{"pattern":".env","mode":"hardlink"}]"#).unwrap_err();
        match err {
            Error::Validation(m) => {
                assert!(m.contains("files_to_copy.invalid_json_rules"), "got {m}")
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn parse_json_rules_malformed_is_validation_error() {
        let err = parse_json_rules("not json").unwrap_err();
        assert!(matches!(err, Error::Validation(_)));
    }

    #[cfg(unix)]
    #[test]
    fn apply_for_repo_resolves_sources_against_reference_root() {
        // The reference worktree holds the `.env`; a SEPARATE destination
        // worktree (a non-reference repo) receives it.
        let reference = tempfile::tempdir().unwrap();
        let other_repo = tempfile::tempdir().unwrap();
        std::fs::write(reference.path().join(".env"), b"shared\n").unwrap();
        let report = apply_for_repo(reference.path(), other_repo.path(), &parse(".env\n")).unwrap();
        assert_eq!(report.applied, 1);
        assert_eq!(
            std::fs::read(other_repo.path().join(".env")).unwrap(),
            b"shared\n",
            "reference-worktree .env lands in the non-reference repo's worktree"
        );
    }

    #[cfg(unix)]
    #[test]
    fn apply_for_repo_warns_on_broken_source_symlink() {
        // A `symlink`-mode rule whose source is itself a broken symlink:
        // skipped, never blocks, and surfaces a BrokenSymlink warning.
        let reference = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(
            reference.path().join("does-not-exist"),
            reference.path().join("link.toml"),
        )
        .unwrap();
        let report = apply_for_repo(reference.path(), dst.path(), &parse("link.toml !\n")).unwrap();
        assert_eq!(report.applied, 0, "broken source is skipped");
        assert_eq!(report.warnings.len(), 1);
        match &report.warnings[0] {
            ApplyWarning::BrokenSymlink { rel } => assert_eq!(rel, "link.toml"),
            other => panic!("expected BrokenSymlink, got {other:?}"),
        }
        assert_eq!(
            report.warnings[0].message(),
            "symlink to `link.toml` is broken"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_symlink_mode_falls_back_to_link_or_copy_not_error() {
        // On Windows the `symlink` mode must never `Error::Internal`: it
        // produces a real symlink (Developer Mode), a hardlink, or a copy
        // fallback (with a warning). Either way the bytes are present.
        let reference = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        std::fs::write(reference.path().join("shared.toml"), b"x").unwrap();
        let report =
            apply_for_repo(reference.path(), dst.path(), &parse("shared.toml !\n")).unwrap();
        assert_eq!(report.applied, 1);
        let dest = dst.path().join("shared.toml");
        assert!(
            dest.exists(),
            "symlink/hardlink/copy fallback must materialize the file"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"x");
        // Any warning present must be the SymlinkUnsupported copy-fallback
        // chip (a real link / hardlink emits none).
        for w in &report.warnings {
            assert!(matches!(w, ApplyWarning::SymlinkUnsupported { .. }));
        }
    }
}
