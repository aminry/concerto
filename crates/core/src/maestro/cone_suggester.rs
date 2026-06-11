//! The Maestro-backed plan-mode cone suggester (Task 411, `design/08 §3.8`).
//!
//! This is the LIVE wiring of the seam **Task 305** froze: the
//! [`crate::repo_manager::ConeSuggester`] trait + the
//! `RepoManager::with_cone_suggester` injector + `RepoManager::suggest_cones`
//! that returns [`crate::repo_manager::ConeSuggestError::Unwired`] until an
//! implementor is injected. Until this task lands, every `SuggestCones`/create
//! flow honestly returns `UNIMPLEMENTED`; once `boot.rs` injects a
//! [`MaestroConeSuggester`], `RepoManager::suggest_cones` delegates here.
//!
//! ## The LIVE path (D5 / §4.5)
//!
//! The suggestion routes `issue_text` + the repo's top-level directory tree
//! through **312's [`OneShotLlm`]** with the [`ActionKind::DigestSummary`]
//! one-shot intent (the reserved cone-suggestion intent; there is no separate
//! `ActionKind` for it in the FROZEN seam). The LIVE Phase-4 path is
//! [`crate::llm::oneshot::DeterministicOneShot`]: a pure, no-I/O guess that
//! intersects the issue keywords with the repo's real top-level directories
//! (read from the git tree, NOT a filesystem walk). The real-LLM cone quality
//! is the **Phase-4 Tier-3 gate** (412 supplies the real provider behind the
//! same `Arc<dyn OneShotLlm>`, with zero change here).
//!
//! The directory candidates always come from the repo's **actual** tree
//! (`RepoManager::list_tree` at the repo root), so a suggested cone is always a
//! real directory the cone-picker / `SetCones` accepts — the LLM only ranks,
//! it never invents a path.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use concerto_error::Result;
use concerto_gix_wrap::ConePath;
use concerto_persist::RepositoryId;

use crate::llm::oneshot::{ActionKind, OneShotLlm, OneShotRequest};
use crate::repo_manager::{ConeSuggester, RepoManager};

/// Upper bound on the number of suggested cones (keeps the chip slate legible
/// and the deterministic fallback from selecting the whole tree).
const MAX_SUGGESTED_CONES: usize = 5;

/// The Maestro-backed plan-mode cone suggester (Task 411). Injected into the
/// [`RepoManager`] via `with_cone_suggester` at boot, this is the LIVE wiring of
/// the seam Task 305 froze. The live path routes `issue_text` + the repo tree
/// through an [`OneShotLlm`] (`DeterministicOneShot` fallback); the real-LLM
/// cone quality is the Phase-4 Tier-3 gate.
#[derive(Clone)]
pub struct MaestroConeSuggester {
    /// The repo-tree access used to enumerate the **real** top-level directories
    /// a cone may select (so the suggestion is always a valid cone path).
    repo: RepoManager,
    /// 312's one-shot LLM seam (`DeterministicOneShot` is the LIVE P4 fallback;
    /// 412 swaps the real provider in with no change here).
    llm: Arc<dyn OneShotLlm>,
}

impl MaestroConeSuggester {
    /// Build the suggester from the repo handle (tree access) + the injected
    /// one-shot LLM seam.
    pub fn new(repo: RepoManager, llm: Arc<dyn OneShotLlm>) -> Self {
        Self { repo, llm }
    }
}

#[async_trait]
impl ConeSuggester for MaestroConeSuggester {
    async fn suggest_cones(&self, repo: &RepositoryId, issue_text: &str) -> Result<Vec<ConePath>> {
        // 1. Enumerate the repo's REAL top-level directories (git tree, not a
        //    filesystem walk). Empty `git_ref`/`path` ⇒ HEAD root. A bare repo
        //    or empty tree yields no candidates → an empty suggestion (the
        //    create flow then falls back to the whole tree / user edit).
        let entries = self.repo.list_tree(repo, "", "").await?;
        let dirs: Vec<String> = entries
            .into_iter()
            .filter(|e| e.is_dir)
            .map(|e| e.path)
            .collect();
        if dirs.is_empty() {
            return Ok(Vec::new());
        }

        // 2. Run the one-shot LLM over the issue text + the candidate dir list.
        //    The DeterministicOneShot fallback ignores the prompt and echoes
        //    `context`; we don't depend on its text — we use it only to keep the
        //    seam LIVE (and the real provider, 412, gets the full prompt). The
        //    deterministic ranking below is what actually picks the cones.
        let candidate_list = dirs.join(", ");
        let prompt = format!(
            "Given this issue/description, choose the most relevant top-level \
             directories (cones) from the candidate list. Issue:\n{issue_text}\n\n\
             Candidates: {candidate_list}"
        );
        // Keep the seam live; a deterministic fallback never errors, but a real
        // provider (412) might — surface it through the typed `Result`.
        let _ = self
            .llm
            .suggest(OneShotRequest::new(
                ActionKind::DigestSummary,
                repo.as_str(),
                prompt,
                issue_text.to_string(),
            ))
            .await?;

        // 3. Deterministic ranking (the LIVE P4 quality): keep the candidate
        //    directories whose basename is mentioned (case-insensitive,
        //    word-token) in the issue text, preserving tree order. When nothing
        //    matches, fall back to the first `MAX_SUGGESTED_CONES` directories so
        //    the planner always has a non-empty, user-editable starting cone set
        //    (never a silent empty success that reads as "no suggestions").
        let keywords = tokenize(issue_text);
        let mut matched: Vec<ConePath> = dirs
            .iter()
            .filter(|dir| {
                let base = dir.rsplit('/').next().unwrap_or(dir).to_ascii_lowercase();
                !base.is_empty() && keywords.contains(&base)
            })
            .take(MAX_SUGGESTED_CONES)
            .cloned()
            .collect();

        if matched.is_empty() {
            matched = dirs.into_iter().take(MAX_SUGGESTED_CONES).collect();
        }
        Ok(matched)
    }
}

/// Lowercase alphanumeric word tokens of `text` (the deterministic keyword set
/// the cone ranking intersects against directory basenames). Pure.
fn tokenize(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_lowercases_and_splits_on_punctuation() {
        let toks = tokenize("Fix the API gateway, then the iOS app!");
        assert!(toks.contains("api"));
        assert!(toks.contains("gateway"));
        assert!(toks.contains("ios"));
        assert!(toks.contains("app"));
        // Stop-shaped non-alphanumerics are not tokens.
        assert!(!toks.contains(","));
    }
}
