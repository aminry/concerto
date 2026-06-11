//! Per-workarea summary cache (Task 404, design/08 §3.3/§3.4/§6.2,
//! PHASE4_PLANNING §4.4).
//!
//! The Maestro reads a rolling **summary** of each workarea instead of the raw
//! chat (design/08 §3.3). This module owns that cache and FREEZES its shape.
//!
//! ## What is frozen here (§4.4, D9)
//!
//! - [`WorkareaSummary`] / [`SessionSummary`] / [`RepoSummary`] — the
//!   `design/08 §3.3` field set, but with **`i64` unix-ms** timestamps (NOT
//!   `Instant`, per D9 — wire/persistence-friendly, matches the codebase's
//!   `created_at`/`updated_at` columns) and `String`/`Option<String>` for the
//!   FSM/PR/CI state strings (kept as opaque strings so consumers 405/409/413
//!   read one shape and never re-derive a typed enum here).
//! - [`SummaryCache`] — the in-memory `HashMap<WorkareaId, WorkareaSummary>`
//!   with a clock seam, a `generation` counter, and the
//!   `get`/`refresh`/`is_stale`/`force_refresh_if_stale(60_000)` API.
//! - **The refresh contract** — refresh on `AgentEvent::TurnComplete` (per
//!   active session), on a `workarea.events` `status:<to>` transition, after
//!   10-min idle, and force-on-`GetDigest`-if-stale-60s.
//!
//! ## Agent-independence (D9)
//!
//! The cache is built from **existing** signals — it does NOT spawn or require a
//! Maestro agent (Task 402). It consumes the agent supervisor's per-session
//! `AgentEvent` broadcast ([`crate::agent_supervisor::actor::AgentSupervisorHandle::subscribe_events`]),
//! the `workarea.events` status feed, `gix-wrap` diffs + the new
//! [`concerto_gix_wrap::commits_ahead`] helper, `pull_requests.state`, and the
//! opaque `checks.<wa>.<repo>` frames. That is what lets 404 run in the same
//! wave as 402.
//!
//! ## Summarizer (§3.4, D5)
//!
//! The fallback summarizer REUSES [`crate::llm::oneshot::OneShotLlm`] with
//! [`crate::llm::oneshot::ActionKind::DigestSummary`] (FROZEN by Task 312 —
//! consumed, not re-locked, no new `ActionKind`). The LIVE Phase-4 impl is
//! [`crate::llm::oneshot::DeterministicOneShot`]; the real Haiku/Sonnet provider
//! is Task 412, judged at the Phase-4 Tier-3 gate. The agent's own end-of-turn
//! summary is preferred when present (it is free — §3.4).

use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use concerto_error::Result;
use concerto_persist::{RepositoryId, SessionId, WorkareaId, WorkspaceId};

use crate::agent_supervisor::actor::AgentKind;
use crate::llm::oneshot::{compose_action_prompt, ActionKind, OneShotLlm, OneShotRequest};
use crate::maestro::privacy::{MaestroLlmGate, PrivacyPolicy, SummarySource};
use crate::settings::{Resolved, SettingsSource};

/// Upper bound on a `last_turn_summary` (design/08 §3.3: "≤ 300 chars").
pub const MAX_TURN_SUMMARY_LEN: usize = 300;

/// The default staleness window the `GetDigest` force-refresh path uses
/// (design/08 §3.4: "force-refresh if stale > 60 s"). Milliseconds.
pub const GET_DIGEST_STALE_MS: i64 = 60_000;

/// The idle window after which the Concerto-side summarizer ensures freshness
/// (design/08 §3.4: "after 10 minutes of inactivity"). Milliseconds.
pub const IDLE_REFRESH_MS: i64 = 10 * 60 * 1_000;

// ===========================================================================
// FROZEN shapes (design/08 §3.3, PHASE4_PLANNING §4.4).
// ===========================================================================

/// Per-workarea rolling summary the Maestro reads instead of raw chat
/// (design/08 §3.3). Timestamps are `i64` unix-ms (D9), NOT `Instant`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkareaSummary {
    pub workarea_id: WorkareaId,
    pub workspace_id: WorkspaceId,
    pub workspace_name: String,
    pub composer_name: String,
    pub branch_name: String,
    /// The workarea FSM state string (`workspace_manager::workarea`).
    pub status: String,
    /// unix-ms.
    pub last_activity_at: i64,

    pub sessions: Vec<SessionSummary>,
    /// `<= 300 chars`; from the most-recently-active session.
    pub last_turn_summary: String,
    pub last_3_turn_summaries: Vec<String>,

    /// Hard facts (per repo in the workarea — no LLM).
    pub repos: Vec<RepoSummary>,

    /// `"awaiting_approval" | "test_failure" | "merge_conflict" | ...`
    pub blocked_on: Option<String>,

    /// unix-ms.
    pub generated_at: i64,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub session_id: SessionId,
    pub agent_kind: AgentKind,
    pub model: String,
    /// The session status string.
    pub status: String,
    pub last_turn_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSummary {
    pub repository_id: RepositoryId,
    pub repo_name: String,
    /// Via [`concerto_gix_wrap::commits_ahead`].
    pub commits_ahead: u32,
    /// `diff_to_main` `DiffPayload.files.len()`.
    pub files_changed: u32,
    pub lines_added: u32,
    pub lines_removed: u32,
    /// `pull_requests.state`: `open|closed|merged|draft`.
    pub pr_state: Option<String>,
    /// Parsed from the opaque `checks.<wa>.<repo>` frames; `None` when the
    /// frame shape is not yet stable (see [`parse_ci_state`]).
    pub ci_state: Option<String>,
}

// ===========================================================================
// Clock seam (synthetic-clock testability — design/08 §10).
// ===========================================================================

/// A monotone-ish wall-clock source returning **unix-ms**.
///
/// Injected so the 10-min-idle / 60-s-stale logic is exercised against a
/// synthetic clock in tests (D9 / design/08 §10 "synthetic clock"). The
/// production clock is [`SystemClock`]; tests use [`ManualClock`].
pub trait Clock: Send + Sync + 'static {
    /// Current time as unix epoch milliseconds.
    fn now_ms(&self) -> i64;
}

/// The production clock: the real wall clock as unix-ms.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// A test clock whose `now_ms` is set explicitly (interior-mutable so it can be
/// advanced behind a shared `&`).
#[derive(Debug, Default)]
pub struct ManualClock {
    now: std::sync::atomic::AtomicI64,
}

impl ManualClock {
    /// Construct a clock fixed at `now_ms`.
    pub fn new(now_ms: i64) -> Self {
        Self {
            now: std::sync::atomic::AtomicI64::new(now_ms),
        }
    }
    /// Move the clock forward by `delta_ms`.
    pub fn advance(&self, delta_ms: i64) {
        self.now
            .fetch_add(delta_ms, std::sync::atomic::Ordering::SeqCst);
    }
    /// Set the clock to an absolute `now_ms`.
    pub fn set(&self, now_ms: i64) {
        self.now.store(now_ms, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.now.load(std::sync::atomic::Ordering::SeqCst)
    }
}

// ===========================================================================
// The cache.
// ===========================================================================

/// In-memory per-workarea summary cache (design/08 §3.3, §6.2).
///
/// `HashMap<WorkareaId, WorkareaSummary>` + a monotonically-increasing
/// `generation` counter (bumped on every entry mutation so consumers can detect
/// staleness) + an injected [`Clock`]. **No migration** — the cache is in-memory
/// (PHASE4_PLANNING §3/§2-404); it is rebuilt on Core restart from the same
/// existing signals.
pub struct SummaryCache {
    entries: HashMap<WorkareaId, WorkareaSummary>,
    /// Process-wide generation counter; every mutation bumps it and stamps the
    /// touched entry's `generation`.
    generation: u64,
    clock: Box<dyn Clock>,
}

impl std::fmt::Debug for SummaryCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SummaryCache")
            .field("entries", &self.entries.len())
            .field("generation", &self.generation)
            .finish()
    }
}

impl Default for SummaryCache {
    fn default() -> Self {
        Self::new(Box::new(SystemClock))
    }
}

impl SummaryCache {
    /// Construct a cache with the given clock seam.
    pub fn new(clock: Box<dyn Clock>) -> Self {
        Self {
            entries: HashMap::new(),
            generation: 0,
            clock,
        }
    }

    /// Construct a cache with the production [`SystemClock`].
    pub fn with_system_clock() -> Self {
        Self::new(Box::new(SystemClock))
    }

    /// Current unix-ms from the injected clock.
    pub fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }

    /// Test-only: replace the clock with a [`ManualClock`] fixed at `now_ms`,
    /// so a test can advance synthetic time after seeding entries.
    #[cfg(test)]
    fn set_clock_for_test(&mut self, now_ms: i64) {
        self.clock = Box::new(ManualClock::new(now_ms));
    }

    /// Cheap clone of one entry for the read tool (Task 405's
    /// `get_workarea_summary`). `None` when the workarea is untracked.
    ///
    /// This is the **raw** entry — callers serving a summary to the Maestro
    /// must instead use [`SummaryCache::get_for_maestro`], which applies the
    /// read-time `exclude_from_maestro` privacy filter (Task 413).
    pub fn get(&self, wa: &WorkareaId) -> Option<WorkareaSummary> {
        self.entries.get(wa).cloned()
    }

    /// Serve one entry to the Maestro with the **read-time privacy filter**
    /// applied (Task 413, design/08 §3.3). When `excluded` (the workarea's
    /// `exclude_from_maestro` flag, resolved by the caller from
    /// `workareas.settings_json`), the returned summary is blanked to
    /// name-only via [`PrivacyPolicy::blank_excluded`] — every LLM/chat-derived
    /// field stripped, every git/PR/CI hard fact preserved.
    ///
    /// Blanking is applied at **serve time, not refresh time**: a freshly
    /// flipped `exclude_from_maestro` is honored on the next read without a
    /// cache rebuild, so a stale pre-toggle cache entry can NOT leak summary
    /// prose. `None` when the workarea is untracked.
    pub fn get_for_maestro(&self, wa: &WorkareaId, excluded: bool) -> Option<WorkareaSummary> {
        self.entries
            .get(wa)
            .cloned()
            .map(|s| PrivacyPolicy::blank_excluded(s, excluded))
    }

    /// Snapshot of every tracked summary (Task 409's `list_active_summaries`).
    ///
    /// **Raw** — the digest/summary serve path must blank `exclude_from_maestro`
    /// workareas via [`SummaryCache::list_for_maestro`].
    pub fn list(&self) -> Vec<WorkareaSummary> {
        self.entries.values().cloned().collect()
    }

    /// Snapshot of every tracked summary with the read-time privacy filter
    /// applied (Task 413). `is_excluded(&WorkareaId) -> bool` resolves each
    /// workarea's `exclude_from_maestro` flag (the caller reads it from
    /// `workareas.settings_json`); excluded workareas are blanked to name-only
    /// while their hard facts + name stay visible. Task 409's digest builds on
    /// this serve path.
    pub fn list_for_maestro(
        &self,
        mut is_excluded: impl FnMut(&WorkareaId) -> bool,
    ) -> Vec<WorkareaSummary> {
        self.entries
            .values()
            .map(|s| {
                let excluded = is_excluded(&s.workarea_id);
                PrivacyPolicy::blank_excluded(s.clone(), excluded)
            })
            .collect()
    }

    /// Number of tracked workareas.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when no workarea is tracked.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// True when the entry is missing OR older than `max_age_ms` against the
    /// injected clock. The `GetDigest` force-refresh predicate (design/08 §3.4,
    /// `max_age_ms = 60_000`). A missing entry is always "stale".
    pub fn is_stale(&self, wa: &WorkareaId, max_age_ms: i64) -> bool {
        match self.entries.get(wa) {
            None => true,
            Some(s) => self.clock.now_ms().saturating_sub(s.generated_at) > max_age_ms,
        }
    }

    /// True when the entry has been idle for at least [`IDLE_REFRESH_MS`]
    /// (10 min) — the cadence the Concerto-side summarizer uses to ensure
    /// freshness (design/08 §3.4). A missing entry is treated as not-idle (there
    /// is nothing to refresh).
    pub fn is_idle(&self, wa: &WorkareaId) -> bool {
        match self.entries.get(wa) {
            None => false,
            Some(s) => self.clock.now_ms().saturating_sub(s.last_activity_at) >= IDLE_REFRESH_MS,
        }
    }

    /// Insert or replace a whole workarea entry, stamping `generation` +
    /// `generated_at` from the cache's counter/clock. The caller builds the
    /// `WorkareaSummary` (hard facts + summaries); the cache owns the
    /// bookkeeping fields. Returns the new generation.
    pub fn upsert(&mut self, mut summary: WorkareaSummary) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        summary.generation = self.generation;
        summary.generated_at = self.clock.now_ms();
        let wa = summary.workarea_id.clone();
        self.entries.insert(wa, summary);
        self.generation
    }

    /// Rebuild one entry in place via a closure, bumping `generation` +
    /// `generated_at`. Returns the new generation, or `None` if the workarea is
    /// untracked. This is the generic refresh primitive the trigger-specific
    /// helpers below build on.
    pub fn refresh_workarea(
        &mut self,
        wa: &WorkareaId,
        f: impl FnOnce(&mut WorkareaSummary),
    ) -> Option<u64> {
        let now = self.clock.now_ms();
        self.generation = self.generation.wrapping_add(1);
        let gen = self.generation;
        let entry = self.entries.get_mut(wa)?;
        f(entry);
        entry.generation = gen;
        entry.generated_at = now;
        Some(gen)
    }

    // -----------------------------------------------------------------------
    // The refresh contract (the load-bearing deliverable, §4.4).
    // -----------------------------------------------------------------------

    /// Refresh trigger (a): an `AgentEvent::TurnComplete` arrived for `session`
    /// inside workarea `wa`. Updates that session's `last_turn_summary` + the
    /// owning workarea's `last_turn_summary`/`last_3_turn_summaries`, sets
    /// `last_activity_at = now`, and bumps `generation`. `summary` is the
    /// closing turn's summary (the agent's own end-of-turn summary when present
    /// — preferred per §3.4 — else the Concerto summarizer's output; see
    /// [`summarize_turn`]). Returns the new generation, or `None` if the
    /// workarea is untracked.
    pub fn on_turn_complete(
        &mut self,
        wa: &WorkareaId,
        session: &SessionId,
        summary: &str,
    ) -> Option<u64> {
        let now = self.clock.now_ms();
        let summary = truncate_turn_summary(summary);
        self.refresh_workarea(wa, |entry| {
            entry.last_activity_at = now;
            // Update the per-session entry inside `sessions` (design/08 §6.2.2).
            if let Some(s) = entry.sessions.iter_mut().find(|s| &s.session_id == session) {
                s.last_turn_summary = summary.clone();
            }
            // The workarea's most-recent summary is this turn's.
            entry.last_turn_summary = summary.clone();
            push_recent(&mut entry.last_3_turn_summaries, summary.clone());
        })
    }

    /// Refresh trigger (b): a `workarea.events` `status:<to>` transition.
    /// Rebuilds the entry's `status` (and clears/sets `blocked_on` derived from
    /// the new status). Sets `last_activity_at = now` and bumps `generation`.
    /// Returns the new generation, or `None` if the workarea is untracked.
    pub fn on_status_change(
        &mut self,
        wa: &WorkareaId,
        to_status: &str,
        blocked_on: Option<String>,
    ) -> Option<u64> {
        let now = self.clock.now_ms();
        let to_status = to_status.to_string();
        self.refresh_workarea(wa, |entry| {
            entry.last_activity_at = now;
            entry.status = to_status.clone();
            entry.blocked_on = blocked_on.clone();
        })
    }

    /// Refresh trigger (d): force a refresh ONLY when the entry is stale beyond
    /// `max_age_ms` — the on-`GetDigest` path (`max_age_ms = 60_000`, design/08
    /// §3.4). Returns `Some(new_generation)` if a refresh ran, `None` if the
    /// entry was fresh (or untracked). `rebuild` rebuilds the entry's hard facts
    /// + summary; it runs only on the stale branch so a fresh digest is cheap.
    pub fn force_refresh_if_stale(
        &mut self,
        wa: &WorkareaId,
        max_age_ms: i64,
        rebuild: impl FnOnce(&mut WorkareaSummary),
    ) -> Option<u64> {
        if !self.is_stale(wa, max_age_ms) {
            return None;
        }
        // A missing entry cannot be refreshed in place; `is_stale` reports it
        // stale, but there is nothing to rebuild — the caller seeds it via
        // `upsert` first. Only a present-but-old entry is rebuilt here.
        if !self.entries.contains_key(wa) {
            return None;
        }
        self.refresh_workarea(wa, rebuild)
    }
}

// ===========================================================================
// Hard-fact derivation (no LLM).
// ===========================================================================

/// The hard facts for one repo, derived from already-fetched inputs.
///
/// `commits_ahead` is the one fact that shells out ([`commits_ahead`]); the rest
/// are pure derivations the caller passes in (the `DiffPayload`, the
/// `pull_requests.state` string, and the opaque `checks` frame). Keeping the
/// derivation pure over injected inputs is what makes trigger logic + the
/// hard-fact counts CI-provable without a live repo manager.
pub fn build_repo_summary(
    repository_id: RepositoryId,
    repo_name: impl Into<String>,
    commits_ahead: u32,
    diff: &concerto_gix_wrap::DiffPayload,
    pr_state: Option<String>,
    ci_frame: Option<&[u8]>,
) -> RepoSummary {
    let (files_changed, lines_added, lines_removed) = diff_counts(diff);
    RepoSummary {
        repository_id,
        repo_name: repo_name.into(),
        commits_ahead,
        files_changed,
        lines_added,
        lines_removed,
        pr_state: normalize_pr_state(pr_state),
        ci_state: ci_frame.and_then(parse_ci_state),
    }
}

/// Count `(files_changed, lines_added, lines_removed)` from a [`DiffPayload`].
///
/// `files_changed` is `files.len()`; the line counts sum the `+`/`-` lines of
/// every hunk body across every file. Hunk header/context lines (no leading
/// `+`/`-`, or the `+++`/`---` file markers) are not counted.
pub fn diff_counts(diff: &concerto_gix_wrap::DiffPayload) -> (u32, u32, u32) {
    let files_changed = diff.files.len() as u32;
    let mut added = 0u32;
    let mut removed = 0u32;
    for file in &diff.files {
        for hunk in &file.hunks {
            for line in hunk.body.lines() {
                // Skip the `+++`/`---` file-header markers; a real change line
                // starts with a single `+`/`-` followed by content (or EOL).
                if line.starts_with("+++") || line.starts_with("---") {
                    continue;
                }
                match line.as_bytes().first() {
                    Some(b'+') => added += 1,
                    Some(b'-') => removed += 1,
                    _ => {}
                }
            }
        }
    }
    (files_changed, added, removed)
}

/// Normalize a raw `pull_requests.state` string to the frozen set
/// (`open|closed|merged|draft`). An unrecognized/blank value maps to `None`
/// rather than leaking an arbitrary string into the summary.
pub fn normalize_pr_state(state: Option<String>) -> Option<String> {
    let s = state?;
    let s = s.trim().to_ascii_lowercase();
    match s.as_str() {
        "open" | "closed" | "merged" | "draft" => Some(s),
        _ => None,
    }
}

/// Parse the opaque `checks.<wa>.<repo>` frame into a `ci_state` string.
///
/// **The `checks.<wa>.<repo>` frame shape is not yet stable** (the check-run
/// aggregation wire format is owned downstream). This parser is therefore TOTAL
/// and conservative: it defaults to `None` and never panics, never returns a
/// fake-success, and never uses `todo!()`/`unimplemented!()` (the 305 seam
/// discipline). The entry point is FROZEN so a later task that stabilizes the
/// frame fills the body without changing the call sites.
///
/// Today it recognizes only a plain UTF-8 status token equal to one of the
/// canonical CI states (`pending|running|success|failure|error|cancelled|
/// neutral`); anything else — including a binary/length-framed payload whose
/// schema is not yet locked — yields `None`. `ci_state = None` thus means
/// "unknown / not-yet-parseable", not "no CI".
pub fn parse_ci_state(frame: &[u8]) -> Option<String> {
    let token = std::str::from_utf8(frame).ok()?.trim().to_ascii_lowercase();
    match token.as_str() {
        "pending" | "running" | "success" | "failure" | "error" | "cancelled" | "neutral" => {
            Some(token)
        }
        _ => None,
    }
}

// ===========================================================================
// The fallback summarizer (§3.4, D5).
// ===========================================================================

/// Summarize a turn via the FROZEN [`OneShotLlm`] seam
/// ([`ActionKind::DigestSummary`]).
///
/// **Prefer the agent's own end-of-turn summary** — when the closing
/// `chat_messages` row already carries a summary, the caller uses it directly
/// (it is free, design/08 §3.4) and never calls this. This fallback runs only
/// when the closing turn carries no summary. `llm` is the injected LIVE impl
/// ([`DeterministicOneShot`] in Phase 4; the real Haiku/Sonnet provider is Task
/// 412, judged at the Tier-3 gate). It consumes the Task-312 seam — it does NOT
/// add an `ActionKind` or change the trait.
pub async fn summarize_turn(llm: &dyn OneShotLlm, repo_id: &str, recent: &str) -> Result<String> {
    // No resolved pref at this seam (the digest-summary pref is a P4/412
    // concern); pass a `None` pref so `compose_action_prompt` echoes `recent`
    // through unchanged.
    let pref: Resolved<Option<String>> = Resolved {
        value: None,
        source: SettingsSource::Default,
    };
    let prompt = compose_action_prompt(ActionKind::DigestSummary, &pref, recent);
    let req = OneShotRequest::new(ActionKind::DigestSummary, repo_id, prompt, recent);
    let out = llm.suggest(req).await?;
    Ok(truncate_turn_summary(&out))
}

/// The marker a workarea's `last_turn_summary` carries when the external
/// Maestro summarizer is disabled by the enterprise-privacy policy (design/08
/// §3.10). Hard facts still render; only the LLM-derived prose is replaced.
pub const MAESTRO_DISABLED_BY_POLICY_SUMMARY: &str = "[maestro disabled by policy]";

/// Summarize a turn through the external [`OneShotLlm`] seam **only when the
/// enterprise-privacy gate allows it** (Task 413, design/08 §3.10).
///
/// This is the gated counterpart to [`summarize_turn`]: it is the call site for
/// the *external* provider path. `gate` is computed once by the caller via
/// [`PrivacyPolicy::llm_gate`] from the resolved `enterprise_data_privacy` +
/// the chosen model's externality.
///
/// - [`MaestroLlmGate::Allowed`] ⇒ issue the external call (delegates to
///   [`summarize_turn`]).
/// - [`MaestroLlmGate::DisabledExternalPolicy`] ⇒ **do not call** the external
///   provider; return [`MAESTRO_DISABLED_BY_POLICY_SUMMARY`] so the workarea
///   still renders its hard facts + the disabled marker. The in-process
///   deterministic summarizer (`DeterministicOneShot`) is NOT external and is
///   reached only through [`summarize_turn`], so callers that want a real
///   deterministic summary when disabled pass a deterministic `llm` AND
///   [`MaestroLlmGate::Allowed`] — the gate guards exactly the external egress.
pub async fn summarize_turn_gated(
    llm: &dyn OneShotLlm,
    repo_id: &str,
    recent: &str,
    gate: MaestroLlmGate,
) -> Result<String> {
    if gate.is_disabled() {
        // The external provider is disabled by policy — egress nothing.
        return Ok(MAESTRO_DISABLED_BY_POLICY_SUMMARY.to_string());
    }
    summarize_turn(llm, repo_id, recent).await
}

/// Resolve, on the refresh path, whether a workarea entry should populate the
/// raw last-3-turns or summary text only (Task 413, design/08 §3.3).
///
/// `full_chat_access` is the per-workspace `concerto_chat_full_chat_access`
/// flag (default `false`, read via
/// [`concerto_persist::workspaces::get_settings_json_bool`]). The refresher
/// consults the returned [`SummarySource`] to decide whether to keep the raw
/// last-3-turns in the cache entry ([`SummarySource::FullLast3Turns`]) or only
/// the summary text ([`SummarySource::SummaryOnly`], the default).
pub fn refresh_summary_source(full_chat_access: bool) -> SummarySource {
    PrivacyPolicy::summary_source(full_chat_access)
}

/// The `commits_ahead` hard fact via the new gix-wrap helper. Thin re-export so
/// the cache's hard-fact derivation reaches the git primitive without `core`
/// gaining a git dep (the 305 placement rule — the shell-out lives in
/// `gix-wrap`).
pub async fn commits_ahead(worktree_path: &Path, base: &str) -> Result<u32> {
    concerto_gix_wrap::commits_ahead(worktree_path, base).await
}

// ===========================================================================
// Helpers.
// ===========================================================================

/// Truncate a turn summary to [`MAX_TURN_SUMMARY_LEN`] chars (design/08 §3.3:
/// "≤ 300 chars"), on a char boundary, trimming trailing whitespace.
fn truncate_turn_summary(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= MAX_TURN_SUMMARY_LEN {
        return s.to_string();
    }
    let truncated: String = s.chars().take(MAX_TURN_SUMMARY_LEN).collect();
    truncated.trim_end().to_string()
}

/// Push `summary` onto a bounded ring of the last 3 turn summaries (most-recent
/// last). Drops the oldest when full.
fn push_recent(ring: &mut Vec<String>, summary: String) {
    ring.push(summary);
    while ring.len() > 3 {
        ring.remove(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use concerto_gix_wrap::{DiffHunk, DiffKind, DiffPayload, FileDiff};
    use std::path::PathBuf;

    fn wa(id: &str) -> WorkareaId {
        WorkareaId(id.to_string())
    }
    fn ws(id: &str) -> WorkspaceId {
        WorkspaceId(id.to_string())
    }
    fn sid(id: &str) -> SessionId {
        SessionId(id.to_string())
    }
    fn repo(id: &str) -> RepositoryId {
        RepositoryId(id.to_string())
    }

    /// A seed entry with no summaries/repos, at a given activity time.
    ///
    /// `upsert` stamps `generated_at`/`generation` from the cache clock but
    /// leaves the caller's `last_activity_at` intact, so the value passed here
    /// is what the idle/stale predicates see.
    fn seed(cache: &mut SummaryCache, id: &str, last_activity_at: i64) {
        let s = WorkareaSummary {
            workarea_id: wa(id),
            workspace_id: ws("ws-1"),
            workspace_name: "Workspace One".into(),
            composer_name: "bach".into(),
            branch_name: "concerto/bach".into(),
            status: "running".into(),
            last_activity_at,
            sessions: vec![SessionSummary {
                session_id: sid("sess-1"),
                agent_kind: AgentKind::Claude,
                model: "claude".into(),
                status: "running".into(),
                last_turn_summary: String::new(),
            }],
            last_turn_summary: String::new(),
            last_3_turn_summaries: Vec::new(),
            repos: Vec::new(),
            blocked_on: None,
            generated_at: 0,
            generation: 0,
        };
        cache.upsert(s);
    }

    #[test]
    fn turn_complete_bumps_generation_and_updates_right_workarea() {
        let clock = Box::new(ManualClock::new(1_000));
        let mut cache = SummaryCache::new(clock);
        seed(&mut cache, "wa-a", 1_000);
        seed(&mut cache, "wa-b", 1_000);

        let gen_before = cache.get(&wa("wa-a")).unwrap().generation;
        let new_gen = cache
            .on_turn_complete(&wa("wa-a"), &sid("sess-1"), "did the thing")
            .expect("tracked");
        assert!(new_gen > gen_before, "generation must bump");

        let a = cache.get(&wa("wa-a")).unwrap();
        assert_eq!(a.last_turn_summary, "did the thing");
        assert_eq!(a.last_3_turn_summaries, vec!["did the thing".to_string()]);
        assert_eq!(a.sessions[0].last_turn_summary, "did the thing");

        // The OTHER workarea is untouched.
        let b = cache.get(&wa("wa-b")).unwrap();
        assert_eq!(b.last_turn_summary, "");

        // Untracked workarea → None.
        assert!(cache
            .on_turn_complete(&wa("nope"), &sid("sess-1"), "x")
            .is_none());
    }

    #[test]
    fn last_3_turn_summaries_is_bounded_to_three() {
        let mut cache = SummaryCache::new(Box::new(ManualClock::new(0)));
        seed(&mut cache, "wa-a", 0);
        for t in ["t1", "t2", "t3", "t4"] {
            cache.on_turn_complete(&wa("wa-a"), &sid("sess-1"), t);
        }
        let a = cache.get(&wa("wa-a")).unwrap();
        assert_eq!(a.last_3_turn_summaries, vec!["t2", "t3", "t4"]);
        assert_eq!(a.last_turn_summary, "t4");
    }

    #[test]
    fn status_change_updates_status_and_blocked_on() {
        let mut cache = SummaryCache::new(Box::new(ManualClock::new(5)));
        seed(&mut cache, "wa-a", 5);

        cache
            .on_status_change(&wa("wa-a"), "blocked", Some("awaiting_approval".into()))
            .expect("tracked");
        let a = cache.get(&wa("wa-a")).unwrap();
        assert_eq!(a.status, "blocked");
        assert_eq!(a.blocked_on.as_deref(), Some("awaiting_approval"));

        // Transition back to running clears the block.
        cache.on_status_change(&wa("wa-a"), "running", None);
        let a = cache.get(&wa("wa-a")).unwrap();
        assert_eq!(a.status, "running");
        assert!(a.blocked_on.is_none());
    }

    #[test]
    fn is_stale_and_force_refresh_against_synthetic_clock() {
        let clock = Box::new(ManualClock::new(0));
        // Keep a raw pointer to advance the clock; we own it via the cache, so
        // build a second handle by sharing through an Arc-free manual approach:
        // construct, seed at t=0, then advance using the cache's own clock by
        // re-creating. Simpler: drive time via a dedicated ManualClock.
        let mut cache = SummaryCache::new(clock);
        seed(&mut cache, "wa-a", 0);
        // The seed entry's generated_at was stamped at now()=0 by upsert.

        // Fresh: not stale at 60s window when 30s elapsed.
        cache.set_clock_for_test(30_000);
        assert!(!cache.is_stale(&wa("wa-a"), GET_DIGEST_STALE_MS));
        assert!(cache
            .force_refresh_if_stale(&wa("wa-a"), GET_DIGEST_STALE_MS, |_| {})
            .is_none());

        // Stale: 61s elapsed > 60s window.
        cache.set_clock_for_test(61_000);
        assert!(cache.is_stale(&wa("wa-a"), GET_DIGEST_STALE_MS));
        let mut ran = false;
        let gen = cache.force_refresh_if_stale(&wa("wa-a"), GET_DIGEST_STALE_MS, |e| {
            e.last_turn_summary = "rebuilt".into();
            ran = true;
        });
        assert!(gen.is_some(), "stale entry should refresh");
        assert!(ran, "rebuild closure must run on the stale branch");
        assert_eq!(cache.get(&wa("wa-a")).unwrap().last_turn_summary, "rebuilt");

        // Untracked workarea: is_stale=true but force_refresh has nothing to
        // rebuild → None (caller must seed first).
        assert!(cache.is_stale(&wa("ghost"), GET_DIGEST_STALE_MS));
        assert!(cache
            .force_refresh_if_stale(&wa("ghost"), GET_DIGEST_STALE_MS, |_| {})
            .is_none());
    }

    #[test]
    fn idle_predicate_uses_last_activity() {
        let mut cache = SummaryCache::new(Box::new(ManualClock::new(0)));
        seed(&mut cache, "wa-a", 0);
        cache.set_clock_for_test(IDLE_REFRESH_MS - 1);
        assert!(!cache.is_idle(&wa("wa-a")));
        cache.set_clock_for_test(IDLE_REFRESH_MS);
        assert!(cache.is_idle(&wa("wa-a")));
    }

    #[test]
    fn diff_counts_from_fixture_payload() {
        // 2 files; file A: +2/-1, file B: +1/-0.
        let diff = DiffPayload {
            files: vec![
                FileDiff {
                    path: PathBuf::from("a.rs"),
                    kind: DiffKind::Modified,
                    old_path: None,
                    hunks: vec![DiffHunk {
                        old_start: 1,
                        old_lines: 2,
                        new_start: 1,
                        new_lines: 3,
                        body: " ctx\n+added one\n+added two\n-removed one".into(),
                    }],
                },
                FileDiff {
                    path: PathBuf::from("b.rs"),
                    kind: DiffKind::Added,
                    old_path: None,
                    hunks: vec![DiffHunk {
                        old_start: 0,
                        old_lines: 0,
                        new_start: 1,
                        new_lines: 1,
                        body: "+only added".into(),
                    }],
                },
            ],
        };
        let (files, added, removed) = diff_counts(&diff);
        assert_eq!(files, 2);
        assert_eq!(added, 3);
        assert_eq!(removed, 1);
    }

    #[test]
    fn build_repo_summary_derives_hard_facts() {
        let diff = DiffPayload {
            files: vec![FileDiff {
                path: PathBuf::from("x.rs"),
                kind: DiffKind::Modified,
                old_path: None,
                hunks: vec![DiffHunk {
                    old_start: 1,
                    old_lines: 1,
                    new_start: 1,
                    new_lines: 2,
                    body: "+new\n-old".into(),
                }],
            }],
        };
        let rs = build_repo_summary(
            repo("r1"),
            "my-repo",
            7,
            &diff,
            Some("OPEN".into()),
            Some(b"success"),
        );
        assert_eq!(rs.commits_ahead, 7);
        assert_eq!(rs.files_changed, 1);
        assert_eq!(rs.lines_added, 1);
        assert_eq!(rs.lines_removed, 1);
        assert_eq!(rs.pr_state.as_deref(), Some("open")); // normalized
        assert_eq!(rs.ci_state.as_deref(), Some("success"));
    }

    #[test]
    fn pr_state_normalizes_and_rejects_garbage() {
        assert_eq!(
            normalize_pr_state(Some("Merged".into())).as_deref(),
            Some("merged")
        );
        assert_eq!(
            normalize_pr_state(Some("  draft ".into())).as_deref(),
            Some("draft")
        );
        assert_eq!(normalize_pr_state(Some("bogus".into())), None);
        assert_eq!(normalize_pr_state(None), None);
    }

    #[test]
    fn ci_state_parser_is_total_and_defaults_none() {
        assert_eq!(parse_ci_state(b"success").as_deref(), Some("success"));
        assert_eq!(parse_ci_state(b"FAILURE").as_deref(), Some("failure"));
        // Unknown token → None (not a panic, not a fake success).
        assert_eq!(parse_ci_state(b"weird-token"), None);
        // Non-stable binary frame → None.
        assert_eq!(parse_ci_state(&[0xff, 0x00, 0x01]), None);
        // Empty → None.
        assert_eq!(parse_ci_state(b""), None);
    }

    #[tokio::test]
    async fn summarizer_routes_through_deterministic_oneshot() {
        use crate::llm::oneshot::DeterministicOneShot;
        let llm = DeterministicOneShot;
        // DigestSummary's deterministic path echoes the whitespace-collapsed
        // context (oneshot.rs `_ => echo`), proving the seam without a real LLM.
        let out = summarize_turn(&llm, "repo-1", "  applied   the migration  ")
            .await
            .expect("summarize");
        assert_eq!(out, "applied the migration");
    }

    #[tokio::test]
    async fn summarizer_truncates_to_300_chars() {
        use crate::llm::oneshot::DeterministicOneShot;
        let llm = DeterministicOneShot;
        let long = "word ".repeat(200); // 1000 chars
        let out = summarize_turn(&llm, "repo-1", &long).await.unwrap();
        assert!(out.chars().count() <= MAX_TURN_SUMMARY_LEN);
    }

    #[test]
    fn get_for_maestro_blanks_excluded_at_serve_time() {
        let mut cache = SummaryCache::new(Box::new(ManualClock::new(0)));
        seed(&mut cache, "wa-a", 0);
        // Seed real chat prose so blanking has something to strip.
        cache.on_turn_complete(&wa("wa-a"), &sid("sess-1"), "secret prose");

        // Not excluded ⇒ raw summary served.
        let raw = cache.get_for_maestro(&wa("wa-a"), false).unwrap();
        assert_eq!(raw.last_turn_summary, "secret prose");
        assert_eq!(raw.sessions[0].last_turn_summary, "secret prose");

        // Excluded ⇒ name-only; hard facts (branch/status) survive.
        let blanked = cache.get_for_maestro(&wa("wa-a"), true).unwrap();
        assert_eq!(
            blanked.last_turn_summary,
            crate::maestro::privacy::PRIVATE_WORKAREA_BLANK
        );
        assert!(blanked.last_3_turn_summaries.is_empty());
        assert!(blanked.sessions.is_empty());
        assert_eq!(blanked.branch_name, "concerto/bach");
        assert_eq!(blanked.status, "running");

        // The cache entry itself is untouched (blanking is a serve-time copy).
        assert_eq!(
            cache.get(&wa("wa-a")).unwrap().last_turn_summary,
            "secret prose"
        );
    }

    #[test]
    fn list_for_maestro_blanks_only_excluded_workareas() {
        let mut cache = SummaryCache::new(Box::new(ManualClock::new(0)));
        seed(&mut cache, "wa-pub", 0);
        seed(&mut cache, "wa-priv", 0);
        cache.on_turn_complete(&wa("wa-pub"), &sid("sess-1"), "public prose");
        cache.on_turn_complete(&wa("wa-priv"), &sid("sess-1"), "private prose");

        let served = cache.list_for_maestro(|id| id == &wa("wa-priv"));
        let pubd = served
            .iter()
            .find(|s| s.workarea_id == wa("wa-pub"))
            .unwrap();
        let privd = served
            .iter()
            .find(|s| s.workarea_id == wa("wa-priv"))
            .unwrap();
        assert_eq!(pubd.last_turn_summary, "public prose");
        assert_eq!(
            privd.last_turn_summary,
            crate::maestro::privacy::PRIVATE_WORKAREA_BLANK
        );
    }

    #[tokio::test]
    async fn summarize_turn_gated_skips_external_call_when_disabled() {
        use crate::llm::oneshot::DeterministicOneShot;
        let llm = DeterministicOneShot;

        // Allowed ⇒ the external seam runs (here a deterministic stand-in).
        let allowed = summarize_turn_gated(&llm, "repo-1", "did the work", MaestroLlmGate::Allowed)
            .await
            .unwrap();
        assert_eq!(allowed, "did the work");

        // Disabled ⇒ no external call; the policy marker is returned.
        let disabled = summarize_turn_gated(
            &llm,
            "repo-1",
            "did the work",
            MaestroLlmGate::DisabledExternalPolicy,
        )
        .await
        .unwrap();
        assert_eq!(disabled, MAESTRO_DISABLED_BY_POLICY_SUMMARY);
    }

    #[test]
    fn refresh_summary_source_honors_full_chat_access() {
        assert_eq!(refresh_summary_source(true), SummarySource::FullLast3Turns);
        assert_eq!(refresh_summary_source(false), SummarySource::SummaryOnly);
    }
}
