//! The Maestro **provider-selection seam** (Task 402, PHASE4_PLANNING §4.3 /
//! D1/D5, design/08 §3.9).
//!
//! This is the interactive-agent backend seam: it resolves which CLI binary,
//! model, Maestro preamble, `--mcp-config` endpoint, strict mode, and scratch
//! cwd to launch the long-lived Maestro session under the Agent Supervisor.
//! The supervisor's `resolve_agent_bin` Maestro arm delegates to a
//! [`MaestroProvider`] to obtain a [`MaestroLaunchSpec`].
//!
//! ## Frozen by 402, extended by 412
//!
//! 402 freezes the trait + the launch-spec shape with **`ClaudeCliProvider`
//! LIVE only**. Task 412 adds the Codex/Gemini LIVE providers + a
//! `DirectApiProvider` frozen-unwired arm (whose `resolve_launch` returns a
//! **typed** [`Error::Validation`], never `unimplemented!()`/`todo!()`, never an
//! empty-success spec) + the daily token budget. The field names/types here are
//! designed minimally + append-friendly and are FROZEN.
//!
//! ## Distinct from `OneShotLlm` (D5)
//!
//! Do **not** conflate this with `OneShotLlm` (Task 312, `suggest(req)->String`),
//! which is the one-shot summarizer/digest path (404/409). The Maestro chat
//! agent needs a launch *spec* (binary / model / preamble / mcp-config), not a
//! string completion — that is why this is a separate trait.
//!
//! ## No `--dangerously-skip-permissions`
//!
//! The Maestro runs `permission_mode=strict`: every tool call is intercepted by
//! the `PermissionResolver` (reads auto-approved via `ToolClass::ReadOnly`,
//! writes surfaced as confirmation chips). The provider therefore NEVER emits
//! `--dangerously-skip-permissions`; instead it relies on the strict resolver +
//! `--strict-mcp-config` (so ONLY the 401 Maestro tools are visible).

use std::path::PathBuf;

use concerto_error::{Error, Result};

use crate::security::managed::ManagedPolicy;

/// Default Maestro model when no org-pinned `defaultModel` is present
/// (design/08 R-1: Sonnet-class default).
pub const DEFAULT_MAESTRO_MODEL: &str = "claude-4.6-sonnet";

/// Fallback Claude CLI binary name resolved on `$PATH` when no org-pinned
/// `claudeExecutablePath` is configured.
pub const DEFAULT_CLAUDE_BIN: &str = "claude";

/// The Maestro permission mode — always `"strict"` (design/08 §3.1 / D4). The
/// launch spec carries it as a string so the supervisor persists it verbatim on
/// the `sessions.permission_mode` row.
pub const MAESTRO_PERMISSION_MODE: &str = "strict";

/// Inputs a [`MaestroProvider`] reads to resolve a [`MaestroLaunchSpec`].
///
/// Carries the org-managed-policy view (the provider reads
/// [`ManagedPolicy::default_model`] / [`ManagedPolicy::claude_executable_path`]),
/// the resolved scratch directory (`~/concerto/maestro/`), and the path to
/// 401's `concerto-maestro-mcp` stdio MCP-config endpoint the spawned CLI dials
/// via `--mcp-config`.
///
/// FROZEN by 402; 412 reads the same context (it adds the Codex/Gemini
/// executable-path getters + the Direct-API key, all already on
/// [`ManagedPolicy`]) — append-only.
#[derive(Debug, Clone)]
pub struct MaestroLaunchContext {
    /// The org-managed policy view (model pin, CLI executable paths,
    /// enterprise-data-privacy). Read-only here.
    pub managed: ManagedPolicy,
    /// The Maestro scratch working directory (`~/concerto/maestro/`). NOT a
    /// worktree; the Maestro has no file-edit tools, so there is no edit-mutex.
    pub scratch_cwd: PathBuf,
    /// Path to 401's `concerto-maestro-mcp` stdio MCP-config file the spawned
    /// CLI dials via `--mcp-config`.
    pub mcp_config_path: PathBuf,
}

impl MaestroLaunchContext {
    /// Construct a launch context.
    pub fn new(managed: ManagedPolicy, scratch_cwd: PathBuf, mcp_config_path: PathBuf) -> Self {
        Self {
            managed,
            scratch_cwd,
            mcp_config_path,
        }
    }
}

/// The fully-resolved "how to launch the Maestro CLI" tuple (design/08 §3.9 /
/// PHASE4_PLANNING §4.3). FROZEN by 402; 412 builds the same shape for its
/// Codex/Gemini providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaestroLaunchSpec {
    /// The agent CLI binary to spawn (e.g. `"claude"`, or an org-pinned
    /// absolute path).
    pub bin: String,
    /// The full argument vector — includes `--mcp-config <endpoint>`,
    /// `--strict-mcp-config`, the model flag, and the Maestro-preamble flag.
    /// Never contains `--dangerously-skip-permissions`.
    pub args: Vec<String>,
    /// The selected model (default [`DEFAULT_MAESTRO_MODEL`], design/08 R-1).
    pub model: String,
    /// The Maestro preamble — replaces the default agent preamble for the
    /// Maestro session.
    pub preamble: String,
    /// 401's `concerto-maestro-mcp` stdio endpoint the CLI dials.
    pub mcp_config_path: PathBuf,
    /// `true` ⇒ ONLY the Maestro tools are visible (`--strict-mcp-config`).
    pub strict_mcp_config: bool,
    /// Always [`MAESTRO_PERMISSION_MODE`] (`"strict"`).
    pub permission_mode: String,
    /// The Maestro scratch cwd (`~/concerto/maestro/`).
    pub scratch_cwd: PathBuf,
}

/// The Maestro provider-selection trait. FROZEN by 402 (Claude live), extended
/// by 412 (Codex/Gemini live + the `DirectApiProvider` frozen-unwired arm).
pub trait MaestroProvider: Send + Sync {
    /// Resolve the launch spec for the Maestro session from `ctx`.
    fn resolve_launch(&self, ctx: &MaestroLaunchContext) -> Result<MaestroLaunchSpec>;
}

/// The Maestro preamble (design/08 §3.1). Replaces the default agent preamble:
/// it frames the CLI as Concerto's outer orchestration agent, points it at the
/// `concerto-maestro-mcp` tools, and reminds it that write tools require user
/// confirmation (the strict + chip gate).
pub const MAESTRO_PREAMBLE: &str = "\
You are Concerto's Maestro — the outer orchestration agent at the top of the \
app. You observe and route across the user's workspaces and workareas using \
ONLY the `concerto-maestro-mcp` tools (no filesystem, shell, or network \
access). Read tools (list/get/summary/search) run freely; write tools (route, \
fanout, create, pause) and side-channel actions require explicit user \
confirmation, which Concerto surfaces as a confirmation chip. Be concise; \
prefer routing work to the right workarea session over doing it yourself.";

/// LIVE Claude-CLI provider (Task 402). Resolves the Claude CLI binary + model
/// from [`ManagedPolicy`], emits the [`MAESTRO_PREAMBLE`], and dials 401's MCP
/// endpoint with `--strict-mcp-config`.
#[derive(Debug, Clone, Default)]
pub struct ClaudeCliProvider {
    _private: (),
}

impl ClaudeCliProvider {
    /// Construct the live Claude-CLI provider.
    pub fn new() -> Self {
        Self::default()
    }
}

impl MaestroProvider for ClaudeCliProvider {
    fn resolve_launch(&self, ctx: &MaestroLaunchContext) -> Result<MaestroLaunchSpec> {
        // Binary: the org-pinned Claude executable path, else `claude` on $PATH.
        let bin = ctx
            .managed
            .claude_executable_path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| DEFAULT_CLAUDE_BIN.to_string());

        // Model: the org-pinned default model, else the Sonnet default.
        let model = ctx
            .managed
            .default_model()
            .map(|m| m.to_string())
            .unwrap_or_else(|| DEFAULT_MAESTRO_MODEL.to_string());

        let mcp_config = ctx.mcp_config_path.to_string_lossy().into_owned();

        // Args: model + the 401 MCP endpoint (strict so ONLY Maestro tools are
        // visible) + the Maestro preamble. NO `--dangerously-skip-permissions`:
        // the Maestro runs permission_mode=strict and every tool call is gated
        // by the PermissionResolver (reads auto-approved via ToolClass::ReadOnly,
        // writes surfaced as confirmation chips).
        let args = vec![
            "--model".to_string(),
            model.clone(),
            "--mcp-config".to_string(),
            mcp_config,
            "--strict-mcp-config".to_string(),
            "--append-system-prompt".to_string(),
            MAESTRO_PREAMBLE.to_string(),
        ];

        Ok(MaestroLaunchSpec {
            bin,
            args,
            model,
            preamble: MAESTRO_PREAMBLE.to_string(),
            mcp_config_path: ctx.mcp_config_path.clone(),
            strict_mcp_config: true,
            permission_mode: MAESTRO_PERMISSION_MODE.to_string(),
            scratch_cwd: ctx.scratch_cwd.clone(),
        })
    }
}

/// The frozen-unwired Direct-API provider arm — **412-owned** (D1). 402 leaves
/// it as a documented seam returning a typed [`Error::Validation`] so a caller
/// that wires it before 412 lands fails loudly (never `unimplemented!()`/
/// `todo!()`, never an empty-success spec). 412 replaces this body with the
/// native function-call loop's launch resolution.
///
/// Kept here (not stubbed empty) so the seam's shape is visible to 412; it is
/// NOT registered as a live provider in 402.
#[derive(Debug, Clone, Default)]
pub struct DirectApiProvider {
    _private: (),
}

impl DirectApiProvider {
    /// Construct the (frozen-unwired) Direct-API provider.
    pub fn new() -> Self {
        Self::default()
    }
}

impl MaestroProvider for DirectApiProvider {
    fn resolve_launch(&self, _ctx: &MaestroLaunchContext) -> Result<MaestroLaunchSpec> {
        // 412-owned: the Direct-API native function-call loop has no precedent
        // in the codebase and ships as a frozen-unwired Tier-1 seam (D1). Return
        // a typed error — NOT the macro, NOT an empty success.
        Err(Error::Validation(
            "maestro.direct_api.unimplemented: the Direct-API Maestro backend is \
             a frozen-unwired seam (Task 412)"
                .to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with(managed: ManagedPolicy) -> MaestroLaunchContext {
        MaestroLaunchContext::new(
            managed,
            PathBuf::from("/home/user/concerto/maestro"),
            PathBuf::from("/home/user/concerto/maestro/.mcp.json"),
        )
    }

    #[test]
    fn claude_provider_emits_mcp_strict_and_model_without_skip_permissions() {
        let provider = ClaudeCliProvider::new();
        let spec = provider
            .resolve_launch(&ctx_with(ManagedPolicy::default()))
            .expect("default managed policy resolves a launch spec");

        assert_eq!(spec.bin, DEFAULT_CLAUDE_BIN);
        assert_eq!(spec.model, DEFAULT_MAESTRO_MODEL);
        assert_eq!(spec.permission_mode, "strict");
        assert!(spec.strict_mcp_config);
        assert_eq!(
            spec.scratch_cwd,
            PathBuf::from("/home/user/concerto/maestro")
        );

        // The frozen flag contract.
        assert!(spec.args.iter().any(|a| a == "--mcp-config"));
        assert!(spec.args.iter().any(|a| a == "--strict-mcp-config"));
        assert!(spec.args.iter().any(|a| a == &spec.model));
        assert!(
            !spec
                .args
                .iter()
                .any(|a| a == "--dangerously-skip-permissions"),
            "the Maestro NEVER skips permissions (strict mode)"
        );
        // The 401 endpoint is passed through.
        assert!(spec
            .args
            .iter()
            .any(|a| a == "/home/user/concerto/maestro/.mcp.json"));
        // The preamble is carried.
        assert!(!spec.preamble.is_empty());
        assert!(spec.args.iter().any(|a| a == MAESTRO_PREAMBLE));
    }

    #[test]
    fn direct_api_provider_returns_typed_err_not_panic() {
        let provider = DirectApiProvider::new();
        let err = provider
            .resolve_launch(&ctx_with(ManagedPolicy::default()))
            .expect_err("the Direct-API arm is a frozen-unwired seam");
        match err {
            Error::Validation(msg) => {
                assert!(msg.contains("maestro.direct_api.unimplemented"));
            }
            other => panic!("expected a typed Validation error, got {other:?}"),
        }
    }
}
