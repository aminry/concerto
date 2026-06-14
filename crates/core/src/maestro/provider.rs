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
use concerto_keychain as keychain;

use crate::security::managed::ManagedPolicy;

/// Default Maestro model when no org-pinned `defaultModel` is present
/// (design/08 R-1: Sonnet-class default).
pub const DEFAULT_MAESTRO_MODEL: &str = "claude-sonnet-4-6";

/// Fallback Claude CLI binary name resolved on `$PATH` when no org-pinned
/// `claudeExecutablePath` is configured.
pub const DEFAULT_CLAUDE_BIN: &str = "claude";

/// Fallback Codex CLI binary name resolved on `$PATH` when no org-pinned
/// `codexExecutablePath` is configured (Task 412).
pub const DEFAULT_CODEX_BIN: &str = "codex";

/// Fallback Gemini CLI binary name resolved on `$PATH` when no org-pinned
/// `geminiExecutablePath` is configured (Task 412).
pub const DEFAULT_GEMINI_BIN: &str = "gemini";

/// Stable marker embedded in the [`Error::Validation`] every
/// [`DirectApiProvider`] method returns. A caller (or the fast-follow that
/// fills the native function-call loop) distinguishes "Direct-API not wired
/// yet" from a real provider error by testing for this prefix via
/// [`is_direct_api_unimplemented`] — mirroring 313's `unimplemented_err` /
/// `is_unimplemented` discipline. NEVER `unimplemented!()`/`todo!()`.
pub const DIRECT_API_UNIMPLEMENTED_MARKER: &str = "maestro.direct_api.unimplemented";

/// Build the typed [`Error::Validation`] the frozen-unwired Direct-API seam
/// returns (Task 412, D1). `what` names the call site for the operator log.
pub fn direct_api_unimplemented(what: &str) -> Error {
    Error::Validation(format!(
        "{DIRECT_API_UNIMPLEMENTED_MARKER}: {what} — the Direct-API Maestro \
         backend is a frozen-unwired Tier-1 seam (Task 412)"
    ))
}

/// `true` when `err` is the frozen-unwired Direct-API marker (a positive
/// predicate so callers can branch on "not wired yet" without string-matching
/// at every site).
pub fn is_direct_api_unimplemented(err: &Error) -> bool {
    matches!(err, Error::Validation(msg) if msg.contains(DIRECT_API_UNIMPLEMENTED_MARKER))
}

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

        Ok(resolve_cli_launch_spec(ctx, bin))
    }
}

/// Resolve the model the Maestro CLI launches with: the org-pinned
/// `defaultModel`, else the Sonnet default (design/08 R-1). Shared by every CLI
/// provider + the Direct-API seam's constructor.
fn resolve_model(ctx: &MaestroLaunchContext) -> String {
    ctx.managed
        .default_model()
        .map(|m| m.to_string())
        .unwrap_or_else(|| DEFAULT_MAESTRO_MODEL.to_string())
}

/// Build the CLI [`MaestroLaunchSpec`] shared by Claude/Codex/Gemini (Task
/// 412): the three live CLI backends differ ONLY in the resolved binary; the
/// model, the 401 `--mcp-config` endpoint, `--strict-mcp-config`, the Maestro
/// preamble, strict permission mode, and the scratch cwd are identical (they
/// flow through the same `start_session` spawn shape per PHASE4_PLANNING §4.3).
/// NO `--dangerously-skip-permissions`: the Maestro runs permission_mode=strict
/// and every tool call is gated by the PermissionResolver (reads auto-approved
/// via ToolClass::ReadOnly, writes surfaced as confirmation chips).
fn resolve_cli_launch_spec(ctx: &MaestroLaunchContext, bin: String) -> MaestroLaunchSpec {
    let model = resolve_model(ctx);
    let mcp_config = ctx.mcp_config_path.to_string_lossy().into_owned();

    let args = vec![
        "--model".to_string(),
        model.clone(),
        // `--input-format stream-json` REQUIRES `--print` (claude CLI). With
        // stream-json input, `--print` is the *streaming* multi-turn mode: it
        // reads a stream of newline-delimited user-message envelopes from stdin
        // and stays alive responding to each — exactly the long-lived Maestro
        // session model (NOT the one-shot `-p "<prompt>"` form).
        "--print".to_string(),
        "--input-format".to_string(),
        "stream-json".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--mcp-config".to_string(),
        mcp_config,
        "--strict-mcp-config".to_string(),
        // M1: auto-approve the live read tools (whole server; the 5 write + 2
        // side-channel tools return typed-unimplemented, so nothing to gate yet).
        // M2 replaces this with --permission-prompt-tool → PermissionResolver chips.
        "--allowedTools".to_string(),
        "mcp__concerto-maestro-mcp".to_string(),
        "--append-system-prompt".to_string(),
        MAESTRO_PREAMBLE.to_string(),
    ];

    MaestroLaunchSpec {
        bin,
        args,
        model,
        preamble: MAESTRO_PREAMBLE.to_string(),
        mcp_config_path: ctx.mcp_config_path.clone(),
        strict_mcp_config: true,
        permission_mode: MAESTRO_PERMISSION_MODE.to_string(),
        scratch_cwd: ctx.scratch_cwd.clone(),
    }
}

/// LIVE Codex-CLI provider (Task 412). Same `start_session` spawn shape as
/// [`ClaudeCliProvider`], differing only in the resolved binary: the org-pinned
/// `codexExecutablePath`, else `codex` on `$PATH`.
#[derive(Debug, Clone, Default)]
pub struct CodexCliProvider {
    _private: (),
}

impl CodexCliProvider {
    /// Construct the live Codex-CLI provider.
    pub fn new() -> Self {
        Self::default()
    }
}

impl MaestroProvider for CodexCliProvider {
    fn resolve_launch(&self, ctx: &MaestroLaunchContext) -> Result<MaestroLaunchSpec> {
        let bin = ctx
            .managed
            .codex_executable_path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| DEFAULT_CODEX_BIN.to_string());
        Ok(resolve_cli_launch_spec(ctx, bin))
    }
}

/// LIVE Gemini-CLI provider (Task 412). Same `start_session` spawn shape as
/// [`ClaudeCliProvider`], differing only in the resolved binary: the org-pinned
/// `geminiExecutablePath`, else `gemini` on `$PATH`.
#[derive(Debug, Clone, Default)]
pub struct GeminiCliProvider {
    _private: (),
}

impl GeminiCliProvider {
    /// Construct the live Gemini-CLI provider.
    pub fn new() -> Self {
        Self::default()
    }
}

impl MaestroProvider for GeminiCliProvider {
    fn resolve_launch(&self, ctx: &MaestroLaunchContext) -> Result<MaestroLaunchSpec> {
        let bin = ctx
            .managed
            .gemini_executable_path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| DEFAULT_GEMINI_BIN.to_string());
        Ok(resolve_cli_launch_spec(ctx, bin))
    }
}

/// The frozen-unwired Direct-API provider arm — **412-owned** (D1, design/08
/// §3.9). Ships as a documented seam: every [`MaestroProvider`] method returns
/// the typed [`direct_api_unimplemented`] marker (never `unimplemented!()`/
/// `todo!()`, never an empty-success spec) so a caller that wires it before the
/// fast-follow lands fails loudly + recognizably (via
/// [`is_direct_api_unimplemented`]).
///
/// The constructor ALREADY reads the future request's inputs —
/// [`keychain::Provider`] (whose [`keychain::SecretKind::ProviderToken`] the
/// fast-follow looks up in the OS keychain) + the resolved
/// [`ManagedPolicy::default_model`] — so filling the native function-call loop
/// (Anthropic/OpenAI/Bedrock/Vertex/Foundry/OpenRouter request + tool-call +
/// stream) is a **body-only** change against this unchanged frozen seam.
#[derive(Debug, Clone)]
pub struct DirectApiProvider {
    /// The cloud LLM provider whose [`keychain::SecretKind::ProviderToken`] the
    /// fast-follow reads for the native request. Frozen here so the seam's
    /// shape is visible; not yet dialed.
    provider: keychain::Provider,
    /// The resolved model for the future Direct-API request (org-pinned
    /// `defaultModel`, else the Sonnet default). `None` only when no context is
    /// available at construction.
    model: Option<String>,
}

impl DirectApiProvider {
    /// Construct the (frozen-unwired) Direct-API provider for `provider`,
    /// resolving the model from `ctx`'s managed policy so the fast-follow has
    /// the request inputs without touching the seam. Reads
    /// [`keychain::SecretKind::ProviderToken`] indirectly: the stored
    /// `provider` is the keychain lookup key the body-only fast-follow uses.
    pub fn new(provider: keychain::Provider, ctx: &MaestroLaunchContext) -> Self {
        Self {
            provider,
            model: Some(resolve_model(ctx)),
        }
    }

    /// The cloud provider this Direct-API arm targets (the keychain
    /// [`keychain::SecretKind::ProviderToken`] lookup key for the fast-follow).
    pub fn provider(&self) -> keychain::Provider {
        self.provider
    }

    /// The resolved model the future Direct-API request will use.
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
}

impl MaestroProvider for DirectApiProvider {
    fn resolve_launch(&self, _ctx: &MaestroLaunchContext) -> Result<MaestroLaunchSpec> {
        // 412-owned: the Direct-API native function-call loop has no precedent
        // in the codebase and ships as a frozen-unwired Tier-1 seam (D1). Return
        // the typed marker — NOT the macro, NOT an empty success.
        Err(direct_api_unimplemented("resolve_launch"))
    }
}

/// The four Maestro LLM backends (Task 412, design/08 §3.9). The three CLI
/// backends ship LIVE; `Direct` is the FROZEN unwired seam (D1).
///
/// NET-NEW here — 402 froze the [`MaestroProvider`] trait + a
/// [`ClaudeCliProvider`] *struct* (no backend enum); this enum's `Claude`
/// variant maps to 402's [`ClaudeCliProvider`], `Codex`/`Gemini` to 412's live
/// providers, and `Direct` to the frozen [`DirectApiProvider`]. The variant set
/// is FROZEN by 412.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaestroBackend {
    /// Claude CLI (402's live `ClaudeCliProvider`).
    Claude,
    /// Codex CLI (412's live `CodexCliProvider`).
    Codex,
    /// Gemini CLI (412's live `GeminiCliProvider`).
    Gemini,
    /// Direct-API native function-call loop (FROZEN unwired seam, D1).
    Direct,
}

impl MaestroBackend {
    /// `true` for an external cloud API backend — `Direct` only (the CLI
    /// backends use the user's own CLI auth, so `enterpriseDataPrivacy` does
    /// not gate them). Used by [`select_provider`]'s privacy interlock.
    pub fn is_external_api(self) -> bool {
        matches!(self, MaestroBackend::Direct)
    }
}

/// The outcome of [`select_provider`]: either a chosen backend, or a typed
/// reason the Maestro is not selectable (surfaced as disabled by 414, NOT a
/// panic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderSelection {
    /// A backend was selected (auto-pick or honored user override).
    Selected(MaestroBackend),
    /// `enterpriseDataPrivacy=true` selected an external Direct backend
    /// (design/08 §3.10). 414 publishes `maestro.disabled_by_policy`.
    DisabledByPolicy,
}

/// Auto-pick the Maestro backend in the design/08 §3.9 order `Claude → Codex →
/// Gemini → Direct`: the first CLI whose binary resolves (org-pinned managed
/// path or bare name on `$PATH`), falling through to `Direct` only when a
/// `ProviderToken` is configured. A user override wins outright.
///
/// Privacy interlock (consumes 413's resolved decision, does not re-own it):
/// when `enterprise_data_privacy` is true the selector MUST NOT land on the
/// external `Direct` backend — it returns [`ProviderSelection::DisabledByPolicy`]
/// (414 publishes `maestro.disabled_by_policy`). The CLI backends are
/// unaffected (they use the user's own CLI auth).
///
/// `cli_resolves` reports whether a given CLI backend's binary is launchable
/// (injected so tests script availability without a real `$PATH`);
/// `direct_token_present` reports whether a `ProviderToken` exists for the
/// Direct fallback. Returns a typed [`Error::Validation`] when no backend is
/// configured (surfaced as disabled, never a panic).
pub fn select_provider(
    enterprise_data_privacy: bool,
    override_backend: Option<MaestroBackend>,
    cli_resolves: impl Fn(MaestroBackend) -> bool,
    direct_token_present: bool,
) -> Result<ProviderSelection> {
    // A user override wins outright — but the privacy interlock still applies
    // to an external Direct override.
    if let Some(backend) = override_backend {
        if backend.is_external_api() && enterprise_data_privacy {
            return Ok(ProviderSelection::DisabledByPolicy);
        }
        return Ok(ProviderSelection::Selected(backend));
    }

    // Auto-pick the first available CLI in the design/08 §3.9 order.
    for backend in [
        MaestroBackend::Claude,
        MaestroBackend::Codex,
        MaestroBackend::Gemini,
    ] {
        if cli_resolves(backend) {
            return Ok(ProviderSelection::Selected(backend));
        }
    }

    // No CLI resolved — fall through to Direct only when a token exists.
    if direct_token_present {
        if enterprise_data_privacy {
            // External Direct under enterprise privacy is not selectable.
            return Ok(ProviderSelection::DisabledByPolicy);
        }
        return Ok(ProviderSelection::Selected(MaestroBackend::Direct));
    }

    // Nothing configured: the Maestro is unconfigured (typed error, surfaced
    // as disabled by 414 — NOT a panic).
    Err(Error::Validation(
        "maestro.provider.unconfigured: no Maestro backend is available \
         (no Claude/Codex/Gemini CLI on PATH and no Direct-API ProviderToken)"
            .to_string(),
    ))
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
    fn maestro_launch_is_headless_stream_json_with_read_tools_allowed() {
        let spec = ClaudeCliProvider::new()
            .resolve_launch(&ctx_with(ManagedPolicy::default()))
            .expect("spec");
        // `--input-format stream-json` REQUIRES `--print` (claude CLI errors out
        // without it); `--print` + stream-json input is the streaming multi-turn
        // mode, not the one-shot prompt form.
        assert!(spec.args.iter().any(|a| a == "--print"));
        assert!(spec
            .args
            .windows(2)
            .any(|w| w == ["--input-format", "stream-json"]));
        assert!(spec
            .args
            .windows(2)
            .any(|w| w == ["--output-format", "stream-json"]));
        assert!(spec.args.iter().any(|a| a == "--verbose"));
        assert!(spec
            .args
            .windows(2)
            .any(|w| w == ["--allowedTools", "mcp__concerto-maestro-mcp"]));
        assert!(spec.args.iter().any(|a| a == "--strict-mcp-config"));
        assert!(!spec
            .args
            .iter()
            .any(|a| a == "--dangerously-skip-permissions"));
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
    fn codex_provider_resolves_codex_bin_with_same_strict_spawn_shape() {
        let provider = CodexCliProvider::new();
        let spec = provider
            .resolve_launch(&ctx_with(ManagedPolicy::default()))
            .expect("default managed policy resolves a Codex launch spec");

        assert_eq!(spec.bin, DEFAULT_CODEX_BIN);
        assert_eq!(spec.model, DEFAULT_MAESTRO_MODEL);
        assert_eq!(spec.permission_mode, "strict");
        assert!(spec.strict_mcp_config);
        // Same frozen flag contract as Claude — differs only in the binary.
        assert!(spec.args.iter().any(|a| a == "--mcp-config"));
        assert!(spec.args.iter().any(|a| a == "--strict-mcp-config"));
        assert!(spec.args.iter().any(|a| a == MAESTRO_PREAMBLE));
        assert!(
            !spec
                .args
                .iter()
                .any(|a| a == "--dangerously-skip-permissions"),
            "the Maestro NEVER skips permissions (strict mode)"
        );
    }

    #[test]
    fn gemini_provider_resolves_gemini_bin() {
        let provider = GeminiCliProvider::new();
        let spec = provider
            .resolve_launch(&ctx_with(ManagedPolicy::default()))
            .expect("default managed policy resolves a Gemini launch spec");
        assert_eq!(spec.bin, DEFAULT_GEMINI_BIN);
        assert!(spec.strict_mcp_config);
    }

    #[test]
    fn direct_api_provider_returns_typed_marker_not_panic() {
        let ctx = ctx_with(ManagedPolicy::default());
        let provider = DirectApiProvider::new(keychain::Provider::Anthropic, &ctx);
        // The constructor already reads the future request inputs.
        assert_eq!(provider.provider(), keychain::Provider::Anthropic);
        assert_eq!(provider.model(), Some(DEFAULT_MAESTRO_MODEL));

        let err = provider
            .resolve_launch(&ctx)
            .expect_err("the Direct-API arm is a frozen-unwired seam");
        assert!(
            is_direct_api_unimplemented(&err),
            "expected the typed direct_api_unimplemented marker, got {err:?}"
        );
        match err {
            Error::Validation(msg) => {
                assert!(msg.contains(DIRECT_API_UNIMPLEMENTED_MARKER));
            }
            other => panic!("expected a typed Validation error, got {other:?}"),
        }
    }

    #[test]
    fn select_provider_auto_picks_claude_codex_gemini_direct_in_order() {
        // Claude available ⇒ Claude wins.
        assert_eq!(
            select_provider(false, None, |b| b == MaestroBackend::Claude, false).unwrap(),
            ProviderSelection::Selected(MaestroBackend::Claude)
        );
        // Only Codex available ⇒ Codex.
        assert_eq!(
            select_provider(false, None, |b| b == MaestroBackend::Codex, false).unwrap(),
            ProviderSelection::Selected(MaestroBackend::Codex)
        );
        // Only Gemini available ⇒ Gemini.
        assert_eq!(
            select_provider(false, None, |b| b == MaestroBackend::Gemini, false).unwrap(),
            ProviderSelection::Selected(MaestroBackend::Gemini)
        );
        // No CLI but a Direct token ⇒ Direct.
        assert_eq!(
            select_provider(false, None, |_| false, true).unwrap(),
            ProviderSelection::Selected(MaestroBackend::Direct)
        );
    }

    #[test]
    fn select_provider_user_override_wins() {
        // Claude is available but the user pinned Gemini.
        assert_eq!(
            select_provider(false, Some(MaestroBackend::Gemini), |_| true, true).unwrap(),
            ProviderSelection::Selected(MaestroBackend::Gemini)
        );
    }

    #[test]
    fn select_provider_no_backend_is_typed_error_not_panic() {
        let err = select_provider(false, None, |_| false, false)
            .expect_err("no CLI and no token ⇒ unconfigured");
        match err {
            Error::Validation(msg) => assert!(msg.contains("maestro.provider.unconfigured")),
            other => panic!("expected a typed Validation error, got {other:?}"),
        }
    }

    #[test]
    fn select_provider_external_direct_under_privacy_is_disabled_by_policy() {
        // Auto-pick fallthrough to Direct under enterprise privacy ⇒ disabled.
        assert_eq!(
            select_provider(true, None, |_| false, true).unwrap(),
            ProviderSelection::DisabledByPolicy
        );
        // An explicit Direct override under privacy is also disabled.
        assert_eq!(
            select_provider(true, Some(MaestroBackend::Direct), |_| true, true).unwrap(),
            ProviderSelection::DisabledByPolicy
        );
        // A CLI backend is unaffected by enterprise privacy.
        assert_eq!(
            select_provider(true, None, |b| b == MaestroBackend::Claude, false).unwrap(),
            ProviderSelection::Selected(MaestroBackend::Claude)
        );
    }
}
