//! Maestro Agent subsystem — the **cluster-M root** (Task 401, design/08,
//! PHASE4_PLANNING §4.1).
//!
//! The Maestro is Concerto's "outer agent" — the chat at the top of the app
//! that dispatches, historians, plans, routes prompts to workareas, and surfaces
//! a digest on the user's return (design/08 §1). It runs as a long-lived
//! PTY-CLI session under the Agent Supervisor (`AgentKind::Maestro`, Task 402)
//! whose tools are served by the **first MCP server in the codebase**: the
//! in-process `concerto-maestro-mcp` stdio server ([`mcp`]).
//!
//! Task 401 lands the **skeleton + the MCP surface only**:
//!
//! - [`mcp`] — the in-process `rmcp` stdio MCP server (`concerto-maestro-mcp`) + the net-new Core↔CLI transport endpoint 402 dials. See [`mcp::serve_maestro_mcp`] / [`mcp::McpServerHandle`].
//! - [`tools`] — the FROZEN Maestro tool descriptor registry (design/08 §5.1: 11 read, 5 write, 2 side-channel = 18 tools; the doc's "16" headline is an arithmetic slip) with input/output JSON schemas + [`tools::ToolKind`] + a typed-unimplemented [`tools::dispatch`]. 405/406/407 fill the tool bodies behind these unchanged schemas.
//!
//! Everything else — the agent lifecycle/actor, `MaestroHandle`, the summary
//! cache, routing, digests, the provider seam, privacy enforcement, token
//! accounting — is OUT of 401 (Scope — out) and lands in 402/404/408/409/410/413
//! and 401.5/414. This `mod.rs` deliberately carries **no** actor logic, **no**
//! `MaestroHandle`, and **no** `MaestroState` wiring beyond the server handle
//! re-export.
//!
//! ## Module gating
//!
//! The whole module is `#[cfg(unix)] pub mod maestro;` in `lib.rs` (it sits over
//! the `cfg(unix)` agent supervisor, mirroring `agent_supervisor`/`scheduler`/
//! `suggestions`). The Windows lane (Task 113) simply omits it; no call site
//! outside this module references it in 401, so no non-unix stub is needed.

// ===========================================================================
// SOFT SEAM — the cluster-M `mod.rs` (PHASE4_PLANNING §8.1).
//
// 401 owns this initial `mod.rs`. Later Maestro tasks each append ONE
// `pub mod X;` line below, on its OWN distinct line, so a rebase auto-merges:
//
//   pub mod summary;   // Task 404 — WorkareaSummary cache (§4.4)
//   pub mod provider;  // Task 402/412 — the LLM provider-selection seam (§4.3)
//   pub mod routing;   // Task 408 — the @workarea / @composer pre-parser (§4.7)
//   pub mod digest;    // Task 409 — digest generation (§3.6)
//   pub mod condense;  // Task 410 — daily chat condensation (§3.7)
//   pub mod privacy;   // Task 413 — enterprise-privacy + exclude_from_maestro gate
//
// (Intentionally only `mcp` + `tools` in 401 — the MCP surface.)
// ===========================================================================
pub mod mcp;
pub mod summary; // Task 404 — WorkareaSummary cache (§4.4)
pub mod tools;

// ===========================================================================
// Task 401.5 — wire-contract freeze (additive region; PHASE4_PLANNING §4.2).
//
// Added by 401.5 in its OWN distinct region so it auto-merges against 401's
// `pub mod mcp;`/`pub mod tools;` above and the sibling 402/404/410 soft-seam
// lines. `handle` carries the FROZEN `MaestroHandle` Core-side API surface
// (design/08 §5.2) — an opaque struct whose five async signatures return a
// typed `"unimplemented:"`-prefixed `Err` until Task 414 supplies the actor.
// ===========================================================================
pub mod handle;

// ===========================================================================
// Task 402 region — DISTINCT additive zone (do NOT merge with 401's `pub mod
// mcp;`/`pub mod tools;` above, nor with 404/408/410's future lines). Keeping
// the 402 additions in their own block lets the concurrent siblings (401.5,
// 404, 410) auto-merge on rebase per PHASE4_PLANNING §8.1.
//
// The Maestro provider-selection seam (which CLI binary + model + preamble +
// `--mcp-config` + strict + scratch cwd to launch). FROZEN by 402 (Claude
// live), extended by 412 (Codex/Gemini live + the Direct-API frozen-unwired
// arm). See PHASE4_PLANNING §4.3 / D1/D5.
pub mod provider;
// ===========================================================================

// --- Task 410 (daily condensation, §3.7) — distinct additive region. -------
pub mod condense;

// ===========================================================================
// Task 409 region — DISTINCT additive zone (PHASE4_PLANNING §3.6 / §8.1). Kept
// in its OWN clearly-labeled block so it auto-merges against 401's `pub mod
// mcp;`/`pub mod tools;`, the 401.5/402/404/408/410 soft-seam lines above, AND
// task 412's in-flight `mod.rs` re-export region.
//
// Return-from-absence digest generation: consumes 404's summary cache, 408's
// `/digest` route, 312's `OneShotLlm`/`DigestSummary`, and 403's
// `last_digest_at`/maestro-chat singleton; persists the digest's chips on the
// `kind='maestro'` chat row (D11). FROZEN by 409; 414 (`GetDigest`) consumes
// `Digest`/`WorkareaDelta`/`generate_digest`.
pub mod digest;
// ===========================================================================

// ===========================================================================
// Task 408 region — DISTINCT additive zone (PHASE4_PLANNING §4.7 / §8.1). Kept
// in its OWN clearly-labeled block so it auto-merges against 401's `pub mod
// mcp;`/`pub mod tools;`, the 401.5/402/404 lines above, AND task 410's
// in-flight `pub mod condense;` soft-seam line.
//
// The deterministic, zero-LLM routing pre-parser + composer→workarea→session
// resolver (`pre_parse(&str) -> ParseOutcome`, design/08 §3.5/§6.3). FROZEN by
// 408; 409 (`/digest`) + 414 (`SendToMaestro` pre-parse) consume the grammar.
pub mod routing;
// ===========================================================================

// ===========================================================================
// Task 413 region — DISTINCT additive zone (PHASE4_PLANNING §8.1 / §4.4 / D10).
// Kept in its OWN clearly-labeled block so it auto-merges against 401's
// `pub mod mcp;`/`pub mod tools;`, the 401.5/402/404 lines above, AND task
// 409's in-flight digest soft-seam line.
//
// The pure Maestro privacy policy: blank `exclude_from_maestro` workareas to
// name-only over 404's summary cache, the `concerto_chat_full_chat_access`
// summary-source flip, and the `enterpriseDataPrivacy`+external ⇒ Maestro-LLM
// disabled gate (routing unaffected). design/08 §3.3 / §3.10. Consumed by
// 405/409 (read-blanked summaries) + 412/414 (the disable decision).
pub mod privacy;
// ===========================================================================

// ===========================================================================
// Task 414 region — DISTINCT additive zone (PHASE4_PLANNING §4.2 / §8.1 / D7).
// Kept in its OWN clearly-labeled block so it auto-merges against the lines
// above AND task 411's in-flight `pub mod cone_suggester;` soft-seam line.
//
// The five Maestro stream events + their opaque-JSON wire frame
// (`{"kind": ...}` on `Event.checks_opaque = 17`, NO new oneof arm) the live
// service publishes on `maestro.events`. FROZEN by 414; consumed by 415.
pub mod events;
// ===========================================================================

// ===========================================================================
// Task 411 region — DISTINCT additive zone (PHASE4_PLANNING §8.1 / §2 row 411).
// Kept in its OWN clearly-labeled block so it auto-merges against 414's
// `pub mod events;` line above and the sibling soft-seam lines.
//
// The Maestro-backed `ConeSuggester` (the LIVE wiring of 305's seam through
// 312's `OneShotLlm`, `DeterministicOneShot` fallback) injected into the
// RepoManager via `with_cone_suggester` at boot. design/08 §3.8. Consumed by
// `RepoManager::suggest_cones` (the `Repositories.SuggestCones` RPC) + the
// create-from-description planner.
pub mod cone_suggester;
// ===========================================================================

// ===========================================================================
// Task 3 (Maestro Live-Integration) region — DISTINCT additive zone
// (design Fork B1). Kept in its OWN clearly-labeled block so it auto-merges
// against the sibling soft-seam lines above.
//
// The reserved, UI-hidden system workspace + workarea that hosts the global
// Maestro session, satisfying `sessions.workarea_id NOT NULL REFERENCES
// workareas(id)` without a schema change. Ensured idempotently at boot; the
// sentinel ids are filtered from every user-facing list (a separate task).
pub mod system_workarea;
// ===========================================================================

// ---------------------------------------------------------------------------
// Public surface re-exports (the cluster-M root's `pub use` zone).
// ---------------------------------------------------------------------------
pub use mcp::{serve_maestro_mcp, MaestroMcpServer, McpServerHandle, SERVER_NAME};
pub use tools::{all_tools, dispatch, ToolDescriptor, ToolKind};

// Task 401.5 — re-export the frozen handle surface (additive). Task 414 adds
// the live `SendOutcome`/`InertReason` companions the handler + 412 consume.
pub use handle::{InertReason, MaestroHandle, MaestroStateView, SendOutcome};

// Task 414 — re-export the FROZEN Maestro events surface (distinct region; see
// the `pub mod events;` block above). The domain event enum + its producer the
// live service publishes on `maestro.events`.
pub use events::{MaestroEvent, MaestroEventSender};

// Task 411 — re-export the Maestro-backed cone suggester (distinct region; see
// the `pub mod cone_suggester;` block above). `boot.rs` injects it into the
// RepoManager via `with_cone_suggester`.
pub use cone_suggester::MaestroConeSuggester;

// Task 3 (Maestro Live-Integration) — re-export the reserved system
// workspace+workarea ensure-helper + its sentinel ids (distinct region; see the
// `pub mod system_workarea;` block above). Boot calls
// `ensure_system_workspace_and_workarea` to host the global Maestro session.
pub use system_workarea::{
    ensure_system_workspace_and_workarea, SYSTEM_WORKAREA_ID, SYSTEM_WORKSPACE_ID,
};

// Task 402 re-exports (distinct region — see above).
pub use provider::{
    ClaudeCliProvider, DirectApiProvider, MaestroLaunchContext, MaestroLaunchSpec, MaestroProvider,
    MAESTRO_PERMISSION_MODE, MAESTRO_PREAMBLE,
};

// Task 408 re-exports (distinct region — see the `pub mod routing;` block
// above). The FROZEN routing grammar + resolver surface 409/414 consume.
pub use routing::{
    pre_parse, DispatchResult, ParseOutcome, ResolvedRoute, Router, RoutingError, RoutingTarget,
    SlashDirective, WorkareaRef,
};

// Task 409 re-exports (distinct region — see the `pub mod digest;` block
// above). The FROZEN digest producer surface 408 (`/digest`) + 414 (`GetDigest`)
// consume.
pub use digest::{generate_digest, Digest, DigestEntry, DigestGroup, WorkareaDelta};

// ===========================================================================
// Task 412 region — DISTINCT additive zone (do NOT merge with 402's `pub use`
// above, nor with 408/410's future lines). The live Codex/Gemini providers, the
// `MaestroBackend` enum, the `select_provider` auto-pick + `disabled_by_policy`
// outcome, and the typed Direct-API marker helpers. See PHASE4_PLANNING §4.3.
// ===========================================================================
pub use provider::{
    direct_api_unimplemented, is_direct_api_unimplemented, select_provider, CodexCliProvider,
    GeminiCliProvider, MaestroBackend, ProviderSelection, DEFAULT_CODEX_BIN, DEFAULT_GEMINI_BIN,
    DIRECT_API_UNIMPLEMENTED_MARKER,
};
// ===========================================================================

// ===========================================================================
// Task 413 re-exports (distinct region — see the `pub mod privacy;` block
// above). The FROZEN privacy-policy surface 405/409 (read-blanked summaries) +
// 412/414 (the LLM-disable decision) consume.
// ===========================================================================
pub use privacy::{
    MaestroLlmGate, MaestroModelLocality, PrivacyPolicy, SummarySource, PRIVATE_WORKAREA_BLANK,
};
// ===========================================================================

// ===========================================================================
// Task 402 — the Maestro spawn-config constructor + the scratch-cwd convention
// (PHASE4_PLANNING §4.8 / §2). The boot-time call site + the
// `enterpriseDataPrivacy`-disabled gate are Task 414's; here we freeze the
// pure constructor + the scratch-dir convention so 414 just calls it.
// ===========================================================================

use std::path::{Path, PathBuf};

use concerto_error::{Error, Result};

use crate::agent_supervisor::{AgentKind, StartSessionRequest};
use concerto_persist::WorkareaId;

/// The Maestro scratch working directory relative to the user's home:
/// `~/concerto/maestro/`. A scratch dir, NOT a worktree — the Maestro has no
/// file-edit tools, so there is no edit-mutex (PHASE4_PLANNING §2 / D4).
pub const MAESTRO_SCRATCH_SUBDIR: &str = "concerto/maestro";

/// Resolve the Maestro scratch directory (`~/concerto/maestro/`) from the
/// user's home directory. Mirrors `ensure_claude_trusts_dir`'s `home::home_dir`
/// resolution so the path is consistent across the supervisor and the provider.
pub fn maestro_scratch_dir() -> Result<PathBuf> {
    let home = home::home_dir().ok_or_else(|| {
        Error::Internal("cannot resolve home dir for the Maestro scratch cwd".into())
    })?;
    Ok(home.join(MAESTRO_SCRATCH_SUBDIR))
}

/// Create the Maestro scratch directory (idempotent) and return its path. Call
/// before spawning the Maestro session so the CLI's cwd exists. The directory
/// is created with the platform default permissions; it holds only the CLI's
/// transient session state (no user repo data).
pub fn ensure_maestro_scratch_dir() -> Result<PathBuf> {
    let dir = maestro_scratch_dir()?;
    std::fs::create_dir_all(&dir).map_err(Error::Io)?;
    Ok(dir)
}

/// Build the [`StartSessionRequest`] that spawns the long-lived Maestro session
/// under the Agent Supervisor (PHASE4_PLANNING §4.8). Pure: it constructs the
/// request (`agent_kind = Maestro`, `permission_mode = "strict"`, `cwd =
/// scratch_dir`) without touching the supervisor — Task 414 calls this at boot
/// (gated on `maestro_state.enabled` + the `enterpriseDataPrivacy` policy) and
/// hands the result to `start_session`, reusing host-survival / cold-resume
/// verbatim.
///
/// `workarea_id` is the placeholder workarea the Maestro singleton is recorded
/// against; `scratch_cwd` is [`ensure_maestro_scratch_dir`]'s result.
pub fn maestro_start_request(workarea_id: WorkareaId, scratch_cwd: PathBuf) -> StartSessionRequest {
    StartSessionRequest {
        workarea_id,
        agent_kind: AgentKind::Maestro,
        echo_text: None,
        cwd: scratch_cwd,
        // The Maestro ALWAYS runs strict: reads auto-approve via
        // ToolClass::ReadOnly, writes/propose_chip surface as confirmation
        // chips (Task 402's permission matrix).
        permission_mode: Some(provider::MAESTRO_PERMISSION_MODE.to_string()),
        // First spawn — no cold-resume token. Cold-resume after a Core restart
        // is the supervisor's `cold_resume_session` path (found via the
        // chats(kind='maestro') singleton), unchanged by this task.
        resume_session_id: None,
    }
}

/// Pre-seed Claude's folder-trust record for the Maestro scratch dir so the
/// CLI's interactive "trust this folder?" dialog never blocks the strict
/// Maestro session. Delegates to the supervisor's
/// [`crate::agent_supervisor::ensure_claude_trusts_dir`] (the same trust-preseed
/// pattern used for workarea sessions). Idempotent.
pub fn ensure_maestro_scratch_trusted(scratch_cwd: &Path) -> Result<()> {
    crate::agent_supervisor::ensure_claude_trusts_dir(scratch_cwd)
}

/// The Core's Maestro-MCP unix socket path (`~/.concerto/maestro-mcp.sock`).
/// The bridge dials this; the Core listens on it (a later task).
pub fn maestro_mcp_socket_path() -> Result<PathBuf> {
    let home = home::home_dir().ok_or_else(|| {
        Error::Internal("cannot resolve home dir for the Maestro MCP socket path".into())
    })?;
    Ok(home.join(".concerto").join("maestro-mcp.sock"))
}

/// Compose the `.mcp.json` body that points the spawned CLI at the bridge.
/// `--strict-mcp-config` (in the launch args) restricts the CLI to exactly the
/// server registered here.
pub fn compose_maestro_mcp_json(bridge_bin: &std::path::Path, socket: &std::path::Path) -> String {
    let v = serde_json::json!({
        "mcpServers": {
            crate::maestro::mcp::SERVER_NAME: {
                "command": bridge_bin.to_string_lossy(),
                "args": ["--socket", socket.to_string_lossy()],
            }
        }
    });
    serde_json::to_string_pretty(&v).expect("serializing a json! literal never fails")
}

/// Write the Maestro `.mcp.json` into `scratch_cwd` (the `--mcp-config` target:
/// `scratch_cwd/.mcp.json`). Idempotent (overwrites).
pub fn write_maestro_mcp_json(
    scratch_cwd: &std::path::Path,
    bridge_bin: &std::path::Path,
    socket: &std::path::Path,
) -> Result<PathBuf> {
    let path = scratch_cwd.join(".mcp.json");
    std::fs::write(&path, compose_maestro_mcp_json(bridge_bin, socket)).map_err(Error::Io)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_subdir_is_concerto_maestro() {
        assert_eq!(MAESTRO_SCRATCH_SUBDIR, "concerto/maestro");
        // The resolved path ends with the scratch subdir under $HOME.
        let dir = maestro_scratch_dir().expect("home dir resolves in test env");
        assert!(dir.ends_with("concerto/maestro"));
    }

    #[test]
    fn maestro_start_request_is_strict_maestro_in_scratch_cwd() {
        let scratch = PathBuf::from("/home/user/concerto/maestro");
        let req = maestro_start_request(WorkareaId("wa-maestro".into()), scratch.clone());
        assert_eq!(req.agent_kind, AgentKind::Maestro);
        assert_eq!(req.permission_mode.as_deref(), Some("strict"));
        assert_eq!(req.cwd, scratch);
        assert!(req.resume_session_id.is_none());
        assert!(req.echo_text.is_none());
    }

    #[test]
    fn mcp_json_names_bridge_and_socket_under_server_name() {
        let json = compose_maestro_mcp_json(
            std::path::Path::new("/opt/concerto/concerto-maestro-bridge"),
            std::path::Path::new("/home/u/.concerto/maestro-mcp.sock"),
        );
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let server = &v["mcpServers"][crate::maestro::mcp::SERVER_NAME];
        assert_eq!(server["command"], "/opt/concerto/concerto-maestro-bridge");
        assert_eq!(server["args"][0], "--socket");
        assert_eq!(server["args"][1], "/home/u/.concerto/maestro-mcp.sock");
    }
}
