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

// ---------------------------------------------------------------------------
// Public surface re-exports (the cluster-M root's `pub use` zone).
// ---------------------------------------------------------------------------
pub use mcp::{serve_maestro_mcp, MaestroMcpServer, McpServerHandle, SERVER_NAME};
pub use tools::{all_tools, dispatch, ToolDescriptor, ToolKind};

// Task 401.5 — re-export the frozen handle surface (additive).
pub use handle::{MaestroHandle, MaestroStateView};
