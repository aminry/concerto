//! PR title/body composition (Task 321, `design/13 §3.4` + §12 R-4).
//!
//! This module holds the **deterministic, pure** pieces of the PR-compose
//! step: the [`PrComposeContext`] the compose entry point consumes, the
//! `title\n\nbody` split, the deterministic title/body builder, the
//! `.github/pull_request_template.md` fold, and the Concerto footer. The
//! orchestration ([`crate::workspace_manager::WorkareaManager::compose_pr`] /
//! `create_pr_for_repo`) lives in `workarea.rs` because it needs `&self` (the
//! settings resolver + the [`crate::llm::OneShotLlm`] seam).
//!
//! ## Reuse, not new machinery (D1)
//!
//! Per `PHASE3_PLANNING §1 D1` + the 312/321 row in §2, **Task 312 owns** the
//! `OneShotLlm` trait + `DeterministicOneShot` + `compose_action_prompt`. This
//! task adds **no new LLM machinery** — it routes the base prompt through
//! `compose_action_prompt(ActionKind::PrCreate, …)`, calls the seam with a 2 s
//! timeout, and **always** falls back to the LIVE deterministic composer
//! (`DeterministicOneShot`, which for `PrCreate` returns a `title\n\nbody`
//! template). In Phase 3 no real provider is wired (412), so the deterministic
//! fallback IS the live path — fully CI-provable here. The live-LLM-quality
//! path is wired in P4 (412) and judged at that phase gate.

use std::time::Duration;

use concerto_persist::{RepositoryId, WorkareaId};

/// Wall-clock budget for the [`crate::llm::OneShotLlm::suggest`] call (FROZEN,
/// `design/13 §12 R-4`). On `Elapsed` (or any provider error) the compose step
/// falls straight to the deterministic composer; PR creation never blocks past
/// this budget on the LLM.
pub const LLM_TIMEOUT: Duration = Duration::from_secs(2);

/// The opt-out key (FROZEN, `design/13 §12 R-4`). A `bool` in the project's
/// `settings_json` PR-defaults; **absent ⇒ on** (composition default-on). When
/// `false`, the compose step skips the LLM and uses the deterministic composer
/// directly (the footer + template fold still apply).
pub const PR_COMPOSE_KEY: &str = "pr_compose";

/// The resolved composition context (FROZEN field set, `design/13 §3.4` step 1).
///
/// The caller (`create_pr_for_repo`) resolves these workarea-side and hands
/// them to `compose_pr`; the compose step itself adds the per-repo
/// `action_prefs.pr_create` pref (via the resolver) + the 2 s LLM timeout +
/// the deterministic fallback + the template fold + the footer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrComposeContext {
    /// The workarea the PR is opened from (used for the footer deep link).
    pub workarea_id: WorkareaId,
    /// The repo the PR targets (the `action_prefs` source; per-repo — prefs do
    /// not bleed across repos in a multi-repo workarea).
    pub repository_id: RepositoryId,
    /// The workarea's composer name (`bach`, `mozart`, …) — the footer's
    /// `workarea <name>` segment and the deterministic title's lead.
    pub composer: String,
    /// The branch the PR opens from — the deterministic title's tail.
    pub branch: String,
    /// The last user message (the deterministic body source, `design/13 §3.4`).
    /// Empty when no chat message is available yet.
    pub last_user_message: String,
    /// A short summary of the change (recent commits / diff stat) — extra
    /// context the (P4) LLM prompt carries. May be empty.
    pub change_summary: String,
    /// The agent kind that produced the change (`claude|codex|gemini|…`) — the
    /// footer's `agent <kind>` segment. Empty ⇒ rendered as `unknown`.
    pub agent_kind: String,
}

/// The caller-override contract (`design/13 §3.4`, FROZEN): when the caller
/// (agent/UI) supplies **both** a non-empty title and body they are used
/// verbatim and composition is skipped. When either is empty, composition
/// fills them. Returns `Some((title, body))` to use verbatim, or `None` to
/// compose.
pub fn caller_override(title: &str, body: &str) -> Option<(String, String)> {
    if !title.trim().is_empty() && !body.trim().is_empty() {
        Some((title.to_string(), body.to_string()))
    } else {
        None
    }
}

/// Split a `title\n\nbody` blob (the `DeterministicOneShot` / LLM convention,
/// per Task 312's Handoff) on the **first** `"\n\n"`. The title is the part
/// before; the body is everything after. With no blank-line separator the
/// whole string is the title and the body is empty.
pub fn split_title_body(text: &str) -> (String, String) {
    match text.split_once("\n\n") {
        Some((title, body)) => (title.trim().to_string(), body.trim().to_string()),
        None => (text.trim().to_string(), String::new()),
    }
}

/// Build the deterministic title + body from the context (`design/13 §3.4`:
/// "deterministic title (composer + branch) and body (last user message)").
/// Pure + total — never empty. This is the LIVE Phase-3 output; it is made
/// genuinely useful (a readable title + the last user message as the body),
/// not a placeholder.
pub fn deterministic_title_body(ctx: &PrComposeContext) -> (String, String) {
    let title = deterministic_title(&ctx.composer, &ctx.branch);
    let body = {
        let msg = ctx.last_user_message.trim();
        if msg.is_empty() {
            "Opened by Concerto.".to_string()
        } else {
            msg.to_string()
        }
    };
    (title, body)
}

/// `<composer> · <branch>` style deterministic title (`design/13 §3.4`),
/// readably trimmed. Falls back to a non-empty default if both are blank.
fn deterministic_title(composer: &str, branch: &str) -> String {
    let composer = composer.trim();
    let branch = branch.trim();
    match (composer.is_empty(), branch.is_empty()) {
        (false, false) => format!("{composer} · {branch}"),
        (false, true) => composer.to_string(),
        (true, false) => branch.to_string(),
        (true, true) => "Concerto pull request".to_string(),
    }
}

/// Fold the repo's `.github/pull_request_template.md` (when present) into the
/// composed body (`design/13 §3.4` step 3). Deterministic merge: the template
/// is placed first (it carries the project's required checklist/sections), then
/// the composed body, separated by a blank line. A blank template is ignored.
pub fn fold_template(template: Option<&str>, body: &str) -> String {
    match template.map(str::trim).filter(|t| !t.is_empty()) {
        Some(tpl) => {
            let body = body.trim();
            if body.is_empty() {
                tpl.to_string()
            } else {
                format!("{tpl}\n\n{body}")
            }
        }
        None => body.trim().to_string(),
    }
}

/// The Concerto footer (FROZEN format, `design/13 §3.4`): appended to **every**
/// PR body (composed or fallback). Carries the workarea name, the agent kind,
/// and a `concerto://workarea/<id>` deep link (`design/15 §3.8`).
pub fn footer(ctx: &PrComposeContext) -> String {
    let composer = {
        let c = ctx.composer.trim();
        if c.is_empty() {
            "workarea"
        } else {
            c
        }
    };
    let agent = {
        let a = ctx.agent_kind.trim();
        if a.is_empty() {
            "unknown"
        } else {
            a
        }
    };
    format!(
        "Created from Concerto · workarea `{composer}` · agent `{agent}`\nconcerto://workarea/{}",
        ctx.workarea_id.0
    )
}

/// Assemble the final body: fold the template, then append the footer. The
/// footer is separated from the body by a blank line and is **always** present.
pub fn assemble_body(
    ctx: &PrComposeContext,
    composed_body: &str,
    template: Option<&str>,
) -> String {
    let folded = fold_template(template, composed_body);
    let foot = footer(ctx);
    if folded.is_empty() {
        foot
    } else {
        format!("{folded}\n\n{foot}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> PrComposeContext {
        PrComposeContext {
            workarea_id: WorkareaId("wa-1".to_string()),
            repository_id: RepositoryId("repo-1".to_string()),
            composer: "bach".to_string(),
            branch: "concerto/bach".to_string(),
            last_user_message: "Add idempotency keys to the payments endpoint".to_string(),
            change_summary: String::new(),
            agent_kind: "claude".to_string(),
        }
    }

    #[test]
    fn caller_override_uses_both_or_composes() {
        assert_eq!(
            caller_override("T", "B"),
            Some(("T".to_string(), "B".to_string()))
        );
        // Either empty → compose.
        assert_eq!(caller_override("", "B"), None);
        assert_eq!(caller_override("T", ""), None);
        assert_eq!(caller_override("  ", "B"), None);
    }

    #[test]
    fn split_on_first_blank_line() {
        let (t, b) = split_title_body("Title here\n\nbody line 1\n\nbody line 2");
        assert_eq!(t, "Title here");
        assert_eq!(b, "body line 1\n\nbody line 2");
        // No separator → all title, empty body.
        let (t, b) = split_title_body("just a title");
        assert_eq!(t, "just a title");
        assert!(b.is_empty());
    }

    #[test]
    fn deterministic_title_is_composer_and_branch() {
        let (t, b) = deterministic_title_body(&ctx());
        assert_eq!(t, "bach · concerto/bach");
        assert_eq!(b, "Add idempotency keys to the payments endpoint");
    }

    #[test]
    fn deterministic_body_falls_back_when_no_message() {
        let mut c = ctx();
        c.last_user_message = "   ".to_string();
        let (_t, b) = deterministic_title_body(&c);
        assert_eq!(b, "Opened by Concerto.");
    }

    #[test]
    fn deterministic_title_handles_blanks() {
        assert_eq!(deterministic_title("", ""), "Concerto pull request");
        assert_eq!(deterministic_title("bach", ""), "bach");
        assert_eq!(deterministic_title("", "feat/x"), "feat/x");
    }

    #[test]
    fn template_folds_before_body() {
        let out = fold_template(Some("## Checklist\n- [ ] tests"), "Did the thing.");
        assert_eq!(out, "## Checklist\n- [ ] tests\n\nDid the thing.");
        // Blank template is ignored.
        assert_eq!(fold_template(Some("   "), "body"), "body");
        assert_eq!(fold_template(None, "body"), "body");
    }

    #[test]
    fn footer_format_is_frozen() {
        let f = footer(&ctx());
        assert_eq!(
            f,
            "Created from Concerto · workarea `bach` · agent `claude`\nconcerto://workarea/wa-1"
        );
    }

    #[test]
    fn assemble_appends_footer_and_template() {
        let out = assemble_body(&ctx(), "The body.", Some("## Template"));
        assert!(out.starts_with("## Template\n\nThe body."));
        assert!(out.ends_with("concerto://workarea/wa-1"));
        assert!(out.contains("Created from Concerto · workarea `bach` · agent `claude`"));
    }
}
