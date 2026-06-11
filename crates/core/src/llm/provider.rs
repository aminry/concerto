//! The Maestro LLM **daily token budget** + inert-on-exhaust state (Task 412,
//! design/08 §3.9 / PHASE4_PLANNING §4.6 / D6).
//!
//! NET-NEW token accounting. There is **zero** prior token accounting in the
//! codebase; `AgentEvent::ContextUsage{pct}` is wired-but-never-emitted and is
//! explicitly **NOT** the carrier (D6). [`parse_token_usage`] is the carrier:
//! it scrapes `(in, out)` token counts from each live CLI's end-of-turn usage
//! report; [`TokenBudget`] accumulates them, cumulative **across backends**
//! (the counter lives in one `maestro_state` row regardless of which provider
//! produced the tokens), and goes **inert on exhaust** (LLM calls stop; routing
//! + deterministic tools keep working; the last good digest is served with a
//!   stale badge, design/08 R-7).
//!
//! ## Source of truth
//!
//! `maestro_state` (Task 403, migration 0015) is the single source of truth.
//! The in-memory [`TokenBudget`] is a cache hydrated at boot from 403's
//! singleton-get; every [`TokenBudget::record_usage`] / [`TokenBudget::reset`]
//! writes through 403's `bump_daily_counters` / `reset_budget` accessor so a
//! Core restart mid-day resumes the same cumulative count — the budget never
//! resets on restart, only at UTC midnight or a manual reset.
//!
//! ## What this does NOT cover (Tier-2 double)
//!
//! The mock-provider scripted-token-count double proves the budget math,
//! cumulative-across-backends, inert-on-exhaust + last-good-digest/stale, the
//! reset clock, and the parse seam. It does **NOT** prove real Codex/Gemini CLI
//! token-report accuracy or the real on-prem Direct-API loop — those are the
//! Phase-4 Tier-3 checklist line "confirm budget-exhaust goes inert while
//! routing still works."

use concerto_error::Result;
use concerto_persist::{maestro_state, MaestroState};
use sqlx::SqliteConnection;

use crate::maestro::provider::MaestroBackend;

/// Default daily **input**-token cap (design/08 §3.9). User/managed-overridable,
/// but the cap source is out of this task — this is the default constant.
pub const DEFAULT_DAILY_IN_CAP: u64 = 200_000;

/// Default daily **output**-token cap (design/08 §3.9).
pub const DEFAULT_DAILY_OUT_CAP: u64 = 50_000;

/// Milliseconds in a UTC day — the reset-clock granularity.
const MS_PER_DAY: i64 = 86_400_000;

/// The next UTC-midnight instant strictly after `now_unix_ms`, as unix-ms.
///
/// Pure integer arithmetic (no `chrono` dependency in `concerto-core`), mirroring
/// the `div_euclid`/`rem_euclid` day math in `log_filter`/`audit`. UTC midnight
/// is a multiple of [`MS_PER_DAY`]; this returns the first such multiple `>
/// now_unix_ms` so a reset always advances the clock.
pub fn next_utc_midnight_ms(now_unix_ms: i64) -> i64 {
    // Floor to the start of today's UTC day, then add one day.
    let day_start = now_unix_ms.div_euclid(MS_PER_DAY) * MS_PER_DAY;
    day_start + MS_PER_DAY
}

/// Net-new daily token accounting (D6). Cumulative ACROSS backends. The
/// in-memory copy is a cache; `maestro_state` (Task 403) is the source of
/// truth — every mutating method writes through 403's accessor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenBudget {
    /// Input tokens spent today (cumulative across backends).
    pub daily_in_today: u64,
    /// Output tokens spent today (cumulative across backends).
    pub daily_out_today: u64,
    /// Daily input cap (default [`DEFAULT_DAILY_IN_CAP`]).
    pub in_cap: u64,
    /// Daily output cap (default [`DEFAULT_DAILY_OUT_CAP`]).
    pub out_cap: u64,
    /// Next reset instant (unix-ms; UTC midnight or manual).
    pub resets_at_unix_ms: i64,
}

impl TokenBudget {
    /// Construct a budget with the default 200K/50K caps and an explicit reset
    /// instant, zero counters. Used when no persisted state exists yet.
    pub fn new(resets_at_unix_ms: i64) -> Self {
        Self {
            daily_in_today: 0,
            daily_out_today: 0,
            in_cap: DEFAULT_DAILY_IN_CAP,
            out_cap: DEFAULT_DAILY_OUT_CAP,
            resets_at_unix_ms,
        }
    }

    /// Hydrate the in-memory cache from 403's persisted [`MaestroState`]
    /// singleton (the boot path). Caps default to 200K/50K (the cap-source
    /// override is out of this task). Negative stored counters (impossible
    /// under the additive accessor) clamp to 0.
    pub fn from_state(state: &MaestroState) -> Self {
        Self {
            daily_in_today: state.daily_in_today.max(0) as u64,
            daily_out_today: state.daily_out_today.max(0) as u64,
            in_cap: DEFAULT_DAILY_IN_CAP,
            out_cap: DEFAULT_DAILY_OUT_CAP,
            resets_at_unix_ms: state.budget_resets_at,
        }
    }

    /// Bump both counters by a parsed turn's usage AND persist via 403's
    /// `bump_daily_counters` accessor (the single source of truth). Cumulative
    /// across backends — the counter lives in one row regardless of which
    /// provider produced the tokens (D6).
    pub async fn record_usage(
        &mut self,
        conn: &mut SqliteConnection,
        in_tokens: u64,
        out_tokens: u64,
    ) -> Result<()> {
        self.daily_in_today = self.daily_in_today.saturating_add(in_tokens);
        self.daily_out_today = self.daily_out_today.saturating_add(out_tokens);
        maestro_state::bump_daily_counters(conn, in_tokens as i64, out_tokens as i64).await
    }

    /// `true` when either counter has reached its cap. The interactive LLM path
    /// goes inert; routing + deterministic tools keep working (design/08 §3.9).
    pub fn is_exhausted(&self) -> bool {
        self.daily_in_today >= self.in_cap || self.daily_out_today >= self.out_cap
    }

    /// Manual + UTC-midnight reset: zero both counters and advance the reset
    /// instant to the next UTC midnight strictly after `now_unix_ms`,
    /// persisting via 403's `reset_budget` accessor.
    pub async fn reset(&mut self, conn: &mut SqliteConnection, now_unix_ms: i64) -> Result<()> {
        let next = next_utc_midnight_ms(now_unix_ms);
        self.daily_in_today = 0;
        self.daily_out_today = 0;
        self.resets_at_unix_ms = next;
        maestro_state::reset_budget(conn, next).await
    }

    /// `true` when the reset clock has elapsed (`now_unix_ms >=
    /// resets_at_unix_ms`) — the UTC-midnight rollover trigger 414 polls.
    pub fn is_reset_due(&self, now_unix_ms: i64) -> bool {
        now_unix_ms >= self.resets_at_unix_ms
    }
}

/// Per-backend end-of-turn usage parse (NET-NEW carrier — NOT
/// `ContextUsage{pct}`, D6). Extracts `(in_tokens, out_tokens)` from the live
/// CLI's end-of-turn usage line / structured event. `None` is the honest
/// "couldn't account this turn" answer (logged at `debug`, never a panic,
/// never a silent 0 that under-counts).
///
/// The exact scrape source per CLI is what the live CLIs emit at end of turn;
/// the Codex/Gemini arms parse the structured `tokens used: <in> input, <out>
/// output` usage line each emits. The Direct-API arm has no CLI stream (it is
/// the frozen seam) and returns `None`. Real round-trip accuracy is the Tier-3
/// gate.
pub fn parse_token_usage(backend: MaestroBackend, raw: &str) -> Option<(u64, u64)> {
    let parsed = match backend {
        MaestroBackend::Claude | MaestroBackend::Codex | MaestroBackend::Gemini => {
            parse_cli_usage_line(raw)
        }
        // The Direct-API backend is the frozen-unwired seam: its native
        // token-accounting is the fast-follow, so there is no usage to parse
        // here yet.
        MaestroBackend::Direct => None,
    };

    if parsed.is_none() {
        tracing::debug!(
            backend = ?backend,
            "parse_token_usage: unparseable end-of-turn usage; turn not accounted"
        );
    }
    parsed
}

/// Extract `(in, out)` from a CLI end-of-turn usage line of the shape
/// `... <in> input ... <out> output ...` (case-insensitive; the integer
/// immediately preceding the `input`/`output` keyword). Returns `None` when
/// either count is absent (an unaccountable turn).
fn parse_cli_usage_line(raw: &str) -> Option<(u64, u64)> {
    let in_tokens = preceding_count(raw, "input")?;
    let out_tokens = preceding_count(raw, "output")?;
    Some((in_tokens, out_tokens))
}

/// Find the integer token-count immediately preceding the first occurrence of
/// `keyword` (case-insensitive whole-word) in `raw`.
fn preceding_count(raw: &str, keyword: &str) -> Option<u64> {
    let lower = raw.to_ascii_lowercase();
    let tokens: Vec<&str> = lower.split_whitespace().collect();
    let pos = tokens.iter().position(|t| {
        // Strip trailing punctuation so `output,` matches `output`.
        t.trim_end_matches(|c: char| !c.is_ascii_alphanumeric()) == keyword
    })?;
    if pos == 0 {
        return None;
    }
    tokens[pos - 1]
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse::<u64>()
        .ok()
}

/// The Maestro's inert-on-exhaust state (design/08 §3.9 / R-7). When the budget
/// is exhausted the interactive LLM path is skipped — `start_session` /
/// `send_input` of free-form prompts to the agent is suppressed — while routing
/// + deterministic tools still execute and the **last good digest** is served
///   with a `stale` badge instead of a fresh LLM call.
///
/// 414 reads the typed [`InertReason`] to publish `maestro.budget_exhausted` /
/// `maestro.disabled_by_policy`; 415 reads [`MaestroLlmState::is_inert`] /
/// [`MaestroLlmState::stale`] / the current backend to render the banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaestroLlmState {
    /// The selected backend, or `None` when no backend is configured.
    pub backend: Option<MaestroBackend>,
    /// `Some(reason)` when the interactive LLM path is inert; `None` when live.
    pub inert: Option<InertReason>,
    /// The last good digest text, served with a stale badge while inert (R-7).
    /// `None` until the first digest exists.
    pub last_good_digest: Option<String>,
}

/// Why the Maestro's interactive LLM path is inert (typed so 414 can publish
/// the matching event without string-matching).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InertReason {
    /// The daily token budget is exhausted (design/08 §3.9). 414 publishes
    /// `maestro.budget_exhausted`.
    BudgetExhausted,
    /// `enterpriseDataPrivacy=true` selected an external Direct backend
    /// (design/08 §3.10). 414 publishes `maestro.disabled_by_policy`.
    DisabledByPolicy,
}

impl MaestroLlmState {
    /// A live state for `backend` (not inert, no digest yet).
    pub fn live(backend: MaestroBackend) -> Self {
        Self {
            backend: Some(backend),
            inert: None,
            last_good_digest: None,
        }
    }

    /// `true` when the interactive LLM path is inert (budget exhausted or
    /// disabled by policy) — the caller must skip the free-form LLM turn while
    /// keeping routing + deterministic tools live.
    pub fn is_inert(&self) -> bool {
        self.inert.is_some()
    }

    /// `true` when a served digest must carry the stale badge (R-7): the LLM
    /// path is inert, so `get_digest` serves the last good digest rather than a
    /// fresh LLM call.
    pub fn stale(&self) -> bool {
        self.is_inert()
    }

    /// Record a fresh digest (clears nothing — the digest is good whether or
    /// not the LLM later goes inert).
    pub fn set_last_good_digest(&mut self, digest: String) {
        self.last_good_digest = Some(digest);
    }

    /// Mark the LLM path inert with `reason` (budget exhausted / policy). Idempotent.
    pub fn set_inert(&mut self, reason: InertReason) {
        self.inert = Some(reason);
    }

    /// Mark the LLM path live again (e.g. after a budget reset).
    pub fn clear_inert(&mut self) {
        self.inert = None;
    }

    /// Serve the digest under the inert gate (R-7): when inert, return the last
    /// good digest with `stale = true`; when live, the caller makes a fresh LLM
    /// call instead (returns `None` so the caller knows to generate one).
    pub fn digest_for_serving(&self) -> Option<StaleDigest<'_>> {
        if self.is_inert() {
            self.last_good_digest
                .as_deref()
                .map(|text| StaleDigest { text, stale: true })
        } else {
            None
        }
    }
}

/// A digest served while the LLM path is inert: the last good digest text plus
/// the `stale` badge the UI renders (design/08 R-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaleDigest<'a> {
    /// The last good digest text.
    pub text: &'a str,
    /// Always `true` here — the digest is the last good one, not a fresh call.
    pub stale: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use concerto_persist::{Persistence, PersistenceConfig};

    async fn fresh_db() -> (tempfile::TempDir, Persistence) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let persist = Persistence::open(PersistenceConfig {
            db_path,
            max_readers: 2,
        })
        .await
        .expect("open");
        (dir, persist)
    }

    #[test]
    fn next_utc_midnight_advances_and_is_a_day_boundary() {
        // 2021-11-14T22:13:20Z = 1_636_927_200_000 ms.
        let now = 1_636_927_200_000;
        let next = next_utc_midnight_ms(now);
        assert!(next > now, "reset must advance the clock");
        assert_eq!(
            next % MS_PER_DAY,
            0,
            "reset lands on a UTC-midnight boundary"
        );
        // Exactly on a midnight boundary still advances by a full day.
        assert_eq!(next_utc_midnight_ms(0), MS_PER_DAY);
    }

    #[tokio::test]
    async fn record_usage_accumulates_across_backends_and_persists() {
        let (_dir, persist) = fresh_db().await;
        {
            let mut w = persist.writer().await;
            maestro_state::ensure_initialized(&mut w, MS_PER_DAY)
                .await
                .expect("init");
        }

        let mut budget = TokenBudget::new(MS_PER_DAY);
        {
            let mut w = persist.writer().await;
            // Two different backends bump the same cumulative counter (D6).
            budget.record_usage(&mut w, 100, 20).await.expect("claude");
            budget.record_usage(&mut w, 50, 10).await.expect("codex");
        }
        assert_eq!(budget.daily_in_today, 150);
        assert_eq!(budget.daily_out_today, 30);

        // The write-through reaches maestro_state (the source of truth).
        let state = maestro_state::get(persist.readers())
            .await
            .expect("get")
            .expect("present");
        assert_eq!(state.daily_in_today, 150);
        assert_eq!(state.daily_out_today, 30);

        // Hydrating a fresh cache from the persisted row resumes the count
        // (survives a Core restart).
        let hydrated = TokenBudget::from_state(&state);
        assert_eq!(hydrated.daily_in_today, 150);
        assert_eq!(hydrated.daily_out_today, 30);
    }

    #[test]
    fn is_exhausted_trips_on_either_cap() {
        let mut b = TokenBudget::new(0);
        assert!(!b.is_exhausted());
        // Crossing the input cap alone trips it.
        b.daily_in_today = DEFAULT_DAILY_IN_CAP;
        assert!(b.is_exhausted());

        let mut b = TokenBudget::new(0);
        // Crossing the output cap alone trips it.
        b.daily_out_today = DEFAULT_DAILY_OUT_CAP;
        assert!(b.is_exhausted());
    }

    #[tokio::test]
    async fn reset_zeroes_counters_and_advances_resets_at() {
        let (_dir, persist) = fresh_db().await;
        {
            let mut w = persist.writer().await;
            maestro_state::ensure_initialized(&mut w, 1)
                .await
                .expect("init");
        }
        let mut budget = TokenBudget::new(1);
        let now = 1_636_927_200_000; // mid-day
        {
            let mut w = persist.writer().await;
            budget
                .record_usage(&mut w, 5_000, 1_000)
                .await
                .expect("bump");
            budget.reset(&mut w, now).await.expect("reset");
        }
        assert_eq!(budget.daily_in_today, 0);
        assert_eq!(budget.daily_out_today, 0);
        assert_eq!(budget.resets_at_unix_ms, next_utc_midnight_ms(now));

        let state = maestro_state::get(persist.readers())
            .await
            .expect("get")
            .expect("present");
        assert_eq!(state.daily_in_today, 0);
        assert_eq!(state.daily_out_today, 0);
        assert_eq!(state.budget_resets_at, next_utc_midnight_ms(now));
    }

    #[test]
    fn parse_token_usage_extracts_codex_and_gemini_samples() {
        // Recorded-shape Codex usage line.
        let codex = "session complete — tokens used: 1234 input, 567 output";
        assert_eq!(
            parse_token_usage(MaestroBackend::Codex, codex),
            Some((1234, 567))
        );
        // Recorded-shape Gemini usage line (different prose, same keywords).
        let gemini = "Usage: 4,096 input tokens / 2,048 output tokens.";
        assert_eq!(
            parse_token_usage(MaestroBackend::Gemini, gemini),
            Some((4096, 2048))
        );
    }

    #[test]
    fn parse_token_usage_returns_none_on_garbage_without_panic() {
        assert_eq!(
            parse_token_usage(MaestroBackend::Codex, "no usage here"),
            None
        );
        assert_eq!(parse_token_usage(MaestroBackend::Gemini, ""), None);
        // The Direct-API seam has no CLI stream ⇒ None.
        assert_eq!(
            parse_token_usage(MaestroBackend::Direct, "1 input 1 output"),
            None
        );
    }

    #[test]
    fn inert_on_exhaust_serves_last_good_digest_with_stale_badge() {
        let mut state = MaestroLlmState::live(MaestroBackend::Claude);
        state.set_last_good_digest("yesterday's digest".to_string());

        // Live: routing/tools run, get_digest makes a fresh call (None here).
        assert!(!state.is_inert());
        assert!(!state.stale());
        assert!(state.digest_for_serving().is_none());

        // Exhausted: the LLM path is inert; the last good digest is served stale.
        state.set_inert(InertReason::BudgetExhausted);
        assert!(state.is_inert());
        assert!(state.stale());
        let served = state.digest_for_serving().expect("last good digest");
        assert_eq!(served.text, "yesterday's digest");
        assert!(served.stale);

        // A reset clears inert ⇒ back to fresh-call behavior.
        state.clear_inert();
        assert!(!state.is_inert());
        assert!(state.digest_for_serving().is_none());
    }

    #[test]
    fn disabled_by_policy_is_a_distinct_inert_reason() {
        let mut state = MaestroLlmState::live(MaestroBackend::Direct);
        state.set_inert(InertReason::DisabledByPolicy);
        assert_eq!(state.inert, Some(InertReason::DisabledByPolicy));
        assert!(state.is_inert());
    }
}
