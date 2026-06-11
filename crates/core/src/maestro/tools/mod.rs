//! The FROZEN Maestro MCP schema registry (Task 401, design/08 §5.1 /
//! PHASE4_PLANNING §4.1) — the cluster-M tool contract every later Maestro task
//! builds behind.
//!
//! **Tool count:** design/08 §5.1 enumerates **11 read + 5 write + 2
//! side-channel = 18** distinct tools. The design doc's headline "16 tools" is
//! an arithmetic slip (11 + 5 + 2 = 18); every concrete enumeration (design
//! §5.1, PHASE4_PLANNING §4.1, this task's Public-interface block) lists the
//! same 18 names. The VERBATIM name list is the FROZEN contract.
//!
//! This module is the single source of truth for **which** tools the
//! `concerto-maestro-mcp` server (`super::mcp`) exposes, **what** their
//! input/output JSON schemas are, and **how** each one is classified for 04's
//! strict-mode permission matrix. The schemas here are FROZEN: 405/406/407 fill
//! the tool *bodies* (the dispatch) behind these exact names + arg sets +
//! input/output schemas; they **never** re-shape a tool's schema.
//!
//! ## The 18 tools (design/08 §5.1, arg names transcribed VERBATIM)
//!
//! - **11 read tools** ([`ToolKind::ReadOnly`] — 402 auto-approves under
//!   strict): `list_workspaces`, `list_workareas`, `list_sessions`,
//!   `get_workspace_summary`, `get_workarea_summary`, `list_recent_activity`,
//!   `list_active_schedules`, `read_inbox_summary`, `read_pr_set_for_workarea`,
//!   `get_workarea_recent_commits`, `cross_workarea_search`.
//! - **5 write tools** ([`ToolKind::Write`] — 402/406 force `MustAsk` under
//!   strict ⇒ confirmation chip, no bypass, design/08 R-2):
//!   `route_prompt_to_session`, `fanout_to_sessions`, `create_workspace`,
//!   `create_workarea`, `set_workarea_paused`.
//! - **2 side-channel tools** ([`ToolKind::SideChannel`]): `notify_user`
//!   (routes through 14, Task 407 stub), `propose_chip` (adds to the
//!   Maestro-owned slate, D11, Task 407).
//!
//! ## The typed-unimplemented contract (the 305 seam discipline)
//!
//! In Task 401 every tool is **registered** with its frozen schema (so the CLI
//! sees all 18 and the contract is locked) but its [`dispatch`] returns a
//! **typed `rmcp` error** ([`rmcp::ErrorData`] / `McpError`) carrying a stable
//! `"tool <name> is wired in Task 40N"` message. We **never** use
//! `todo!()`/`unimplemented!()` (a panic crashes the in-process server and the
//! agent host) and **never** return an empty / `Ok(())` success (which reads to
//! the agent as "the tool did nothing"). This mirrors 305's
//! `ConeSuggestError::Unwired → Status::unimplemented` discipline.
//!
//! ## The soft seam 405/406/407 extend
//!
//! 405 (read tools), 406 (write tools), and 407 (side-channels) each add their
//! own `tools/{read,write,side}.rs` file and a single `pub mod {read,write,side};`
//! line in the marked region below (the lead-owned seam), then replace their
//! tools' arms inside [`dispatch`]. They do not touch the [`ToolDescriptor`]
//! shape, [`all_tools`], or any schema.

// ---------------------------------------------------------------------------
// SOFT SEAM — 405/406/407 add their tool-impl submodules here, one per line.
// Each line is additive and on its own line so a rebase auto-merges:
//
//   pub mod read;   // Task 405 — the 11 read-tool dispatch arms
//   pub mod write;  // Task 406 — the 5 write-tool dispatch arms
//   pub mod side;   // Task 407 — notify_user / propose_chip dispatch arms
//
// (Intentionally empty in Task 401 — the dispatch below is the frozen
// typed-unimplemented seam each task replaces its own arm of.)
// ---------------------------------------------------------------------------

// Task 405 — the 11 read-tool impls behind 401's frozen read schemas. The live
// entry point is [`read::dispatch_read`] (async + Core-handle-bearing); the
// handle-less [`dispatch`] below routes the 11 read names to it once the MCP
// server (`super::mcp`) threads the Core handles in (402/414's wiring). This
// line sits in 405's OWN region so the sibling 406 `pub mod write;` / 407
// `pub mod side;` lines auto-merge on rebase.
pub mod read;

use rmcp::model::{CallToolResult, JsonObject, Tool};
use rmcp::ErrorData as McpError;
use serde_json::{json, Value};
use std::sync::Arc;

/// The permission class of a Maestro tool, used by 402/406 to map each tool
/// onto 04's strict-mode permission matrix.
///
/// - [`ToolKind::ReadOnly`] — the 11 read tools; 402 adds the
///   `ToolClass::ReadOnly` bucket that auto-approves these under strict.
/// - [`ToolKind::Write`] — the 5 write tools; 402/406 force `MustAsk` under
///   strict ⇒ the existing `AwaitingApproval`/`ResolveApproval` confirmation
///   chip (no bypass, design/08 R-2).
/// - [`ToolKind::SideChannel`] — `notify_user` (routes through 14) and
///   `propose_chip` (adds to the Maestro-owned slate, D11).
///
/// Task 401 only **tags** the descriptors; it does not touch
/// `security/tool_classes.rs` (that is 402's `ToolClass::ReadOnly`, §4.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    ReadOnly,
    Write,
    SideChannel,
}

/// One registered Maestro MCP tool: its name, its read/write/side class, and
/// its input/output JSON schemas.
///
/// The schema is the contract: 405/406/407 fill [`dispatch`] behind these exact
/// names + schemas. FROZEN by Task 401 (design/08 §5.1 / PHASE4_PLANNING §4.1).
#[derive(Debug, Clone)]
pub struct ToolDescriptor {
    /// Exactly the design/08 §5.1 tool name (the FROZEN wire name).
    pub name: &'static str,
    /// The read/write/side classification (402/406 map this to the permission
    /// matrix).
    pub class: ToolKind,
    /// A short human-readable description surfaced to the agent CLI.
    pub description: &'static str,
    /// JSON Schema (a JSON object) for the tool's arguments; arg names per §5.1.
    pub input_schema: Value,
    /// JSON Schema (a JSON object) for the tool's return shape.
    ///
    /// `get_workarea_summary`'s output is a minimal placeholder pending Task 404
    /// (`WorkareaSummary`, §4.4); 404/405 align it. All other shapes are taken
    /// from design/08 §5.1.
    pub output_schema: Value,
}

impl ToolDescriptor {
    /// Convert this descriptor into the `rmcp` [`Tool`] the server registers in
    /// its `list_tools` response. Both schemas are object JSON Schemas, so the
    /// `serde_json::Value`→`JsonObject` conversion is infallible by construction
    /// (every descriptor in [`all_tools`] is built from a `json!({...})` object).
    pub fn to_rmcp_tool(&self) -> Tool {
        Tool::new(
            self.name,
            self.description,
            Arc::new(value_into_object(&self.input_schema)),
        )
        .with_raw_output_schema(Arc::new(value_into_object(&self.output_schema)))
    }
}

/// A `serde_json::Value` that is statically known to be a JSON object →
/// `rmcp`'s `JsonObject` (`serde_json::Map`). Every schema in [`all_tools`] is a
/// `json!({...})` object literal, so the `as_object` never fails; if a future
/// edit breaks that invariant the empty fallback keeps the server alive (it
/// never panics) and the schema test in this module's tests catches the drift.
fn value_into_object(v: &Value) -> JsonObject {
    v.as_object().cloned().unwrap_or_default()
}

/// The stable message-prefix every typed-unimplemented dispatch carries so 402's
/// agent loop and the Tier-1 tests can assert it. Each tool appends "(Task 40N)".
pub const UNIMPLEMENTED_MSG_PREFIX: &str = "maestro tool not yet wired:";

/// Build the typed `unimplemented` MCP error a tool returns until its impl task
/// (405/406/407) lands. This is the 305 seam discipline: a **typed** error
/// (never `todo!()`/`unimplemented!()`, never empty-success).
fn unimplemented(tool: &str, impl_task: &str) -> McpError {
    McpError::internal_error(
        format!("{UNIMPLEMENTED_MSG_PREFIX} {tool} is wired in Task {impl_task}"),
        None,
    )
}

/// The FROZEN registry: exactly 18 descriptors, the design/08 §5.1 set, in the
/// canonical order (11 read, 5 write, 2 side-channel). The server
/// (`super::mcp`) iterates this to populate its `list_tools` response.
pub fn all_tools() -> Vec<ToolDescriptor> {
    vec![
        // ---- 11 read tools (ToolKind::ReadOnly) ----
        ToolDescriptor {
            name: "list_workspaces",
            class: ToolKind::ReadOnly,
            description: "List all workspaces with their workarea/repo counts.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "workspaces": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "name": { "type": "string" },
                                "archived": { "type": "boolean" },
                                "n_workareas": { "type": "integer" },
                                "n_repos": { "type": "integer" }
                            },
                            "required": ["id", "name", "archived", "n_workareas", "n_repos"]
                        }
                    }
                },
                "required": ["workspaces"]
            }),
        },
        ToolDescriptor {
            name: "list_workareas",
            class: ToolKind::ReadOnly,
            description: "List workareas, optionally filtered to one workspace.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace_id": { "type": "string" }
                },
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "workareas": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "workspace_id": { "type": "string" },
                                "composer": { "type": "string" },
                                "branch": { "type": "string" },
                                "status": { "type": "string" },
                                "last_activity": { "type": "integer" }
                            },
                            "required": ["id", "workspace_id", "composer", "branch", "status", "last_activity"]
                        }
                    }
                },
                "required": ["workareas"]
            }),
        },
        ToolDescriptor {
            name: "list_sessions",
            class: ToolKind::ReadOnly,
            description: "List sessions, optionally filtered to one workarea.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workarea_id": { "type": "string" }
                },
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "sessions": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "workarea_id": { "type": "string" },
                                "agent_kind": { "type": "string" },
                                "status": { "type": "string" },
                                "last_activity": { "type": "integer" }
                            },
                            "required": ["id", "workarea_id", "agent_kind", "status", "last_activity"]
                        }
                    }
                },
                "required": ["sessions"]
            }),
        },
        ToolDescriptor {
            name: "get_workspace_summary",
            class: ToolKind::ReadOnly,
            description: "Summary of one workspace (active-workarea counts and rollups).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace_id": { "type": "string" }
                },
                "required": ["workspace_id"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "workspace": { "type": "string" },
                    "n_active_workareas": { "type": "integer" }
                },
                "required": ["workspace", "n_active_workareas"]
            }),
        },
        ToolDescriptor {
            name: "get_workarea_summary",
            class: ToolKind::ReadOnly,
            // NOTE: output is `WorkareaSummary`, FROZEN by Task 404 (§4.4). The
            // schema below is a MINIMAL placeholder authored in 401; 404/405
            // align it to the real `WorkareaSummary` shape (i64-ms timestamps,
            // per-repo hard facts). 401 references it by name only.
            description:
                "Rolling summary of one workarea (WorkareaSummary; shape frozen by Task 404).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workarea_id": { "type": "string" }
                },
                "required": ["workarea_id"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "description": "WorkareaSummary — shape frozen by Task 404 (§4.4); placeholder in 401.",
                "properties": {
                    "workarea_id": { "type": "string" }
                },
                "required": ["workarea_id"]
            }),
        },
        ToolDescriptor {
            name: "list_recent_activity",
            class: ToolKind::ReadOnly,
            description: "Recent activity events since a given unix-ms timestamp.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "since": { "type": "integer" }
                },
                "required": ["since"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "events": {
                        "type": "array",
                        "items": { "type": "object" }
                    }
                },
                "required": ["events"]
            }),
        },
        ToolDescriptor {
            name: "list_active_schedules",
            class: ToolKind::ReadOnly,
            description: "List currently-active schedules.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "schedules": {
                        "type": "array",
                        "items": { "type": "object" }
                    }
                },
                "required": ["schedules"]
            }),
        },
        ToolDescriptor {
            name: "read_inbox_summary",
            class: ToolKind::ReadOnly,
            description: "Summary of the notifications inbox.",
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "description": "InboxSummary",
                "properties": {}
            }),
        },
        ToolDescriptor {
            name: "read_pr_set_for_workarea",
            class: ToolKind::ReadOnly,
            description: "PR-set status for one workarea.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workarea_id": { "type": "string" }
                },
                "required": ["workarea_id"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "description": "PrSetStatus",
                "properties": {}
            }),
        },
        ToolDescriptor {
            name: "get_workarea_recent_commits",
            class: ToolKind::ReadOnly,
            description: "Recent commits for one workarea, optionally scoped to one repo.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workarea_id": { "type": "string" },
                    "repo_id": { "type": "string" }
                },
                "required": ["workarea_id"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "commits": {
                        "type": "array",
                        "items": { "type": "object" }
                    }
                },
                "required": ["commits"]
            }),
        },
        ToolDescriptor {
            name: "cross_workarea_search",
            class: ToolKind::ReadOnly,
            description: "Search commits, diffs, and todos across all workareas.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "hits": {
                        "type": "array",
                        "items": { "type": "object" }
                    }
                },
                "required": ["hits"]
            }),
        },
        // ---- 5 write tools (ToolKind::Write) ----
        ToolDescriptor {
            name: "route_prompt_to_session",
            class: ToolKind::Write,
            description: "Route a prompt to one session (user-confirmed under strict).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "prompt": { "type": "string" }
                },
                "required": ["session_id", "prompt"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDescriptor {
            name: "fanout_to_sessions",
            class: ToolKind::Write,
            description: "Fan a prompt out to several sessions (user-confirmed under strict).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_ids": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "prompt": { "type": "string" }
                },
                "required": ["session_ids", "prompt"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDescriptor {
            name: "create_workspace",
            class: ToolKind::Write,
            description: "Create a workspace from a spec (user confirms).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "spec": { "type": "object" }
                },
                "required": ["spec"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "workspace_id": { "type": "string" }
                },
                "required": ["workspace_id"]
            }),
        },
        ToolDescriptor {
            name: "create_workarea",
            class: ToolKind::Write,
            description: "Create a workarea in a workspace from a spec (user confirms).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace_id": { "type": "string" },
                    "spec": { "type": "object" }
                },
                "required": ["workspace_id", "spec"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {
                    "workarea_id": { "type": "string" }
                },
                "required": ["workarea_id"]
            }),
        },
        ToolDescriptor {
            name: "set_workarea_paused",
            class: ToolKind::Write,
            description: "Pause or resume a workarea (user-confirmed under strict).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workarea_id": { "type": "string" },
                    "paused": { "type": "boolean" }
                },
                "required": ["workarea_id", "paused"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        // ---- 2 side-channel tools (ToolKind::SideChannel) ----
        ToolDescriptor {
            name: "notify_user",
            class: ToolKind::SideChannel,
            description: "Send the user a notification through 14 (Task 407 stub).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "severity": { "type": "string" }
                },
                "required": ["text", "severity"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDescriptor {
            name: "propose_chip",
            class: ToolKind::SideChannel,
            description: "Add a chip to the Maestro-owned current slate (D11, Task 407).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "chip": { "type": "object" }
                },
                "required": ["chip"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
    ]
}

/// Dispatch a tool call by name. In Task 401 every arm returns the typed
/// `unimplemented` MCP error (the 305 seam discipline) — 405/406/407 replace
/// each arm with a live implementation behind the same frozen schema. An
/// unknown name returns `invalid_params` (the agent asked for a tool that is
/// not in the frozen set).
///
/// `_arguments` are the validated tool arguments; unused in 401 (no tool runs).
pub fn dispatch(name: &str, _arguments: Option<JsonObject>) -> Result<CallToolResult, McpError> {
    match name {
        // 11 read tools → Task 405. The live impls live in [`read`]; the real
        // entry point is the async, Core-handle-bearing [`read::dispatch_read`]
        // (the 11 read tools query persistence / the 404 summary cache / live
        // grep, which this handle-less sync `dispatch` cannot reach). The MCP
        // server (`super::mcp`) calls `read::dispatch_read` once 402/414 thread
        // the Core handles into `MaestroMcpServer`; until that wiring lands this
        // handle-less path keeps 401's typed seam error (never a macro, never a
        // fake-success) — `read::dispatch_read` is the route that actually runs.
        "list_workspaces"
        | "list_workareas"
        | "list_sessions"
        | "get_workspace_summary"
        | "get_workarea_summary"
        | "list_recent_activity"
        | "list_active_schedules"
        | "read_inbox_summary"
        | "read_pr_set_for_workarea"
        | "get_workarea_recent_commits"
        | "cross_workarea_search" => Err(unimplemented(name, "405")),

        // 5 write tools → Task 406 (tools/write.rs)
        "route_prompt_to_session"
        | "fanout_to_sessions"
        | "create_workspace"
        | "create_workarea"
        | "set_workarea_paused" => Err(unimplemented(name, "406")),

        // 2 side-channel tools → Task 407 (tools/side.rs)
        "notify_user" | "propose_chip" => Err(unimplemented(name, "407")),

        // Not in the frozen 18-tool set.
        other => Err(McpError::invalid_params(
            format!("unknown maestro tool: {other}"),
            None,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The FROZEN design/08 §5.1 tool-name set, in canonical order.
    const READ_TOOLS: [&str; 11] = [
        "list_workspaces",
        "list_workareas",
        "list_sessions",
        "get_workspace_summary",
        "get_workarea_summary",
        "list_recent_activity",
        "list_active_schedules",
        "read_inbox_summary",
        "read_pr_set_for_workarea",
        "get_workarea_recent_commits",
        "cross_workarea_search",
    ];
    const WRITE_TOOLS: [&str; 5] = [
        "route_prompt_to_session",
        "fanout_to_sessions",
        "create_workspace",
        "create_workarea",
        "set_workarea_paused",
    ];
    const SIDE_TOOLS: [&str; 2] = ["notify_user", "propose_chip"];

    #[test]
    fn registers_exactly_the_frozen_tool_set() {
        // design/08 §5.1 enumerates 11 read + 5 write + 2 side-channel = 18
        // distinct tool names. The doc's headline "16 tools" is an arithmetic
        // slip; every concrete enumeration (design §5.1, PHASE4_PLANNING §4.1,
        // this task's Public-interface block) lists all 18. The VERBATIM name
        // list is the FROZEN contract, not the headline count.
        const EXPECTED: usize = READ_TOOLS.len() + WRITE_TOOLS.len() + SIDE_TOOLS.len();
        assert_eq!(EXPECTED, 18);

        let tools = all_tools();
        assert_eq!(tools.len(), EXPECTED, "the §5.1 set is exactly 18 tools");

        let got: BTreeSet<&str> = tools.iter().map(|t| t.name).collect();
        let mut want: BTreeSet<&str> = BTreeSet::new();
        want.extend(READ_TOOLS);
        want.extend(WRITE_TOOLS);
        want.extend(SIDE_TOOLS);
        assert_eq!(
            got, want,
            "tool names must equal the frozen design/08 §5.1 set"
        );

        // No duplicate names.
        assert_eq!(got.len(), EXPECTED, "tool names must be unique");
    }

    #[test]
    fn class_split_is_eleven_five_two() {
        let tools = all_tools();
        let reads = tools
            .iter()
            .filter(|t| t.class == ToolKind::ReadOnly)
            .count();
        let writes = tools.iter().filter(|t| t.class == ToolKind::Write).count();
        let sides = tools
            .iter()
            .filter(|t| t.class == ToolKind::SideChannel)
            .count();
        assert_eq!(
            (reads, writes, sides),
            (11, 5, 2),
            "the 11/5/2 class split is frozen"
        );

        // And each class carries exactly the right names.
        for t in &tools {
            let in_read = READ_TOOLS.contains(&t.name);
            let in_write = WRITE_TOOLS.contains(&t.name);
            let in_side = SIDE_TOOLS.contains(&t.name);
            match t.class {
                ToolKind::ReadOnly => assert!(in_read, "{} mis-classed", t.name),
                ToolKind::Write => assert!(in_write, "{} mis-classed", t.name),
                ToolKind::SideChannel => assert!(in_side, "{} mis-classed", t.name),
            }
        }
    }

    #[test]
    fn every_tool_has_object_input_and_output_schemas_with_frozen_args() {
        for t in all_tools() {
            assert!(
                t.input_schema.is_object(),
                "{} input_schema must be a JSON object",
                t.name
            );
            assert!(
                t.output_schema.is_object(),
                "{} output_schema must be a JSON object",
                t.name
            );
            assert_eq!(
                t.input_schema.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "{} input_schema must be type=object",
                t.name
            );
        }

        // Spot-check a few frozen arg-name sets (the §5.1 contract).
        let by_name = |n: &str| all_tools().into_iter().find(|t| t.name == n).unwrap();

        let props = |t: &ToolDescriptor| -> BTreeSet<String> {
            t.input_schema
                .get("properties")
                .and_then(|p| p.as_object())
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default()
        };

        assert_eq!(
            props(&by_name("route_prompt_to_session")),
            BTreeSet::from(["session_id".to_string(), "prompt".to_string()])
        );
        assert_eq!(
            props(&by_name("fanout_to_sessions")),
            BTreeSet::from(["session_ids".to_string(), "prompt".to_string()])
        );
        assert_eq!(
            props(&by_name("set_workarea_paused")),
            BTreeSet::from(["workarea_id".to_string(), "paused".to_string()])
        );
        assert_eq!(
            props(&by_name("create_workarea")),
            BTreeSet::from(["workspace_id".to_string(), "spec".to_string()])
        );
        assert_eq!(
            props(&by_name("get_workarea_recent_commits")),
            BTreeSet::from(["workarea_id".to_string(), "repo_id".to_string()])
        );
        assert_eq!(
            props(&by_name("notify_user")),
            BTreeSet::from(["text".to_string(), "severity".to_string()])
        );
        assert_eq!(
            props(&by_name("cross_workarea_search")),
            BTreeSet::from(["query".to_string()])
        );
        // Read tools with no required args still carry an object schema.
        assert!(props(&by_name("list_workspaces")).is_empty());
    }

    #[test]
    fn every_descriptor_converts_to_an_rmcp_tool() {
        for t in all_tools() {
            let tool = t.to_rmcp_tool();
            assert_eq!(tool.name, t.name);
            assert!(
                tool.output_schema.is_some(),
                "{} keeps its output schema",
                t.name
            );
            // The input schema round-trips into the rmcp Tool unchanged.
            assert_eq!(
                tool.input_schema.get("type").and_then(|v| v.as_str()),
                Some("object")
            );
        }
    }

    #[test]
    fn dispatch_returns_typed_unimplemented_not_panic_not_success() {
        // Every one of the 18 frozen tools returns a TYPED unimplemented error
        // (never Ok, never a panic) with the stable "wired in Task 40N" message.
        for t in all_tools() {
            let err = dispatch(t.name, None).expect_err(&format!(
                "{} must return a typed unimplemented error, not Ok(())",
                t.name
            ));
            assert_eq!(
                err.code,
                rmcp::model::ErrorCode::INTERNAL_ERROR,
                "{} must be a typed internal_error",
                t.name
            );
            assert!(
                err.message.contains(UNIMPLEMENTED_MSG_PREFIX),
                "{} message must carry the stable prefix, got: {}",
                t.name,
                err.message
            );
            assert!(
                err.message.contains("wired in Task 40"),
                "{} message must name its impl task",
                t.name
            );
        }
    }

    #[test]
    fn dispatch_maps_impl_task_per_class() {
        // Reads → 405, writes → 406, side-channels → 407.
        assert!(dispatch("list_workspaces", None)
            .unwrap_err()
            .message
            .contains("Task 405"));
        assert!(dispatch("create_workspace", None)
            .unwrap_err()
            .message
            .contains("Task 406"));
        assert!(dispatch("notify_user", None)
            .unwrap_err()
            .message
            .contains("Task 407"));
    }

    #[test]
    fn dispatch_rejects_unknown_tool_with_invalid_params() {
        let err = dispatch("not_a_real_tool", None).unwrap_err();
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
    }
}
