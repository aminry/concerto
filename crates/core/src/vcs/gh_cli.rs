//! `gh` CLI shell-out backend for the V0.1 VCS Provider (Task 45,
//! `design/13 §2` "gh CLI shell-out" row).
//!
//! Every public function spawns `gh` via [`tokio::process::Command`]
//! with `--json <fields>` for structured output, captures stdout, and
//! parses it via `serde_json::from_slice`. Stderr is captured into the
//! returned error message on non-zero exit so the UI can surface
//! actionable text (e.g. `gh: not authenticated`).
//!
//! ## Security / token hygiene
//!
//! - The full subprocess output is NEVER logged via `tracing::*` — only
//!   the command name plus argument count. PRs may contain secrets in
//!   bodies / titles; check-run details URLs may be private.
//! - `GH_TOKEN` is NOT injected by this module; we let `gh` use its
//!   own keychain-managed auth (the V0.1 default per `design/13 §3.1`
//!   "gh CLI fallback" row). A future task can plug in a Concerto-
//!   keychain-supplied token via the environment.
//!
//! ## Title / body via temp files
//!
//! PR titles and bodies may contain newlines, backticks, and other
//! shell-hostile characters. We materialize them to
//! [`tempfile::NamedTempFile`] and pass `--title-file` / `--body-file`
//! to dodge every escaping headache.

use std::ffi::OsStr;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::Stdio;

use concerto_error::{Error, Result};
use serde::Deserialize;
use tokio::process::Command;

/// Resolved path to the `gh` binary on the user's `PATH`, or an error
/// the caller can surface to the user verbatim.
///
/// Walks `$PATH` once at handle construction (cached on
/// [`crate::vcs::VcsHandle`]). The walk is `which`-style: split `PATH`
/// on the OS separator, probe each entry for `gh` (or `gh.exe` on
/// Windows for parity, even though V0.1 is macOS-only), return the
/// first executable hit.
pub fn resolve_gh_path() -> Result<PathBuf> {
    let path_var = std::env::var_os("PATH")
        .ok_or_else(|| Error::Internal("PATH environment variable is not set".into()))?;
    for dir in std::env::split_paths(&path_var) {
        for name in ["gh", "gh.exe"] {
            let candidate = dir.join(name);
            if is_executable_file(&candidate) {
                return Ok(candidate);
            }
        }
    }
    Err(Error::Internal(
        "gh CLI not installed: install via `brew install gh` and run `gh auth login`".into(),
    ))
}

#[cfg(unix)]
fn is_executable_file(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(p) {
        Ok(md) => md.is_file() && (md.permissions().mode() & 0o111) != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable_file(p: &std::path::Path) -> bool {
    std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
}

/// Probe `gh auth status --hostname github.com`. Returns `Ok(())` when
/// the user is authenticated; [`Error::VcsNotAuthenticated`] otherwise.
///
/// `gh auth status` exits non-zero when the user has not run
/// `gh auth login`; stderr carries the human-readable explanation.
pub async fn check_auth(gh: &std::path::Path) -> Result<()> {
    let output = run_gh(gh, &["auth", "status", "--hostname", "github.com"]).await?;
    if !output.status.success() {
        // The remediation hint goes into the error message — the UI
        // surfaces it verbatim.
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(Error::VcsNotAuthenticated(if stderr.is_empty() {
            "gh CLI is not authenticated; run `gh auth login`".into()
        } else {
            format!("{stderr}; run `gh auth login`")
        }));
    }
    Ok(())
}

/// One PR summary, mirrored from `gh pr list --json` field names.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrSummary {
    pub number: i64,
    pub title: String,
    pub state: String,
    pub url: String,
    pub head_ref_name: String,
    pub base_ref_name: String,
}

/// Full PR detail, mirrored from `gh pr view --json` field names. Only
/// the fields the V0.1 cache row needs are pulled.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrDetail {
    pub number: i64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    pub state: String,
    pub url: String,
    pub head_ref_name: String,
    pub base_ref_name: String,
    pub head_ref_oid: String,
}

/// A single check run. Mirrors `gh pr view --json statusCheckRollup`
/// entries; the type's `__typename` is `CheckRun` (workflow steps) or
/// `StatusContext` (legacy commit status). V0.1 normalizes both into
/// this shape.
#[derive(Debug, Clone)]
pub struct CheckRun {
    pub name: String,
    /// `queued | in_progress | completed` (CheckRun) OR `pending |
    /// success | failure | error` (StatusContext, copied verbatim).
    pub status: String,
    pub conclusion: String,
    pub details_url: String,
}

/// GitHub issue projection — minimal shape for the V0.1 fetch RPC.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueDetail {
    pub number: i64,
    pub title: String,
    #[serde(default)]
    pub body: String,
    pub state: String,
    pub url: String,
    #[serde(default)]
    pub labels: Vec<IssueLabel>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IssueLabel {
    pub name: String,
}

/// `gh pr list --repo <repo> --json number,title,state,url,headRefName,baseRefName`.
pub async fn list_prs(gh: &std::path::Path, repo: &str) -> Result<Vec<PrSummary>> {
    let output = run_gh(
        gh,
        &[
            "pr",
            "list",
            "--repo",
            repo,
            "--json",
            "number,title,state,url,headRefName,baseRefName",
        ],
    )
    .await?;
    require_success(&output, "gh pr list")?;
    serde_json::from_slice(&output.stdout)
        .map_err(|e| Error::Vcs(format!("parse gh pr list JSON: {e}")))
}

/// `gh pr view <number> --repo <repo> --json …`.
pub async fn view_pr(gh: &std::path::Path, repo: &str, number: i64) -> Result<PrDetail> {
    let number_str = number.to_string();
    let output = run_gh(
        gh,
        &[
            "pr",
            "view",
            &number_str,
            "--repo",
            repo,
            "--json",
            "number,title,body,state,url,headRefName,baseRefName,headRefOid",
        ],
    )
    .await?;
    require_success(&output, "gh pr view")?;
    serde_json::from_slice(&output.stdout)
        .map_err(|e| Error::Vcs(format!("parse gh pr view JSON: {e}")))
}

/// `gh pr create --repo <repo> --base <base> --head <head>
/// --title-file <tmp> --body-file <tmp>`. Returns the assigned PR
/// number.
///
/// Title and body are written to [`tempfile::NamedTempFile`]s to dodge
/// shell-escaping issues; the files are dropped (and deleted) when
/// this call returns.
pub async fn create_pr(
    gh: &std::path::Path,
    repo: &str,
    head: &str,
    base: &str,
    title: &str,
    body: &str,
) -> Result<i64> {
    // tempfile keeps the file open until the variable is dropped;
    // closing `_keep_alive` would unlink it. Bind to a local so the
    // path stays valid through the `gh` call.
    let mut title_file = tempfile::NamedTempFile::new()?;
    title_file.write_all(title.as_bytes())?;
    title_file.flush()?;

    let mut body_file = tempfile::NamedTempFile::new()?;
    body_file.write_all(body.as_bytes())?;
    body_file.flush()?;

    let title_path = title_file.path().to_string_lossy().into_owned();
    let body_path = body_file.path().to_string_lossy().into_owned();

    let output = run_gh(
        gh,
        &[
            "pr",
            "create",
            "--repo",
            repo,
            "--base",
            base,
            "--head",
            head,
            "--title-file",
            &title_path,
            "--body-file",
            &body_path,
        ],
    )
    .await?;
    require_success(&output, "gh pr create")?;
    // `gh pr create` (without --json) prints the new PR URL on stdout.
    // The last `/<number>` path segment is the assigned PR number.
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_pr_number_from_url(stdout.trim())
}

/// `gh api /repos/<owner>/<repo>/commits/<sha>/check-runs --jq` —
/// returns the flat list of check runs for a commit.
pub async fn get_check_runs(gh: &std::path::Path, repo: &str, sha: &str) -> Result<Vec<CheckRun>> {
    let endpoint = format!("repos/{repo}/commits/{sha}/check-runs");
    let output = run_gh(
        gh,
        &[
            "api",
            &endpoint,
            "--jq",
            ".check_runs[] | {name: .name, status: .status, conclusion: (.conclusion // \"\"), details_url: (.details_url // \"\")}",
        ],
    )
    .await?;
    require_success(&output, "gh api check-runs")?;

    // The `--jq` output is one JSON object per line (no enclosing
    // array). Parse line-by-line; skip blank lines.
    let stdout = std::str::from_utf8(&output.stdout)
        .map_err(|e| Error::Vcs(format!("gh api stdout is not UTF-8: {e}")))?;
    let mut runs = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        #[derive(Deserialize)]
        struct Row {
            name: String,
            status: String,
            #[serde(default)]
            conclusion: String,
            #[serde(default)]
            details_url: String,
        }
        let row: Row = serde_json::from_str(line)
            .map_err(|e| Error::Vcs(format!("parse check-runs row: {e}")))?;
        runs.push(CheckRun {
            name: row.name,
            status: row.status,
            conclusion: row.conclusion,
            details_url: row.details_url,
        });
    }
    Ok(runs)
}

/// `gh pr merge <number> --repo <repo> --<method>`. `method` is
/// validated by the caller (only `merge|squash|rebase` are accepted).
pub async fn merge_pr(gh: &std::path::Path, repo: &str, number: i64, method: &str) -> Result<()> {
    let number_str = number.to_string();
    let method_flag = match method {
        "merge" | "" => "--merge",
        "squash" => "--squash",
        "rebase" => "--rebase",
        other => {
            return Err(Error::Validation(format!(
                "merge method must be merge|squash|rebase (got `{other}`)"
            )));
        }
    };
    let output = run_gh(
        gh,
        &["pr", "merge", &number_str, "--repo", repo, method_flag],
    )
    .await?;
    require_success(&output, "gh pr merge")?;
    Ok(())
}

/// `gh issue view <number> --repo <repo> --json …`.
pub async fn view_issue(gh: &std::path::Path, repo: &str, number: i64) -> Result<IssueDetail> {
    let number_str = number.to_string();
    let output = run_gh(
        gh,
        &[
            "issue",
            "view",
            &number_str,
            "--repo",
            repo,
            "--json",
            "number,title,body,state,url,labels",
        ],
    )
    .await?;
    require_success(&output, "gh issue view")?;
    serde_json::from_slice(&output.stdout)
        .map_err(|e| Error::Vcs(format!("parse gh issue view JSON: {e}")))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Run `gh` with the given argv. Stdout / stderr are both captured
/// into the returned [`std::process::Output`]; only the command name +
/// argument count are logged so PR titles / tokens never reach the
/// trace stream.
async fn run_gh<S: AsRef<OsStr>>(gh: &std::path::Path, args: &[S]) -> Result<std::process::Output> {
    tracing::debug!(
        gh_path = %gh.display(),
        argc = args.len(),
        "spawning gh"
    );
    // Spawn with a bounded retry on ETXTBSY ("Text file busy", os error
    // 26). In a multithreaded process, another thread's fork() (any
    // `Command::spawn`) can land between a freshly-written executable's
    // open-for-write and its close; the forked child inherits that write
    // FD and keeps the file busy-for-write across the window until its own
    // execve, so a concurrent exec of that binary fails with ETXTBSY. It
    // is transient — a short retry clears it. (This is the same mitigation
    // Cargo applies in `retry_etxtbsy`.) In production `gh` is a stable
    // installed binary so this almost never fires, but it hardens the
    // shell-out against a `gh` self-update racing a spawn, and it removes
    // the flake from the freshly-written mock `gh` in the integration
    // tests under `--test-threads` parallelism.
    const MAX_ETXTBSY_RETRIES: u32 = 10;
    let mut attempt: u32 = 0;
    loop {
        match Command::new(gh)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
        {
            Ok(output) => return Ok(output),
            Err(e) if is_etxtbsy(&e) && attempt < MAX_ETXTBSY_RETRIES => {
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            Err(e) => return Err(Error::Vcs(format!("spawn gh: {e}"))),
        }
    }
}

/// True when `e` is `ETXTBSY` ("Text file busy"). The raw errno is `26`
/// on both Linux and macOS; on non-Unix the condition can't arise, so
/// this is always `false` (the retry loop then degrades to a single try).
fn is_etxtbsy(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        e.raw_os_error() == Some(26)
    }
    #[cfg(not(unix))]
    {
        let _ = e;
        false
    }
}

/// Map a non-zero exit into [`Error::Vcs`] / [`Error::VcsNotAuthenticated`].
/// Stderr is included in the message so the caller-facing text is
/// actionable.
fn require_success(output: &std::process::Output, op: &str) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    // `gh` prints "not authenticated" / "auth required" patterns when
    // the user has not run `gh auth login`. Detect them and surface
    // the typed `VcsNotAuthenticated` so the UI can route the user to
    // the auth wizard.
    let lower = stderr.to_lowercase();
    if lower.contains("not authenticated")
        || lower.contains("authentication required")
        || lower.contains("requires authentication")
    {
        return Err(Error::VcsNotAuthenticated(stderr));
    }
    let exit = output.status.code().unwrap_or(-1);
    Err(Error::Vcs(format!("{op} exit {exit}: {stderr}")))
}

/// Extract `<number>` from the trailing path segment of a GitHub PR
/// URL (`https://github.com/owner/repo/pull/123`).
fn parse_pr_number_from_url(url: &str) -> Result<i64> {
    let segment = url
        .rsplit('/')
        .find(|s| !s.is_empty())
        .ok_or_else(|| Error::Vcs(format!("gh pr create returned empty URL `{url}`")))?;
    segment.parse::<i64>().map_err(|e| {
        Error::Vcs(format!(
            "gh pr create URL `{url}` has non-numeric tail: {e}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pr_number_from_clean_url() {
        assert_eq!(
            parse_pr_number_from_url("https://github.com/owner/repo/pull/42").unwrap(),
            42
        );
    }

    #[test]
    fn parse_pr_number_tolerates_trailing_newline() {
        // The leading whitespace `trim()` is applied by the caller; this
        // test only checks the parser strips trailing slashes / empty
        // segments.
        assert_eq!(
            parse_pr_number_from_url("https://github.com/owner/repo/pull/7/").unwrap(),
            7
        );
    }

    #[test]
    fn parse_pr_number_rejects_non_numeric() {
        assert!(parse_pr_number_from_url("https://example.com/").is_err());
    }
}
