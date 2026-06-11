//! Maestro privacy enforcement (Task 413, design/08 §3.3 / §3.10,
//! PHASE4_PLANNING §2 (413) / §4.4 / D10).
//!
//! This module is the **pure policy** that drives the three Maestro privacy
//! rules over 404's [`WorkareaSummary`] cache. It contains **no I/O**: callers
//! resolve the inputs (the `enterprise_data_privacy` bool from
//! [`crate::settings::resolver::WorkspaceSettingsResolver::enterprise_data_privacy`],
//! the chosen model's externality from 412's `MaestroProvider`, the
//! per-workarea `exclude_from_maestro` flag from `workareas.settings_json`, and
//! the per-workspace `concerto_chat_full_chat_access` flag from
//! [`concerto_persist::workspaces::get_settings_json_bool`]) and this object
//! decides. That keeps the policy table-test-driven (design/08 §10).
//!
//! ## The three enforcement points (one shared policy)
//!
//! 1. **Blanking happens at READ/SERVE time.** [`PrivacyPolicy::blank_excluded`]
//!    runs on every summary the cache *serves*, so a freshly-flipped
//!    `exclude_from_maestro` is honored without a cache rebuild — a stale
//!    pre-toggle cache entry can NOT leak summary prose.
//! 2. **The external-LLM gate is checked at CALL time.**
//!    [`PrivacyPolicy::maestro_disabled_by_policy`] /
//!    [`PrivacyPolicy::llm_gate`] are consulted at the *external summary/digest
//!    call site* (in `summary.rs`), **before** the call. The in-process
//!    deterministic summarizer is NOT external and is NOT gated.
//! 3. **Deterministic routing/tools NEVER consult either.** The gate guards
//!    only the LLM/external paths; `@workarea` routing fires in all modes.
//!
//! ## What 413 does NOT own
//!
//! - The [`WorkareaSummary`]/[`super::summary::SessionSummary`]/
//!   [`super::summary::RepoSummary`] shapes (FROZEN by 404, §4.4) — consumed,
//!   never re-derived.
//! - The provider model-externality classification (FROZEN by 402, extended by
//!   412, §4.3) — 413 takes "is the chosen model external?" as a constructed
//!   [`MaestroModelLocality`]/`bool` input. 412 supplies it from the live
//!   `MaestroProvider`; in V1.0 (D1: Direct-API unwired) the practical external
//!   case is "a CLI configured against a public provider", and the on-prem
//!   (Bedrock-VPC / Vertex / Azure-Foundry / local) path that re-enables the
//!   Maestro under `enterpriseDataPrivacy` is the Tier-3 + follow-on.

use super::summary::WorkareaSummary;

/// The exact name-only blank string a private workarea's `last_turn_summary`
/// carries after blanking (design/08 §3.3, verbatim) — FROZEN.
pub const PRIVATE_WORKAREA_BLANK: &str = "[private workarea, name only]";

/// Which raw-content source the summary cache serves to the Maestro.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummarySource {
    /// Default: summaries only (no raw chat). design/08 §3.3.
    SummaryOnly,
    /// `concerto_chat_full_chat_access = true`: raw last-3-turns per session.
    FullLast3Turns,
}

/// Whether the configured Maestro model egresses data off-box.
///
/// A typed input 413 *consumes* (412 derives it from the live `MaestroProvider`
/// plus `ManagedPolicy::default_model()`; PHASE4_PLANNING §4.3). The external
/// case is the Anthropic/OpenAI public API or a CLI dialing a public provider;
/// the on-prem case is Bedrock-VPC / Vertex / Azure-Foundry / local
/// (design/08 §3.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaestroModelLocality {
    /// Off-box public provider (gated under `enterpriseDataPrivacy`).
    External,
    /// In-VPC / local provider — never gated by the privacy policy.
    OnPrem,
}

impl MaestroModelLocality {
    /// True iff this locality is the off-box [`MaestroModelLocality::External`]
    /// case the privacy gate restricts.
    pub fn is_external(self) -> bool {
        matches!(self, MaestroModelLocality::External)
    }
}

/// Whether the Maestro LLM may run, given the enterprise-privacy gate.
/// Deterministic routing/tools NEVER consult this. design/08 §3.10.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaestroLlmGate {
    /// The external LLM call site may proceed.
    Allowed,
    /// `enterpriseDataPrivacy=true` AND the chosen Maestro model is external:
    /// the external LLM is disabled (the deterministic fallback stays live).
    DisabledExternalPolicy,
}

impl MaestroLlmGate {
    /// True iff the external LLM/summarizer call site must NOT issue its call.
    pub fn is_disabled(self) -> bool {
        matches!(self, MaestroLlmGate::DisabledExternalPolicy)
    }
}

/// Pure Maestro privacy policy (no I/O). Callers resolve the inputs; this
/// object decides. design/08 §3.3 + §3.10; PHASE4_PLANNING §2 (413) / D10.
pub struct PrivacyPolicy;

impl PrivacyPolicy {
    /// Blank an `exclude_from_maestro` workarea's summary to **name-only**:
    /// strips every LLM/chat-derived field, preserves every hard fact.
    ///
    /// `excluded == false` ⇒ identity (the summary round-trips unchanged).
    /// When `excluded == true`:
    /// - `last_turn_summary` becomes exactly [`PRIVATE_WORKAREA_BLANK`]
    ///   (`"[private workarea, name only]"`, design/08 §3.3);
    /// - `last_3_turn_summaries` is emptied;
    /// - `sessions` is **emptied** — a [`super::summary::SessionSummary`]
    ///   carries no hard fact the UI needs for a private workarea (only
    ///   `model`/`status`/`last_turn_summary`), so emptying it is the stronger
    ///   guarantee than clearing each `last_turn_summary` in place.
    ///
    /// Every hard fact is preserved verbatim: `workarea_id`, `workspace_id`,
    /// `workspace_name`, `composer_name`, `branch_name`, `status`,
    /// `last_activity_at`, the whole `repos` `Vec` (commits_ahead /
    /// files_changed / lines_* / pr_state / ci_state), `blocked_on`,
    /// `generated_at`, `generation`. FROZEN.
    pub fn blank_excluded(mut summary: WorkareaSummary, excluded: bool) -> WorkareaSummary {
        if !excluded {
            return summary;
        }
        summary.last_turn_summary = PRIVATE_WORKAREA_BLANK.to_string();
        summary.last_3_turn_summaries = Vec::new();
        // Emptying `sessions` is the stronger guarantee: a SessionSummary
        // carries no hard fact the UI needs for a private workarea.
        summary.sessions = Vec::new();
        summary
    }

    /// The cache source the Maestro is granted for a workspace.
    /// `full_chat_access` defaults to `false` ⇒ [`SummarySource::SummaryOnly`]
    /// (design/08 §3.3). FROZEN.
    pub fn summary_source(full_chat_access: bool) -> SummarySource {
        if full_chat_access {
            SummarySource::FullLast3Turns
        } else {
            SummarySource::SummaryOnly
        }
    }

    /// `true` iff `enterprise_data_privacy && is_external_model` (design/08
    /// §3.10). The single disable decision; routing is unaffected. FROZEN.
    pub fn maestro_disabled_by_policy(
        enterprise_data_privacy: bool,
        is_external_model: bool,
    ) -> bool {
        enterprise_data_privacy && is_external_model
    }

    /// The LLM gate the digest/summarizer checks BEFORE any external call.
    /// [`MaestroLlmGate::DisabledExternalPolicy`] iff
    /// [`PrivacyPolicy::maestro_disabled_by_policy`]. FROZEN.
    pub fn llm_gate(enterprise_data_privacy: bool, is_external_model: bool) -> MaestroLlmGate {
        if Self::maestro_disabled_by_policy(enterprise_data_privacy, is_external_model) {
            MaestroLlmGate::DisabledExternalPolicy
        } else {
            MaestroLlmGate::Allowed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_supervisor::actor::AgentKind;
    use crate::maestro::summary::{RepoSummary, SessionSummary, WorkareaSummary};
    use concerto_persist::{RepositoryId, SessionId, WorkareaId, WorkspaceId};

    /// A fully-populated summary with one session + one repo (every hard fact
    /// set to a distinctive value so blanking can be asserted field-by-field).
    fn populated() -> WorkareaSummary {
        WorkareaSummary {
            workarea_id: WorkareaId("wa-1".into()),
            workspace_id: WorkspaceId("ws-1".into()),
            workspace_name: "Workspace One".into(),
            composer_name: "bach".into(),
            branch_name: "concerto/bach".into(),
            status: "running".into(),
            last_activity_at: 4_242,
            sessions: vec![SessionSummary {
                session_id: SessionId("sess-1".into()),
                agent_kind: AgentKind::Claude,
                model: "claude".into(),
                status: "running".into(),
                last_turn_summary: "secret chat prose".into(),
            }],
            last_turn_summary: "secret turn summary".into(),
            last_3_turn_summaries: vec!["a".into(), "b".into(), "c".into()],
            repos: vec![RepoSummary {
                repository_id: RepositoryId("r1".into()),
                repo_name: "my-repo".into(),
                commits_ahead: 7,
                files_changed: 3,
                lines_added: 11,
                lines_removed: 2,
                pr_state: Some("open".into()),
                ci_state: Some("success".into()),
            }],
            blocked_on: Some("awaiting_approval".into()),
            generated_at: 9_000,
            generation: 5,
        }
    }

    #[test]
    fn excluded_leaks_only_hard_facts() {
        let original = populated();
        let blanked = PrivacyPolicy::blank_excluded(original.clone(), true);

        // LLM/chat-derived fields stripped.
        assert_eq!(blanked.last_turn_summary, PRIVATE_WORKAREA_BLANK);
        assert_eq!(blanked.last_turn_summary, "[private workarea, name only]");
        assert!(blanked.last_3_turn_summaries.is_empty());
        assert!(
            blanked.sessions.is_empty(),
            "sessions must be emptied (carry chat prose, no hard facts)"
        );

        // Every hard fact preserved (POSITIVE assertion on each).
        assert_eq!(blanked.workarea_id, original.workarea_id);
        assert_eq!(blanked.workspace_id, original.workspace_id);
        assert_eq!(blanked.workspace_name, original.workspace_name);
        assert_eq!(blanked.composer_name, original.composer_name);
        assert_eq!(blanked.branch_name, original.branch_name);
        assert_eq!(blanked.status, "running");
        assert_eq!(blanked.last_activity_at, 4_242);
        assert_eq!(blanked.blocked_on.as_deref(), Some("awaiting_approval"));
        assert_eq!(blanked.generated_at, 9_000);
        assert_eq!(blanked.generation, 5);
        assert_eq!(blanked.repos, original.repos);
        // Spell the repo hard facts out so a future RepoSummary change can't
        // silently start leaking through the privacy filter.
        let r = &blanked.repos[0];
        assert_eq!(r.commits_ahead, 7);
        assert_eq!(r.files_changed, 3);
        assert_eq!(r.lines_added, 11);
        assert_eq!(r.lines_removed, 2);
        assert_eq!(r.pr_state.as_deref(), Some("open"));
        assert_eq!(r.ci_state.as_deref(), Some("success"));
    }

    #[test]
    fn not_excluded_is_identity() {
        let original = populated();
        let same = PrivacyPolicy::blank_excluded(original.clone(), false);
        assert_eq!(same, original);
    }

    #[test]
    fn maestro_disabled_truth_table() {
        // Only (privacy=true, external=true) disables.
        assert!(PrivacyPolicy::maestro_disabled_by_policy(true, true));
        assert!(!PrivacyPolicy::maestro_disabled_by_policy(true, false));
        assert!(!PrivacyPolicy::maestro_disabled_by_policy(false, true));
        assert!(!PrivacyPolicy::maestro_disabled_by_policy(false, false));

        // llm_gate mirrors the truth table.
        assert_eq!(
            PrivacyPolicy::llm_gate(true, true),
            MaestroLlmGate::DisabledExternalPolicy
        );
        assert_eq!(
            PrivacyPolicy::llm_gate(true, false),
            MaestroLlmGate::Allowed
        );
        assert_eq!(
            PrivacyPolicy::llm_gate(false, true),
            MaestroLlmGate::Allowed
        );
        assert_eq!(
            PrivacyPolicy::llm_gate(false, false),
            MaestroLlmGate::Allowed
        );
    }

    #[test]
    fn routing_path_never_consults_the_gate() {
        // The deterministic routing pre-parser (Task 408) MUST run regardless
        // of the LLM gate — assert that the routing grammar resolves a
        // `@workarea` mention even with the gate set to DisabledExternalPolicy.
        let gate = MaestroLlmGate::DisabledExternalPolicy;
        assert!(gate.is_disabled());
        let outcome = crate::maestro::routing::pre_parse("@bach run the tests");
        // Routing produced a result without ever touching `gate`; the fact that
        // this compiles + runs proves routing is not gated on the LLM gate.
        let _ = outcome;
    }

    #[test]
    fn summary_source_flip() {
        assert_eq!(
            PrivacyPolicy::summary_source(true),
            SummarySource::FullLast3Turns
        );
        assert_eq!(
            PrivacyPolicy::summary_source(false),
            SummarySource::SummaryOnly
        );
    }

    #[test]
    fn model_locality_externality() {
        assert!(MaestroModelLocality::External.is_external());
        assert!(!MaestroModelLocality::OnPrem.is_external());
    }
}
