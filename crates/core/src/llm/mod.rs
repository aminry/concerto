//! One-shot LLM seam (Task 312).
//!
//! Owns [`oneshot`] — the FROZEN `OneShotLlm` trait + `OneShotRequest` +
//! `ActionKind` + the live `DeterministicOneShot` impl + `compose_action_prompt`
//! (`PHASE3_PLANNING §4.4`). Per **D1** the deterministic path is the LIVE
//! Phase-3 path; the pluggable real-LLM provider is an unwired seam supplied in
//! Phase 4 (Task 412). Task 321 reuses this module for PR title/body with no
//! new machinery.

pub mod oneshot;

pub use oneshot::{
    compose_action_prompt, ActionKind, DeterministicOneShot, OneShotLlm, OneShotRequest,
};
