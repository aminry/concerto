//! The one-shot LLM seam (Task 312, FROZEN per `PHASE3_PLANNING §4.4`).
//!
//! This module owns the seam every action-scoped one-shot LLM call routes
//! through. Per **D1** (`PHASE3_PLANNING §1`) the **deterministic fallback is
//! the LIVE path in Phase 3**; the pluggable real-LLM provider is an *unwired
//! trait seam* supplied in Phase 4 (Task 412) and judged at that gate.
//!
//! Three pieces, all frozen here so Task 321 (PR title/body) and Task 412 (the
//! real provider) build on them without re-locking:
//!
//! - [`OneShotLlm`] — the async trait a one-shot LLM call implements. The
//!   real provider (412) implements it; the manager defaults to
//!   [`DeterministicOneShot`].
//! - [`OneShotRequest`] — the input: the [`ActionKind`], the repo id, and the
//!   composed prompt (after [`compose_action_prompt`]) plus any context the
//!   deterministic impl needs (the first-message text for branch rename; the
//!   diff/commit context for PR — Task 321 fills that).
//! - [`compose_action_prompt`] — reads Task 310's resolved `action_prefs.<action>`
//!   and injects it per the `design/04 §3.13` injection table.
//!
//! [`DeterministicOneShot`] is the LIVE Phase-3 implementation: pure, no I/O,
//! no network — fully CI-provable. For `BranchRename` it produces a kebab-case
//! slug from the prompt; for `PrCreate` a template title/body (Task 321's
//! fallback).

use async_trait::async_trait;
use concerto_error::Result;

use crate::settings::{Resolved, SettingsSource};

/// The seven action-scoped one-shot actions (`design/04 §3.13`). Frozen —
/// new actions append-only. Each maps to a per-repo `action_prefs.<action>`
/// pref the resolver (Task 310) supplies and [`compose_action_prompt`] injects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionKind {
    /// User clicks "Review" on a diff (`design/15 §3.5`).
    CodeReview,
    /// User clicks "Create PR" (`design/13`). Task 321 consumes the
    /// [`DeterministicOneShot`] template title/body for this action.
    PrCreate,
    /// User clicks "Fix errors" (`design/15 §3.15`).
    ErrorFix,
    /// Agent invoked to resolve a merge conflict (`design/03 §3.9`).
    ConflictResolve,
    /// One-shot agent call for branch-name suggestion (`design/03 §3.6`,
    /// `design/04 §2` V1.0). The action Task 312 exercises.
    BranchRename,
    /// User clicks "Commit" with an agent-drafted message.
    CommitMessage,
    /// Maestro generates a workspace digest (`design/08 §3.6`).
    DigestSummary,
}

impl ActionKind {
    /// The stable wire/pref-key string — exactly the `action_prefs.<action>`
    /// key the resolver (Task 310) and `design/04 §3.13` use. Frozen.
    pub fn as_str(self) -> &'static str {
        match self {
            ActionKind::CodeReview => "code_review",
            ActionKind::PrCreate => "pr_create",
            ActionKind::ErrorFix => "error_fix",
            ActionKind::ConflictResolve => "conflict_resolve",
            ActionKind::BranchRename => "branch_rename",
            ActionKind::CommitMessage => "commit_message",
            ActionKind::DigestSummary => "digest_summary",
        }
    }
}

impl std::fmt::Display for ActionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The input to [`OneShotLlm::suggest`] (FROZEN). Carries everything a one-shot
/// call needs: the [`ActionKind`], the repo the action runs against, the
/// composed prompt (after [`compose_action_prompt`] has injected the resolved
/// pref), and the raw `context` the deterministic impl slugs/templates from
/// (the first-message text for branch rename; the diff/commit summary for PR —
/// Task 321 fills that). Fields are public so the real provider (412) and Task
/// 321 construct requests directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OneShotRequest {
    /// Which action this call serves.
    pub action: ActionKind,
    /// The repository the action runs against (the pref source). String form
    /// to stay decoupled from the persist `RepositoryId` newtype at this seam.
    pub repo_id: String,
    /// The composed prompt — the result of [`compose_action_prompt`] (the
    /// injected pref + the base prompt). The real provider sends this to the
    /// LLM; the deterministic impl ignores it in favour of `context`.
    pub prompt: String,
    /// The raw context the deterministic impl derives its answer from (the
    /// first user message for branch rename; the diff/commit summary for PR).
    pub context: String,
}

impl OneShotRequest {
    /// Convenience constructor.
    pub fn new(
        action: ActionKind,
        repo_id: impl Into<String>,
        prompt: impl Into<String>,
        context: impl Into<String>,
    ) -> Self {
        Self {
            action,
            repo_id: repo_id.into(),
            prompt: prompt.into(),
            context: context.into(),
        }
    }
}

/// The one-shot LLM seam (FROZEN, `PHASE3_PLANNING §4.4`). The real pluggable
/// provider (Task 412) implements this; the live Phase-3 path is
/// [`DeterministicOneShot`]. Task 321 reuses the trait verbatim for PR
/// title/body.
#[async_trait]
pub trait OneShotLlm: Send + Sync + 'static {
    /// Produce a single short string for `req.action` from `req`. For
    /// [`ActionKind::BranchRename`] this is the proposed branch name; for
    /// [`ActionKind::PrCreate`] a title/body (the exact composition is the
    /// caller's contract — Task 321 splits title from body).
    async fn suggest(&self, req: OneShotRequest) -> Result<String>;
}

/// The LIVE Phase-3 one-shot impl (D1): pure, deterministic, no network.
///
/// - [`ActionKind::BranchRename`] → a kebab-case slug from `req.context`
///   (lowercase, non-alphanumerics → dashes, collapsed, bounded length),
///   honoring a `branch_rename` pref that asks for a ticket prefix
///   (best-effort: a `PROJ-123`-shaped token already in the prompt is kept as
///   a leading segment).
/// - [`ActionKind::PrCreate`] → a `title\n\nbody` template so Task 321 has a
///   working fallback.
/// - every other action → a trimmed echo of the context (a usable, if plain,
///   fallback until 412 wires the real provider).
#[derive(Debug, Clone, Copy, Default)]
pub struct DeterministicOneShot;

/// Upper bound on the generated branch slug length (excluding any ticket
/// prefix). Keeps refs short enough for terminal/UI display and well under
/// git's ref-length limits.
const MAX_SLUG_LEN: usize = 50;

#[async_trait]
impl OneShotLlm for DeterministicOneShot {
    async fn suggest(&self, req: OneShotRequest) -> Result<String> {
        Ok(match req.action {
            ActionKind::BranchRename => branch_slug_from_prompt(&req.context, &req.prompt),
            ActionKind::PrCreate => pr_template(&req.context),
            // The remaining actions are PR/review-adjacent system-message
            // addenda the real provider (412) will own; the deterministic
            // fallback returns a trimmed, single-line echo of the context so
            // the seam is never empty.
            _ => {
                let echo: String = req.context.split_whitespace().collect::<Vec<_>>().join(" ");
                if echo.is_empty() {
                    req.action.as_str().to_string()
                } else {
                    echo
                }
            }
        })
    }
}

/// Compose the action-scoped prompt (FROZEN, `design/04 §3.13`).
///
/// Reads Task 310's resolved `action_prefs.<action>` — passed in as
/// `pref` (the [`Resolved`]`<Option<String>>` the resolver's `action_pref`
/// getter returns) so this helper never re-resolves — and injects it ahead of
/// `context` per the §3.13 injection table. When no pref is set (or it is
/// blank) the base `context` is returned unchanged.
///
/// The injection shape is `"<pref>\n\n<context>"` — the pref is a
/// system-message addendum prepended to the action's base prompt, matching the
/// §3.13 "Prepended to / Added to the … prompt" column. Task 321 reuses this
/// for `pr_create` with no change.
pub fn compose_action_prompt(
    action: ActionKind,
    pref: &Resolved<Option<String>>,
    context: &str,
) -> String {
    match pref
        .value
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        Some(p) => {
            // Tag the addendum with the action so a multi-action transcript is
            // legible, matching the §3.13 "per-action, travels only when that
            // action runs" intent.
            format!("[{} preference] {p}\n\n{context}", action.as_str())
        }
        None => context.to_string(),
    }
}

/// Best-effort: derive a leading ticket prefix from `prompt`/`pref` when the
/// `branch_rename` pref asks for one (e.g. "kebab-case with the Linear ticket
/// prefix when one exists"). Looks for a `LETTERS-DIGITS` token (Linear/Jira
/// shape) in the prompt; returns it lowercased, or `None`.
fn extract_ticket_prefix(prompt: &str, pref: &Resolved<Option<String>>) -> Option<String> {
    let wants_ticket = pref
        .value
        .as_deref()
        .map(|p| {
            let p = p.to_lowercase();
            p.contains("ticket") || p.contains("linear") || p.contains("jira")
        })
        .unwrap_or(false);
    if !wants_ticket {
        return None;
    }
    // Scan whitespace/punctuation-delimited tokens for `ABC-123`.
    for raw in prompt.split(|c: char| c.is_whitespace() || matches!(c, ':' | ',' | ';' | '(' | ')'))
    {
        let tok = raw.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
        if let Some((alpha, digits)) = tok.split_once('-') {
            if !alpha.is_empty()
                && alpha.chars().all(|c| c.is_ascii_alphabetic())
                && !digits.is_empty()
                && digits.chars().all(|c| c.is_ascii_digit())
            {
                return Some(tok.to_ascii_lowercase());
            }
        }
    }
    None
}

/// Produce a kebab-case branch slug from `context` (the first user message),
/// honoring a ticket-prefix pref carried in `prompt`. Deterministic + pure.
///
/// The `prompt` carries the (already-composed) pref text so we can detect a
/// ticket-prefix request; the slug itself comes from `context`.
fn branch_slug_from_prompt(context: &str, prompt: &str) -> String {
    // Synthesize the pref view back from the composed prompt only to decide on
    // the ticket prefix — the prompt already has the pref prepended by
    // `compose_action_prompt`. Treat the whole prompt as the haystack for the
    // ticket token + the pref-keyword check.
    let pref = Resolved {
        value: Some(prompt.to_string()),
        source: SettingsSource::Default,
    };
    let ticket = extract_ticket_prefix(prompt, &pref);

    let slug = slugify(context, MAX_SLUG_LEN);
    let body = if slug.is_empty() {
        "work".to_string()
    } else {
        slug
    };
    match ticket {
        Some(t) => format!("{t}-{body}"),
        None => body,
    }
}

/// Lowercase, replace every run of non-`[a-z0-9]` with a single `-`, trim
/// leading/trailing `-`, and cap at `max_len` (on a word boundary where
/// possible). Pure + deterministic.
fn slugify(input: &str, max_len: usize) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_dash = true; // suppress a leading dash
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    // Trim a trailing dash.
    while out.ends_with('-') {
        out.pop();
    }
    if out.len() <= max_len {
        return out;
    }
    // Truncate at the last dash before `max_len` so we never cut a word.
    let cut = out[..max_len].rfind('-').unwrap_or(max_len);
    let truncated = &out[..cut];
    truncated.trim_end_matches('-').to_string()
}

/// A template PR `title\n\nbody` from `context` (Task 321's deterministic
/// fallback). The first non-empty line of the context becomes the title (capped
/// + cleaned); the rest is echoed as the body. Never empty.
fn pr_template(context: &str) -> String {
    let first = context
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let title = if first.is_empty() {
        "Update".to_string()
    } else {
        // Cap the title at a readable length on a word boundary.
        let t = first;
        if t.len() <= 72 {
            t.to_string()
        } else {
            let cut = t[..72].rfind(' ').unwrap_or(72);
            t[..cut].trim_end().to_string()
        }
    };
    let body = context.trim();
    if body.is_empty() || body == title {
        format!("{title}\n\nOpened by Concerto.")
    } else {
        format!("{title}\n\n{body}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pref(s: Option<&str>) -> Resolved<Option<String>> {
        Resolved {
            value: s.map(str::to_string),
            source: if s.is_some() {
                SettingsSource::LocalDb
            } else {
                SettingsSource::Default
            },
        }
    }

    #[test]
    fn slugify_is_kebab_case_and_bounded() {
        assert_eq!(
            slugify("Add Idempotency Keys!!", 50),
            "add-idempotency-keys"
        );
        assert_eq!(slugify("   leading & trailing   ", 50), "leading-trailing");
        assert_eq!(
            slugify("UPPER_snake.dot/slash", 50),
            "upper-snake-dot-slash"
        );
        // Bounded + word-boundary truncation.
        let long = slugify(
            "this is a very long prompt that should be truncated at a word boundary not mid word",
            30,
        );
        assert!(long.len() <= 30, "len={}", long.len());
        assert!(!long.ends_with('-'));
        assert!(!long.contains("--"));
    }

    #[tokio::test]
    async fn deterministic_branch_slug_is_stable_and_kebab() {
        let llm = DeterministicOneShot;
        let prompt = "Add idempotency keys to the payments endpoint";
        let a = llm
            .suggest(OneShotRequest::new(
                ActionKind::BranchRename,
                "repo-1",
                prompt,
                prompt,
            ))
            .await
            .unwrap();
        let b = llm
            .suggest(OneShotRequest::new(
                ActionKind::BranchRename,
                "repo-1",
                prompt,
                prompt,
            ))
            .await
            .unwrap();
        assert_eq!(a, b, "deterministic");
        assert_eq!(a, "add-idempotency-keys-to-the-payments-endpoint");
        assert!(a
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
    }

    #[tokio::test]
    async fn branch_slug_honors_ticket_prefix_pref() {
        let llm = DeterministicOneShot;
        let p = pref(Some(
            "kebab-case with the Linear ticket prefix when one exists.",
        ));
        let context = "Fix the flaky retry in CON-451 checkout flow";
        let prompt = compose_action_prompt(ActionKind::BranchRename, &p, context);
        let name = llm
            .suggest(OneShotRequest::new(
                ActionKind::BranchRename,
                "repo-1",
                prompt,
                context,
            ))
            .await
            .unwrap();
        assert!(name.starts_with("con-451-"), "got {name}");
    }

    #[test]
    fn compose_injects_pref_when_present() {
        let p = pref(Some(
            "kebab-case with the Linear ticket prefix when one exists.",
        ));
        let composed = compose_action_prompt(ActionKind::BranchRename, &p, "make it faster");
        assert!(composed.contains("[branch_rename preference]"));
        assert!(composed.contains("Linear ticket prefix"));
        assert!(composed.ends_with("make it faster"));
    }

    #[test]
    fn compose_passes_context_through_when_no_pref() {
        let p = pref(None);
        let composed = compose_action_prompt(ActionKind::PrCreate, &p, "the base prompt");
        assert_eq!(composed, "the base prompt");
        // A blank pref is treated as no pref.
        let blank = pref(Some("   "));
        assert_eq!(
            compose_action_prompt(ActionKind::PrCreate, &blank, "ctx"),
            "ctx"
        );
    }

    #[tokio::test]
    async fn pr_create_template_is_nonempty() {
        let llm = DeterministicOneShot;
        let out = llm
            .suggest(OneShotRequest::new(
                ActionKind::PrCreate,
                "repo-1",
                "compose a PR",
                "Add caching layer\n\nSpeeds up the hot path.",
            ))
            .await
            .unwrap();
        assert!(!out.trim().is_empty());
        let (title, body) = out.split_once("\n\n").expect("title/body split");
        assert_eq!(title, "Add caching layer");
        assert!(body.contains("Speeds up the hot path"));
    }

    #[test]
    fn action_kind_wire_strings_match_design_keys() {
        assert_eq!(ActionKind::CodeReview.as_str(), "code_review");
        assert_eq!(ActionKind::PrCreate.as_str(), "pr_create");
        assert_eq!(ActionKind::ErrorFix.as_str(), "error_fix");
        assert_eq!(ActionKind::ConflictResolve.as_str(), "conflict_resolve");
        assert_eq!(ActionKind::BranchRename.as_str(), "branch_rename");
        assert_eq!(ActionKind::CommitMessage.as_str(), "commit_message");
        assert_eq!(ActionKind::DigestSummary.as_str(), "digest_summary");
    }
}
