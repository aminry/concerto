//! Per-provider rate-limit budgets, the FROZEN polling-cadence constants, the
//! warning / degraded-cadence / exhaustion logic, and the queue-resume timer
//! (Task 314, `design/13 §3.3`/§3.9, `design/05 §3.9`).
//!
//! GitHub bills **separate** quotas against an App installation, a PAT, and the
//! `gh` CLI; conflating them is the bug this module exists to avoid. Budgets are
//! keyed strictly off the [`ProviderKey`] enum 313 froze (`GithubApp(app_id) |
//! GithubPat | GhCli`). [`RateLimitPools`] is the three-pool tracker the
//! dispatcher reads; it is seeded from the live `X-RateLimit-*` response headers
//! on every octocrab call ([`RateLimitBudget::observe_headers`]) and exposes the
//! degrade / warn / exhaust decisions plus a per-pool [`ResumeQueue`] timer.
//!
//! ## Single source of truth for the cadence
//!
//! The §3.3 polling-cadence numbers (PR-state 30s/5min, check-run backoff
//! `1,2,4,8,16,30(cap)`, review-thread 60s, deployment 60s) are **identical** to
//! `design/05 §3.9`'s `wait_for_check_runs` backoff. They live here as named
//! constants so this task and Task 318 import the same literals — a divergence
//! between §3.3 and §3.9 would be a latent bug. **318's author: import
//! [`CHECK_RUN_BACKOFF_SECS`] from this module.** Degraded cadence is these
//! numbers **doubled** ([`degraded_interval_secs`]).
//!
//! ## What the Tier-2 double does NOT cover
//!
//! All logic here is proven against 313's `testkit` (synthetic `X-RateLimit-*`
//! headers + the [`testkit::SyntheticClock`](crate::testkit)). It does NOT cover
//! a real GitHub App installation-token mint or a real rate-limit degradation
//! under live load — the Tier-3 Phase-3 checklist line.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use concerto_error::Error;

use crate::dispatch::{ProviderKey, RateLimitBudget};

// ---------------------------------------------------------------------------
// FROZEN cadence constants (`design/13 §3.3` == `design/05 §3.9`)
// ---------------------------------------------------------------------------

/// Check-run polling backoff sequence in seconds, `30s` cap (`design/13 §3.3` ==
/// `design/05 §3.9`). **FROZEN** — Task 318's `wait_for_check_runs` imports this
/// exact slice so the two cadences cannot drift. The last value is the cap: a
/// loop past the end of the slice keeps using `30`.
pub const CHECK_RUN_BACKOFF_SECS: [u64; 6] = [1, 2, 4, 8, 16, 30];

/// PR-state poll while the workarea is in the **foreground** (`design/13 §3.3`).
pub const PR_STATE_FOREGROUND_SECS: u64 = 30;

/// PR-state poll while the workarea is in the **background** (`design/13 §3.3`).
pub const PR_STATE_BACKGROUND_SECS: u64 = 300;

/// Review-thread poll while the Checks panel is open (`design/13 §3.3`).
pub const REVIEW_THREAD_SECS: u64 = 60;

/// Deployment poll (`design/13 §3.3`).
pub const DEPLOYMENT_SECS: u64 = 60;

/// Budget fraction below which the pool is **degraded** (`design/13 §3.9`):
/// polling cadence doubles + background ops yield to user-driven ones.
pub const DEGRADE_FRACTION: f64 = 0.10;

/// Budget fraction below which a **soft warning** fires (`design/13 §3.9`/§5.3):
/// `vcs.rate_limit_warning` is broadcast (once per threshold crossing).
pub const WARN_FRACTION: f64 = 0.20;

/// The next backoff interval for check-run polling at zero-based `attempt`,
/// capped at `30s` (`design/13 §3.3`). Past the end of [`CHECK_RUN_BACKOFF_SECS`]
/// it returns the cap. Task 318 calls this from `wait_for_check_runs`.
pub fn check_run_backoff_secs(attempt: usize) -> u64 {
    let last = CHECK_RUN_BACKOFF_SECS.len() - 1;
    CHECK_RUN_BACKOFF_SECS[attempt.min(last)]
}

/// Apply the `design/13 §3.9` degraded multiplier to a base cadence interval:
/// the interval **doubles** when the relevant pool is degraded, otherwise it is
/// unchanged. The base intervals stay FROZEN from §3.3 (this is a multiplier, not
/// a rewrite).
pub fn degraded_interval_secs(base_secs: u64, degraded: bool) -> u64 {
    if degraded {
        base_secs.saturating_mul(2)
    } else {
        base_secs
    }
}

// ---------------------------------------------------------------------------
// RateLimitBudget logic (the FROZEN struct lives in dispatch.rs; 314 adds the
// behaviour `design/13` Public interface names).
// ---------------------------------------------------------------------------

impl RateLimitBudget {
    /// Default-seed an *unprimed* pool with GitHub's documented hourly cap before
    /// the first response sets the live value (`design/13 §3.3`/§3.9). These are
    /// expectations, NOT the authoritative number — [`observe_headers`] overwrites
    /// them from the live `X-RateLimit-*` headers on the first call.
    ///
    /// [`observe_headers`]: RateLimitBudget::observe_headers
    pub fn seed(limit: u32) -> Self {
        Self {
            limit,
            remaining: limit,
            reset_at: 0,
        }
    }

    /// Seed for a PAT pool (5000/hr, `design/13 §3.3`).
    pub fn seed_pat() -> Self {
        Self::seed(5000)
    }

    /// Seed for a GitHub App installation pool (15000/hr, `design/13 §3.3`).
    pub fn seed_app() -> Self {
        Self::seed(15000)
    }

    /// Update the budget from a response's `X-RateLimit-*` headers
    /// (`design/13 §3.9`: "seed budgets from real headers, degrade off the seeded
    /// state"). The headers are authoritative — Enterprise / secondary limits
    /// differ from the 5000/15000 defaults, so a present header always wins. A
    /// missing/malformed header leaves that field untouched (a non-API response,
    /// e.g. a 5xx with no body, must not zero the budget).
    ///
    /// Generic over the header lookup so both an `http::HeaderMap` (octocrab's
    /// raw response) and the testkit's `(name, value)` pairs can drive it without
    /// coupling this crate to a header type.
    pub fn observe_headers<'a, F>(&mut self, get: F)
    where
        F: Fn(&str) -> Option<&'a str>,
    {
        if let Some(v) = get("x-ratelimit-limit").and_then(|s| s.trim().parse::<u32>().ok()) {
            self.limit = v;
        }
        if let Some(v) = get("x-ratelimit-remaining").and_then(|s| s.trim().parse::<u32>().ok()) {
            self.remaining = v;
        }
        if let Some(v) = get("x-ratelimit-reset").and_then(|s| s.trim().parse::<i64>().ok()) {
            self.reset_at = v;
        }
    }

    /// Fraction of the budget still available (`remaining / limit`), `1.0` for an
    /// unprimed/zero-limit pool (we never report a fresh pool as degraded).
    pub fn fraction_remaining(&self) -> f64 {
        if self.limit == 0 {
            1.0
        } else {
            self.remaining as f64 / self.limit as f64
        }
    }

    /// `design/13 §3.9` degrade trigger: `< 10%` remaining (and not yet
    /// exhausted — an exhausted pool is handled by [`is_exhausted`], which fails
    /// the call rather than merely degrading it).
    ///
    /// [`is_exhausted`]: RateLimitBudget::is_exhausted
    pub fn is_degraded(&self) -> bool {
        self.fraction_remaining() < DEGRADE_FRACTION
    }

    /// `design/13 §3.9`/§5.3 warning trigger: `< 20%` remaining.
    pub fn is_warning(&self) -> bool {
        self.fraction_remaining() < WARN_FRACTION
    }

    /// `design/13 §8` exhaustion: `remaining == 0` (the `403` with
    /// `X-RateLimit-Remaining: 0`). Calls on an exhausted pool fail with
    /// [`rate_limited`] and queue for resume on reset.
    pub fn is_exhausted(&self) -> bool {
        self.remaining == 0
    }

    /// The reset time as **epoch milliseconds** (the unit the `RateLimited`
    /// error + the gRPC `RESOURCE_EXHAUSTED` hint carry). The budget's own
    /// `reset_at` is epoch **seconds** (the `X-RateLimit-Reset` header unit, the
    /// FROZEN 313 field); this converts for the error/handoff boundary.
    pub fn reset_at_ms(&self) -> i64 {
        self.reset_at.saturating_mul(1000)
    }
}

// ---------------------------------------------------------------------------
// The typed `RateLimited{reset_at}` error (mirrors 313's `no_vcs_credentials`
// prefix convention; `concerto-error` is FROZEN/out-of-scope).
// ---------------------------------------------------------------------------

/// Stable wire-code prefix the gRPC handler matches to map a rate-limit failure
/// onto `Code::ResourceExhausted` + the reset hint (`design/13 §8`).
const RATE_LIMITED_PREFIX: &str = "vcs.rate_limited";

/// Build the typed `RateLimited{reset_at}` error (`design/13 §3.9`/§8). Reuses
/// the FROZEN `Error::Vcs` variant (the `concerto-error` enum is out-of-scope)
/// with a stable `vcs.rate_limited reset_at=<epoch-ms>` payload the gRPC handler
/// switches on (→ `RESOURCE_EXHAUSTED` + the reset hint) and the queue/resume
/// path keys off. `reset_at_ms` is epoch milliseconds.
pub fn rate_limited(reset_at_ms: i64) -> Error {
    Error::Vcs(format!(
        "{RATE_LIMITED_PREFIX} reset_at={reset_at_ms}: rate limit exhausted; \
         the call will be queued and resume on reset"
    ))
}

/// True when `e` is a [`rate_limited`] error.
pub fn is_rate_limited(e: &Error) -> bool {
    matches!(e, Error::Vcs(m) if m.starts_with(RATE_LIMITED_PREFIX))
}

/// Extract the `reset_at` (epoch ms) carried by a [`rate_limited`] error, if any.
/// The gRPC handler uses it to populate the `RESOURCE_EXHAUSTED` reset hint.
pub fn rate_limited_reset_at(e: &Error) -> Option<i64> {
    match e {
        Error::Vcs(m) if m.starts_with(RATE_LIMITED_PREFIX) => m
            .split("reset_at=")
            .nth(1)
            .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|digits| digits.parse::<i64>().ok()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Op priority gate (`design/13 §3.9`: background ops yield to user-driven ones
// under degradation).
// ---------------------------------------------------------------------------

/// Whether an op is user-driven (create PR, merge) or background (deployment /
/// review-thread polls). Under a degraded pool, background ops yield to
/// user-driven ones on the **same** pool (`design/13 §3.9`). A simple priority
/// gate threaded from the call site — not a full scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpPriority {
    /// User-driven: create PR, merge. Never deprioritized.
    UserDriven,
    /// Background: deployment / review-thread polling. Yields under degradation.
    Background,
}

impl OpPriority {
    /// True when this op should yield (be skipped/deferred) given the pool's
    /// degraded state. User-driven ops never yield; background ops yield while
    /// the pool is degraded (`design/13 §3.9`).
    pub fn should_yield(self, degraded: bool) -> bool {
        matches!(self, OpPriority::Background) && degraded
    }
}

// ---------------------------------------------------------------------------
// Per-pool resume queue (`design/13 §3.9`/§8: exhausted → queue + resume on
// reset).
// ---------------------------------------------------------------------------

/// A small per-pool queue of background ops parked on exhaustion, resumed once
/// the (synthetic or wall) clock passes the pool's `reset_at` (`design/13 §3.9`).
/// User-driven calls surface the error to the caller instead of queueing; only
/// background work parks here.
///
/// This is the *bookkeeping* seam — it records that a labelled op is waiting and
/// when it may resume, and reports which ops are eligible at a given `now`. The
/// actual re-dispatch is the caller's (the poll loop drains [`drain_ready`] each
/// tick). Cheap-clone (`Arc<Mutex<…>>`) so the dispatcher shares one queue per
/// pool.
#[derive(Clone, Default)]
pub struct ResumeQueue {
    /// `(label, resume_at_secs)` — the parked op + the earliest epoch-second it
    /// may resume (the pool's `reset_at`).
    inner: Arc<Mutex<Vec<(String, i64)>>>,
}

impl ResumeQueue {
    /// Fresh, empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Park `label`, eligible to resume at `resume_at_secs` (the pool's
    /// `reset_at`, epoch seconds).
    pub fn park(&self, label: impl Into<String>, resume_at_secs: i64) {
        self.inner
            .lock()
            .expect("resume queue mutex")
            .push((label.into(), resume_at_secs));
    }

    /// Number of ops still parked.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("resume queue mutex").len()
    }

    /// True when nothing is parked.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Remove + return the labels whose `resume_at` has passed at `now_secs`
    /// (the synthetic clock in tests, the wall clock in production). Parked ops
    /// not yet eligible stay queued.
    pub fn drain_ready(&self, now_secs: i64) -> Vec<String> {
        let mut q = self.inner.lock().expect("resume queue mutex");
        let mut ready = Vec::new();
        q.retain(|(label, resume_at)| {
            if now_secs >= *resume_at {
                ready.push(label.clone());
                false
            } else {
                true
            }
        });
        ready
    }
}

// ---------------------------------------------------------------------------
// The three-pool tracker (`design/13 §4`: `rate_limits: HashMap<ProviderKey,
// RateLimitBudget>`).
// ---------------------------------------------------------------------------

/// A `vcs.rate_limit_warning` broadcast payload (`design/13 §5.3`). Fired once
/// per pool per warning-threshold crossing (debounced — it does NOT spam every
/// call below 20%). The Core's event bus serializes this onto the broadcast
/// stream; the diagnostics path reads the same pool state via
/// [`RateLimitPools::snapshot`].
#[derive(Debug, Clone, PartialEq)]
pub struct RateLimitWarning {
    /// The provider (`github` / `gh`) the pool belongs to.
    pub provider: String,
    /// The pool scope: the app id for an App pool, `pat` / `gh` otherwise.
    pub scope_id: String,
    /// Reset time, epoch milliseconds (the gRPC hint unit).
    pub reset_at_ms: i64,
    /// Fraction still remaining when the warning crossed (for the UI text).
    pub fraction_remaining: f64,
}

impl ProviderKey {
    /// The `(provider, scope_id)` pair the [`RateLimitWarning`] payload + the
    /// diagnostics accessor report for this pool (`design/13 §5.3`).
    pub fn warning_identity(&self) -> (String, String) {
        match self {
            ProviderKey::GithubPat => ("github".to_string(), "pat".to_string()),
            ProviderKey::GithubApp(app_id) => ("github".to_string(), app_id.clone()),
            ProviderKey::GhCli => ("gh".to_string(), "gh".to_string()),
        }
    }
}

/// The three independent rate-limit pools (`design/13 §3.9`/§4) — the populated
/// form of 313's `VcsState.rate_limits` skeleton. Keyed strictly off
/// [`ProviderKey`] so an App pool, a PAT pool, and a `gh` pool are tracked
/// **independently**: draining the PAT pool never degrades the App pool.
///
/// Owns the warning debounce (so a pool below 20% emits `vcs.rate_limit_warning`
/// exactly once per crossing) and a per-pool [`ResumeQueue`]. Cheap-clone via
/// the inner `Arc<Mutex<…>>` so the dispatcher + the diagnostics accessor share
/// one tracker.
#[derive(Clone, Default)]
pub struct RateLimitPools {
    inner: Arc<Mutex<PoolsInner>>,
}

#[derive(Default)]
struct PoolsInner {
    budgets: HashMap<ProviderKey, RateLimitBudget>,
    /// Pools currently flagged "warning emitted" — cleared when the pool
    /// recovers above 20%, so a later re-crossing re-emits (debounce per
    /// crossing, not forever).
    warned: HashMap<ProviderKey, bool>,
    resume_queues: HashMap<ProviderKey, ResumeQueue>,
}

impl RateLimitPools {
    /// Fresh tracker with no pools primed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ensure `key`'s pool exists, default-seeding it from the documented hourly
    /// cap if unprimed (App = 15000, PAT = 5000, `gh` = 5000; the header
    /// overwrites the seed on the first call). Returns the current budget.
    pub fn ensure(&self, key: &ProviderKey) -> RateLimitBudget {
        let mut g = self.inner.lock().expect("pools mutex");
        *g.budgets.entry(key.clone()).or_insert_with(|| match key {
            ProviderKey::GithubApp(_) => RateLimitBudget::seed_app(),
            ProviderKey::GithubPat | ProviderKey::GhCli => RateLimitBudget::seed_pat(),
        })
    }

    /// Read a pool's current budget without priming it.
    pub fn get(&self, key: &ProviderKey) -> Option<RateLimitBudget> {
        self.inner
            .lock()
            .expect("pools mutex")
            .budgets
            .get(key)
            .copied()
    }

    /// Whether `key`'s pool is currently degraded (`< 10%`). Unprimed pools are
    /// not degraded. The dispatcher reads this to double cadence + gate
    /// background ops on this pool.
    pub fn is_degraded(&self, key: &ProviderKey) -> bool {
        self.get(key).map(|b| b.is_degraded()).unwrap_or(false)
    }

    /// The per-pool resume queue (created on first access).
    pub fn resume_queue(&self, key: &ProviderKey) -> ResumeQueue {
        let mut g = self.inner.lock().expect("pools mutex");
        g.resume_queues.entry(key.clone()).or_default().clone()
    }

    /// Seed/update `key`'s pool from a response's `X-RateLimit-*` headers and
    /// return any [`RateLimitWarning`] that should be broadcast on **this** call
    /// (i.e. the pool just crossed below 20% and had not already warned). The
    /// caller broadcasts the returned warning on the `vcs.rate_limit_warning`
    /// stream. Crossing back above 20% re-arms the debounce.
    ///
    /// `get` is the same generic header lookup [`RateLimitBudget::observe_headers`]
    /// takes, so an `http::HeaderMap` or the testkit pairs both drive it.
    pub fn observe<'a, F>(&self, key: &ProviderKey, get: F) -> Option<RateLimitWarning>
    where
        F: Fn(&str) -> Option<&'a str>,
    {
        let mut g = self.inner.lock().expect("pools mutex");
        let budget = g.budgets.entry(key.clone()).or_insert_with(|| match key {
            ProviderKey::GithubApp(_) => RateLimitBudget::seed_app(),
            ProviderKey::GithubPat | ProviderKey::GhCli => RateLimitBudget::seed_pat(),
        });
        budget.observe_headers(get);
        let warning = budget.is_warning();
        let snapshot = *budget;
        let already_warned = g.warned.get(key).copied().unwrap_or(false);
        if !warning {
            // Recovered above 20% → re-arm the debounce.
            g.warned.insert(key.clone(), false);
            return None;
        }
        if already_warned {
            // Still below 20% but we've already warned for this crossing.
            return None;
        }
        g.warned.insert(key.clone(), true);
        let (provider, scope_id) = key.warning_identity();
        Some(RateLimitWarning {
            provider,
            scope_id,
            reset_at_ms: snapshot.reset_at_ms(),
            fraction_remaining: snapshot.fraction_remaining(),
        })
    }

    /// A read-only snapshot of every pool's `(key, budget)` for the Settings →
    /// Diagnostics read accessor (`design/13 §3.9` "soft warning in Settings →
    /// Diagnostics"). The full UI / RPC is Task 324 / 709; this is the data
    /// source they call. Sorted by the pool's `(provider, scope_id)` for a
    /// stable diagnostics ordering.
    pub fn snapshot(&self) -> Vec<(ProviderKey, RateLimitBudget)> {
        let g = self.inner.lock().expect("pools mutex");
        let mut out: Vec<(ProviderKey, RateLimitBudget)> =
            g.budgets.iter().map(|(k, v)| (k.clone(), *v)).collect();
        out.sort_by_key(|(k, _)| k.warning_identity());
        out
    }
}
