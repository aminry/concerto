//! `git log` recent-commit walk + `git grep` cross-worktree search helpers
//! (Task 405).
//!
//! The Maestro read tools `get_workarea_recent_commits` and
//! `cross_workarea_search` (`design/08 §5.1`) need two git primitives that did
//! not exist in `gix-wrap` before this task: a bounded commit-log walk and a
//! scoped content search. Following the 305 placement precedent (and the
//! [`crate::ahead::commits_ahead`] sibling), the git/`gix` tooling lives in
//! `gix-wrap` (a `git` shell-out through the existing [`cmd::run`] helper), so
//! `core` gains no new git dependency or `cargo deny` surface.
//!
//! Both helpers are a deliberate `git`-CLI shell-out rather than a native `gix`
//! revwalk / search:
//!
//! - `git log --format=…` ships everywhere `git` does and gives us the
//!   author/summary/timestamp fields verbatim with no `gix` feature bump
//!   (a `gix` revwalk would still need a `gix-traverse`/`gix-revision` surface
//!   to format the same fields). The [`Commit`] shape is FROZEN here.
//! - `git grep` is the cross-platform content search the design's V1.0 live-grep
//!   path (`design/08 R-6`) calls for — it ships with every `git`, honors the
//!   repo's `.gitignore`/sparse-checkout, and avoids adding a `ripgrep`/`grep`
//!   crate dependency. The [`GrepHit`] shape is FROZEN here.

use std::path::Path;

use concerto_error::{Error, Result};

use crate::cmd;

/// One commit from a [`recent_commits`] walk. Mirrors the `design/08 §5.1`
/// `Commit` return shape (`{oid, short_oid, summary, author, committed_at}`).
/// FROZEN by Task 405 (see the `lib.rs` doc-comment block).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Full 40-char commit OID.
    pub oid: String,
    /// Abbreviated OID (`git`'s default short form).
    pub short_oid: String,
    /// First line of the commit message (the subject).
    pub summary: String,
    /// Author name.
    pub author: String,
    /// Author timestamp as unix **seconds**.
    pub committed_at: i64,
}

/// The record separator the `recent_commits` format string emits between fields.
/// A unit-separator byte (`0x1f`) never appears in commit metadata, so it splits
/// cleanly even when a summary contains tabs/pipes.
const FIELD_SEP: char = '\u{1f}';
/// The record separator between whole commits (a record-separator byte, `0x1e`).
const RECORD_SEP: char = '\u{1e}';

/// Walk the `limit` most-recent commits reachable from `branch` (newest first).
///
/// Shells out `git log --max-count=<limit> --format=…` scoped to `branch`. The
/// `--format` packs `%H` / `%h` / `%s` / `%an` / `%at` separated by a
/// unit-separator byte, commits separated by a record-separator byte, so the
/// output parses unambiguously regardless of message content. `branch` is
/// passed through verbatim — callers building it from user/DB input validate
/// first (it is followed by `--` so it can never be read as a path/flag).
///
/// An empty repo (no commits / unknown ref) surfaces as an `Err` from `git`
/// rather than a silent empty list, mirroring [`crate::ahead::commits_ahead`]'s
/// unknown-base behavior. `limit == 0` returns an empty list without spawning.
pub async fn recent_commits(repo_dir: &Path, branch: &str, limit: usize) -> Result<Vec<Commit>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let max = format!("--max-count={limit}");
    // %H=oid %h=short %s=subject %an=author-name %at=author-date(unix s).
    let format =
        format!("--format=%H{FIELD_SEP}%h{FIELD_SEP}%s{FIELD_SEP}%an{FIELD_SEP}%at{RECORD_SEP}");
    let out = cmd::run(&["log", &max, &format, branch, "--"], repo_dir).await?;
    Ok(parse_log(&out.stdout))
}

/// Parse the record/field-separated `git log` output [`recent_commits`] emits.
fn parse_log(stdout: &str) -> Vec<Commit> {
    let mut commits = Vec::new();
    for record in stdout.split(RECORD_SEP) {
        let record = record.trim_matches(['\n', '\r']);
        if record.is_empty() {
            continue;
        }
        let mut fields = record.split(FIELD_SEP);
        let oid = fields.next().unwrap_or("").to_string();
        let short_oid = fields.next().unwrap_or("").to_string();
        let summary = fields.next().unwrap_or("").to_string();
        let author = fields.next().unwrap_or("").to_string();
        let committed_at = fields
            .next()
            .unwrap_or("")
            .trim()
            .parse::<i64>()
            .unwrap_or(0);
        if oid.is_empty() {
            continue;
        }
        commits.push(Commit {
            oid,
            short_oid,
            summary,
            author,
            committed_at,
        });
    }
    commits
}

/// One `git grep` match from a [`grep`] search. Mirrors the `design/08 §5.1`
/// `Hit` shape's per-repo fields (`{path, line, snippet}`); the caller attaches
/// the `workarea`/`repo` context. FROZEN by Task 405.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepHit {
    /// Forward-slash repo-relative path of the matching file (git always emits
    /// forward slashes, so this is cross-platform without normalization).
    pub path: String,
    /// 1-based line number of the match.
    pub line: u32,
    /// The matching line, trimmed and capped to [`MAX_SNIPPET_LEN`] chars.
    pub snippet: String,
}

/// Per-snippet character cap applied before a hit is returned (the `design/08
/// §8` tool guardrail: never let a 10 MB line reach the LLM).
pub const MAX_SNIPPET_LEN: usize = 200;

/// Search `query` (a fixed string, not a regex) across the tracked files of the
/// worktree at `repo_dir`, returning up to `max_hits` matches.
///
/// Shells out `git grep -n -I --fixed-strings --no-color -e <query>`:
/// - `-n` prefixes each match with its 1-based line number.
/// - `-I` skips binary files.
/// - `--fixed-strings` treats `query` as a literal (no regex injection from an
///   LLM-supplied query) and lets `query` be passed after `-e` so it can never
///   be read as a flag.
/// - The search honors the worktree's sparse-checkout + `.gitignore`, scanning
///   only tracked, in-cone files — exactly the V1.0 live-grep scope
///   (`design/08 R-6`).
///
/// `git grep` exits `1` when there are **no matches** (and `0` when there are);
/// both are success here, so a no-match search returns `Ok(vec![])` rather than
/// an `Err`. A genuine failure (bad repo, unreadable index) still surfaces as an
/// `Err`. Each snippet is trimmed and capped to [`MAX_SNIPPET_LEN`]; the result
/// is capped to `max_hits` (the `design/08 §8` guardrail).
pub async fn grep(repo_dir: &Path, query: &str, max_hits: usize) -> Result<Vec<GrepHit>> {
    if query.is_empty() || max_hits == 0 {
        return Ok(Vec::new());
    }
    // `git grep` does not accept an empty pattern and returns a usage error; the
    // empty-query short-circuit above already handled that.
    let out = match cmd::run(
        &[
            "grep",
            "-n",
            "-I",
            "--no-color",
            "--fixed-strings",
            "-e",
            query,
        ],
        repo_dir,
    )
    .await
    {
        Ok(out) => out,
        Err(Error::Git(msg)) if is_no_match(&msg) => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(parse_grep(&out.stdout, max_hits))
}

/// `git grep` signals "no matches found" with exit code `1`; [`cmd::run`] maps
/// that onto `Error::Git("git grep: exit 1: …")`. Recognize that specific shape
/// so it becomes an empty result rather than a propagated error. Any other exit
/// code (≥ 2 = a real error) is left to propagate.
fn is_no_match(msg: &str) -> bool {
    msg.contains("git grep: exit 1:")
}

/// Parse `git grep -n` output (`path:line:content`) into capped [`GrepHit`]s.
fn parse_grep(stdout: &str, max_hits: usize) -> Vec<GrepHit> {
    let mut hits = Vec::new();
    for raw in stdout.lines() {
        if hits.len() >= max_hits {
            break;
        }
        // `path:line:content` — split twice from the left so a `:` in the
        // content (common) does not corrupt the path/line fields.
        let mut parts = raw.splitn(3, ':');
        let path = match parts.next() {
            Some(p) if !p.is_empty() => p.to_string(),
            _ => continue,
        };
        let line = match parts.next().and_then(|n| n.trim().parse::<u32>().ok()) {
            Some(n) => n,
            None => continue,
        };
        let content = parts.next().unwrap_or("");
        hits.push(GrepHit {
            path,
            line,
            snippet: cap_snippet(content),
        });
    }
    hits
}

/// Trim a matched line and cap it to [`MAX_SNIPPET_LEN`] chars on a char
/// boundary (the `design/08 §8` per-snippet guardrail).
fn cap_snippet(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.chars().count() <= MAX_SNIPPET_LEN {
        return trimmed.to_string();
    }
    trimmed.chars().take(MAX_SNIPPET_LEN).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tempfile::TempDir;
    use tokio::process::Command;

    async fn git(args: &[&str], cwd: &Path) {
        let out = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_AUTHOR_NAME", "Ada")
            .env("GIT_AUTHOR_EMAIL", "ada@example.com")
            .env("GIT_COMMITTER_NAME", "Ada")
            .env("GIT_COMMITTER_EMAIL", "ada@example.com")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .await
            .expect("spawn git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    async fn repo_with_commits(n: usize) -> TempDir {
        let work = TempDir::new().unwrap();
        git(&["init", "-b", "main", "."], work.path()).await;
        for i in 0..n {
            tokio::fs::write(work.path().join(format!("f{i}.txt")), format!("body {i}\n"))
                .await
                .unwrap();
            git(&["add", "."], work.path()).await;
            git(
                &["commit", "-m", &format!("commit number {i}")],
                work.path(),
            )
            .await;
        }
        work
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn recent_commits_returns_newest_first() {
        let work = repo_with_commits(3).await;
        let commits = recent_commits(work.path(), "main", 10).await.expect("log");
        assert_eq!(commits.len(), 3);
        // Newest first: commit 2, then 1, then 0.
        assert_eq!(commits[0].summary, "commit number 2");
        assert_eq!(commits[1].summary, "commit number 1");
        assert_eq!(commits[2].summary, "commit number 0");
        // OIDs are well-formed and short_oid is a prefix of oid.
        for c in &commits {
            assert_eq!(c.oid.len(), 40, "full oid");
            assert!(c.oid.starts_with(&c.short_oid), "short is a prefix");
            assert_eq!(c.author, "Ada");
            assert!(c.committed_at > 0, "author date populated");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn recent_commits_respects_limit() {
        let work = repo_with_commits(5).await;
        let commits = recent_commits(work.path(), "main", 2).await.expect("log");
        assert_eq!(commits.len(), 2, "limit caps the walk");
        assert_eq!(commits[0].summary, "commit number 4");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn recent_commits_zero_limit_is_empty() {
        let work = repo_with_commits(1).await;
        assert!(recent_commits(work.path(), "main", 0)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn recent_commits_errors_on_unknown_branch() {
        let work = repo_with_commits(1).await;
        let r = recent_commits(work.path(), "no-such-branch", 5).await;
        assert!(r.is_err(), "unknown ref should error, got {r:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn grep_finds_a_planted_hit() {
        let work = repo_with_commits(0).await;
        tokio::fs::write(
            work.path().join("auth.rs"),
            "fn login() {}\nlet NEEDLE_TOKEN = 1;\n",
        )
        .await
        .unwrap();
        git(&["add", "."], work.path()).await;
        git(&["commit", "-m", "add auth"], work.path()).await;

        let hits = grep(work.path(), "NEEDLE_TOKEN", 50).await.expect("grep");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "auth.rs");
        assert_eq!(hits[0].line, 2);
        assert!(hits[0].snippet.contains("NEEDLE_TOKEN"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn grep_no_match_is_empty_not_error() {
        let work = repo_with_commits(1).await;
        let hits = grep(work.path(), "definitely-absent-xyzzy", 50)
            .await
            .expect("no-match is Ok, not Err");
        assert!(hits.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn grep_caps_hits() {
        let work = repo_with_commits(0).await;
        let mut body = String::new();
        for _ in 0..20 {
            body.push_str("MATCHME here\n");
        }
        tokio::fs::write(work.path().join("many.txt"), body)
            .await
            .unwrap();
        git(&["add", "."], work.path()).await;
        git(&["commit", "-m", "many"], work.path()).await;

        let hits = grep(work.path(), "MATCHME", 5).await.expect("grep");
        assert_eq!(hits.len(), 5, "result is capped to max_hits");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn grep_fixed_string_does_not_treat_query_as_regex() {
        let work = repo_with_commits(0).await;
        tokio::fs::write(work.path().join("re.txt"), "a.b literal\naxb regex\n")
            .await
            .unwrap();
        git(&["add", "."], work.path()).await;
        git(&["commit", "-m", "re"], work.path()).await;

        // `a.b` as a fixed string matches only the literal line, not `axb`.
        let hits = grep(work.path(), "a.b", 50).await.expect("grep");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("a.b literal"));
    }

    #[test]
    fn cap_snippet_truncates_long_lines() {
        let long = "x".repeat(500);
        let capped = cap_snippet(&long);
        assert_eq!(capped.chars().count(), MAX_SNIPPET_LEN);
    }
}
