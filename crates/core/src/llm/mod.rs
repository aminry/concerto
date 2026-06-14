//! LLM seams.
//!
//! Owns [`oneshot`] — the FROZEN `OneShotLlm` trait + `OneShotRequest` +
//! `ActionKind` + the live `DeterministicOneShot` impl + `compose_action_prompt`
//! (`PHASE3_PLANNING §4.4`). Per **D1** the deterministic path is the LIVE
//! Phase-3 path; the pluggable real-LLM provider is an unwired seam supplied in
//! Phase 4 (Task 412). Task 321 reuses this module for PR title/body with no
//! new machinery.
//!
//! Owns [`provider`] (Task 412, design/08 §3.9 / PHASE4_PLANNING §4.6 / D6) —
//! the net-new daily [`provider::TokenBudget`] + [`provider::parse_token_usage`]
//! carrier + the inert-on-exhaust [`provider::MaestroLlmState`]. This is the
//! Maestro *interactive-agent* budget; it is distinct from the `OneShotLlm`
//! summarizer/digest path (D5). The provider-**selection** seam (which CLI to
//! launch) lives in `crate::maestro::provider`.

pub mod oneshot;
// `provider` is the Maestro interactive-agent token budget; it imports
// `crate::maestro::provider::MaestroBackend`, and `maestro` is `cfg(unix)`
// (it sits over the `cfg(unix)` agent supervisor). Gate the whole module to
// match so the non-unix (Windows) build — which has no `crate::maestro` — still
// compiles. Nothing outside the Maestro subsystem consumes these symbols.
#[cfg(unix)]
pub mod provider;

pub use oneshot::{
    compose_action_prompt, ActionKind, DeterministicOneShot, OneShotLlm, OneShotRequest,
};
#[cfg(unix)]
pub use provider::{
    next_utc_midnight_ms, parse_token_usage, InertReason, MaestroLlmState, StaleDigest,
    TokenBudget, DEFAULT_DAILY_IN_CAP, DEFAULT_DAILY_OUT_CAP,
};
