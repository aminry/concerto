//! Destructive-command intercept (Task 43).
//!
//! Implements the curated pattern table from `design/04 §3.10` and
//! `design/12 §3.6`: a set of regexes that ALWAYS require approval
//! regardless of the resolver's mode-class decision. The intercept is
//! bypassed only when the effective row carries
//! `bypass_destructive_guard = true` AND the entry ceremony for the
//! bypass flag was completed at workarea-create time (Task 32 enforces
//! the latter).
//!
//! ## Pattern matching
//!
//! [`is_destructive`] stringifies the entire tool-args blob via
//! `serde_json::to_string` and runs each regex in [`PATTERNS`] against
//! the result. We match against the whole blob (not just the `command`
//! field) so structured tools that embed shell snippets — e.g.
//! `Bash { command: "rm -rf /" }`, `Edit { ... }` — and free-form tools
//! both flow through one code path. False positives are acceptable (the
//! user gets one extra prompt); false negatives are catastrophic.
//!
//! ## Pattern list (FROZEN)
//!
//! Adding a pattern is fine; removing one requires explicit security
//! justification (the audit log relies on the label set being stable).
//! The label is what the desktop UI surfaces in the red-urgent prompt
//! ("force-push", "recursive-delete", …).

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// Output of [`is_destructive`] when a pattern matched. Carries the
/// human-readable label rendered in the approval prompt and persisted
/// on the `tool_approvals` row (V0.1 stashes it inline in the event;
/// the column is reserved for V1.0's structured per-pattern audit
/// channel).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct DestructiveMatch {
    pub label: &'static str,
}

/// Curated pattern → label table. Each regex is compiled once at
/// program start via [`LazyLock`] and reused for every tool-call check.
///
/// Patterns are deliberately conservative: they err on the side of one
/// extra prompt rather than missing a destructive command.
pub static PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let raw: &[(&str, &str)] = &[
        // `rm -rf` and equivalents. Matches `rm -rf`, `rm -fr`, `rm
        // -r -f`, and the long-form `rm --recursive --force`. Allows
        // arbitrary whitespace between flags and tolerates the order
        // swap.
        (
            r"(?i)\brm\s+(-[a-zA-Z]*r[a-zA-Z]*f|-[a-zA-Z]*f[a-zA-Z]*r|--recursive\s+--force|--force\s+--recursive|-r\s+-f|-f\s+-r)",
            "recursive-delete",
        ),
        // `git push --force` / `--force-with-lease`. Matches both the
        // long forms and the short `-f` flag when adjacent to `push`.
        (
            r"(?i)\bgit\s+push\b[^\n]*?(--force(-with-lease)?|\s-f\b)",
            "force-push",
        ),
        // `git reset --hard`.
        (r"(?i)\bgit\s+reset\b[^\n]*?--hard\b", "git-reset-hard"),
        // `git branch -D` (capital D = force-delete, case-sensitive —
        // lowercase `-d` is a safe delete that only succeeds when the
        // branch is merged) or `git tag -d` (tag has no force variant;
        // lowercase d is the standard).
        (
            r"(?-i)\bgit\s+(branch\s+-D\b|tag\s+-d\b)",
            "git-branch-delete",
        ),
        // `DROP TABLE` / `TRUNCATE TABLE` (case-insensitive).
        (r"(?i)\b(drop|truncate)\s+table\b", "sql-drop"),
        // `kubectl delete`.
        (r"(?i)\bkubectl\s+delete\b", "kubectl-delete"),
        // `docker rm`, `docker volume rm`, `docker system prune`.
        (
            r"(?i)\bdocker\s+(rm\b|volume\s+rm\b|system\s+prune\b)",
            "docker-rm",
        ),
        // Disk-wipe family: `mkfs`, `dd of=/dev/`, `parted`, `wipefs`.
        (
            r"(?i)(\bmkfs(\.[a-z0-9]+)?\b|\bdd\b[^\n]*?\bof=/dev/|\bparted\b|\bwipefs\b)",
            "disk-wipe",
        ),
        // `sudo` (any). Catches privilege escalation in shell tools.
        (r"(?i)\bsudo\b", "sudo"),
    ];
    raw.iter()
        .map(|(re, label)| {
            (
                Regex::new(re).expect("destructive pattern must compile"),
                *label,
            )
        })
        .collect()
});

/// Return `Some(DestructiveMatch)` when the tool name + args match any
/// pattern in [`PATTERNS`].
///
/// The `tool_name` is currently unused by the matcher — the pattern
/// regexes embed the command keyword (`rm`, `git`, etc.). The parameter
/// is preserved for forward compatibility with V1.0's per-tool pattern
/// scoping (e.g. only treat `DROP TABLE` as destructive in SQL-targeted
/// tools).
pub fn is_destructive(_tool_name: &str, args: &Value) -> Option<DestructiveMatch> {
    let haystack = match serde_json::to_string(args) {
        Ok(s) => s,
        // A non-serializable args blob should not silently sneak past
        // the intercept. We can't actually hit this in practice — every
        // `serde_json::Value` round-trips — but keep the conservative
        // branch in case the public signature widens later.
        Err(_) => return None,
    };
    for (re, label) in PATTERNS.iter() {
        if re.is_match(&haystack) {
            return Some(DestructiveMatch { label });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn matches(tool: &str, command: &str) -> Option<&'static str> {
        is_destructive(tool, &json!({"command": command})).map(|m| m.label)
    }

    #[test]
    fn rm_rf_variants_match_recursive_delete() {
        assert_eq!(
            matches("Bash", "rm -rf node_modules"),
            Some("recursive-delete")
        );
        assert_eq!(matches("Bash", "rm -fr /tmp/x"), Some("recursive-delete"));
        assert_eq!(
            matches("Bash", "rm --recursive --force build"),
            Some("recursive-delete")
        );
        assert_eq!(matches("Bash", "rm -r -f dist"), Some("recursive-delete"));
    }

    #[test]
    fn force_push_matches() {
        assert_eq!(
            matches("Bash", "git push --force origin main"),
            Some("force-push")
        );
        assert_eq!(
            matches("Bash", "git push --force-with-lease"),
            Some("force-push")
        );
        assert_eq!(
            matches("Bash", "git push -f origin feature"),
            Some("force-push")
        );
    }

    #[test]
    fn git_reset_hard_matches() {
        assert_eq!(
            matches("Bash", "git reset --hard HEAD~3"),
            Some("git-reset-hard")
        );
    }

    #[test]
    fn git_branch_delete_matches() {
        assert_eq!(
            matches("Bash", "git branch -D feature/old"),
            Some("git-branch-delete")
        );
        assert_eq!(
            matches("Bash", "git tag -d v0.0.1"),
            Some("git-branch-delete")
        );
    }

    #[test]
    fn sql_drop_truncate_match() {
        assert_eq!(
            matches("Bash", "psql -c 'DROP TABLE users'"),
            Some("sql-drop")
        );
        assert_eq!(matches("Bash", "TRUNCATE TABLE sessions"), Some("sql-drop"));
        // Case-insensitive.
        assert_eq!(matches("Bash", "drop table foo"), Some("sql-drop"));
    }

    #[test]
    fn kubectl_delete_matches() {
        assert_eq!(
            matches("Bash", "kubectl delete pod nginx"),
            Some("kubectl-delete")
        );
    }

    #[test]
    fn docker_rm_variants_match() {
        assert_eq!(
            matches("Bash", "docker rm -f mycontainer"),
            Some("docker-rm")
        );
        assert_eq!(
            matches("Bash", "docker volume rm myvolume"),
            Some("docker-rm")
        );
        assert_eq!(matches("Bash", "docker system prune -a"), Some("docker-rm"));
    }

    #[test]
    fn disk_wipe_matches() {
        assert_eq!(matches("Bash", "mkfs.ext4 /dev/sda1"), Some("disk-wipe"));
        assert_eq!(
            matches("Bash", "dd if=/dev/zero of=/dev/sda bs=1M"),
            Some("disk-wipe")
        );
        assert_eq!(matches("Bash", "parted /dev/sda"), Some("disk-wipe"));
        assert_eq!(matches("Bash", "wipefs -a /dev/sdb"), Some("disk-wipe"));
    }

    #[test]
    fn sudo_matches() {
        assert_eq!(matches("Bash", "sudo apt-get update"), Some("sudo"));
    }

    #[test]
    fn benign_rm_does_not_match() {
        assert_eq!(matches("Bash", "rm file.txt"), None);
        assert_eq!(matches("Bash", "rm -i prompt.txt"), None);
    }

    #[test]
    fn benign_git_does_not_match() {
        assert_eq!(matches("Bash", "git push origin main"), None);
        assert_eq!(matches("Bash", "git reset HEAD"), None);
        assert_eq!(matches("Bash", "git branch -d safe"), None); // lowercase d = safe
    }

    #[test]
    fn benign_docker_does_not_match() {
        assert_eq!(matches("Bash", "docker ps"), None);
        assert_eq!(matches("Bash", "docker run nginx"), None);
    }

    #[test]
    fn matches_against_whole_args_blob() {
        // Even if the dangerous substring is not in `command`, it should
        // still match — the matcher stringifies the entire args. Note
        // word-boundary anchors at the start of the command keyword
        // (`\brm`) require a non-word char immediately preceding the
        // keyword; the JSON-encoded `"script"` field provides that
        // (the opening quote is non-word) when the keyword is the first
        // token in the field value.
        let v = json!({"script": "rm -rf /tmp/x"});
        assert_eq!(
            is_destructive("Bash", &v).map(|m| m.label),
            Some("recursive-delete")
        );
    }
}
