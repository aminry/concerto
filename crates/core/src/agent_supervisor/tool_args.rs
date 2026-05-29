//! Tool-argument path extraction (Task 41).
//!
//! Best-effort extraction of the *filesystem path* a tool call is
//! targeting, used by the `PermissionResolver` to consult the
//! [`crate::security::path_policy`] allow/deny lists BEFORE the
//! mode-class table runs.
//!
//! V0.1 covers the Claude Code built-in tools: `Write`, `Edit`, `Read`,
//! `Bash`. For everything else the extractor returns `None` and the
//! resolver falls through to the mode-class table unchanged
//! (consistent with `design/12 §3.5`: "unparseable args conservatively
//! classify as Outside").
//!
//! The extractor is deliberately small — the tool-classification TOML
//! file (`design/04 §3.10` V1.0) will replace the inline match with a
//! per-tool JSON-pointer expression. For now the inline match is
//! exhaustive enough for the smoke gate.

use std::path::PathBuf;

use serde_json::Value;

/// Extract the primary filesystem path a tool call is targeting, or
/// `None` if no path can be parsed out of `args`.
///
/// Heuristics by tool name (case-sensitive — parser packs already
/// normalise tool names):
///
/// - `Write` / `Edit` / `Read` / `NotebookEdit`: look up
///   `args.file_path` first, then `args.path` as a fallback.
/// - `Bash`: scan `args.command` for the first absolute-looking path
///   token (`/foo/bar` or `~/foo`).
/// - everything else: return `None`.
///
/// The `~`-prefixed match in `Bash` is intentionally lexical (no
/// `$HOME` expansion here) — the deny-list constructor expands `~` at
/// build time, so this match is paranoia in case an agent writes
/// `cat ~/.ssh/id_rsa` directly into the shell. The path returned in
/// that case still starts with `~`; [`crate::security::path_policy::classify`]
/// resolves it via `canonicalize_or_clean` to the absolute deny prefix.
pub fn extract_path(tool_name: &str, args: &Value) -> Option<PathBuf> {
    match tool_name {
        "Write" | "Edit" | "Read" | "NotebookEdit" => extract_path_field(args),
        "Bash" => extract_path_from_command(args),
        _ => None,
    }
}

/// Look up `file_path` or `path` on a JSON object, returning the first
/// non-empty string.
fn extract_path_field(args: &Value) -> Option<PathBuf> {
    let obj = args.as_object()?;
    for key in ["file_path", "path", "target"] {
        if let Some(s) = obj.get(key).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return Some(PathBuf::from(s));
            }
        }
    }
    None
}

/// Pull the first absolute-looking path token out of a `Bash` tool's
/// `command` string. Returns `None` for relative-only commands — those
/// are conservatively classified as `Outside` by the resolver, which
/// matches the V0.1 "best-effort" stance from `tasks/41`
/// §"Implementation notes".
fn extract_path_from_command(args: &Value) -> Option<PathBuf> {
    let obj = args.as_object()?;
    let cmd = obj.get("command").and_then(|v| v.as_str())?;
    for raw in cmd.split_whitespace() {
        // Strip a single layer of surrounding quotes so `cat '/etc/x'`
        // matches.
        let token = raw.trim_matches(|c| c == '\'' || c == '"');
        // Strip a `>` redirect prefix so `echo foo > /tmp/bar` matches
        // (`>`-glued tokens are uncommon but cheap to handle).
        let token = token.trim_start_matches('>');
        if token.starts_with('/') || token.starts_with("~/") {
            return Some(PathBuf::from(token));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn write_uses_file_path() {
        let args = json!({"file_path": "/tmp/x.txt", "content": "hi"});
        assert_eq!(
            extract_path("Write", &args),
            Some(PathBuf::from("/tmp/x.txt"))
        );
    }

    #[test]
    fn read_falls_back_to_path() {
        let args = json!({"path": "/tmp/y.txt"});
        assert_eq!(
            extract_path("Read", &args),
            Some(PathBuf::from("/tmp/y.txt"))
        );
    }

    #[test]
    fn edit_returns_none_when_no_path() {
        let args = json!({"content": "no path here"});
        assert_eq!(extract_path("Edit", &args), None);
    }

    #[test]
    fn bash_extracts_first_absolute_path() {
        let args = json!({"command": "ls -la /etc/passwd /home/user"});
        assert_eq!(
            extract_path("Bash", &args),
            Some(PathBuf::from("/etc/passwd"))
        );
    }

    #[test]
    fn bash_extracts_tilde_path() {
        let args = json!({"command": "cat ~/.ssh/id_rsa"});
        assert_eq!(
            extract_path("Bash", &args),
            Some(PathBuf::from("~/.ssh/id_rsa"))
        );
    }

    #[test]
    fn bash_returns_none_for_relative_only() {
        let args = json!({"command": "ls -la"});
        assert_eq!(extract_path("Bash", &args), None);
    }

    #[test]
    fn unknown_tool_returns_none() {
        let args = json!({"file_path": "/tmp/x.txt"});
        assert_eq!(extract_path("Mystery", &args), None);
    }

    #[test]
    fn bash_strips_quotes() {
        let args = json!({"command": "cat '/etc/hosts'"});
        assert_eq!(
            extract_path("Bash", &args),
            Some(PathBuf::from("/etc/hosts"))
        );
    }

    #[test]
    fn bash_strips_redirect() {
        let args = json!({"command": "echo hi >/tmp/out"});
        assert_eq!(extract_path("Bash", &args), Some(PathBuf::from("/tmp/out")));
    }
}
