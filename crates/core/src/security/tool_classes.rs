//! Tool-name → [`ToolClass`] lookup table (Task 42).
//!
//! V0.1 uses an inline `LazyLock<HashMap<&'static str, ToolClass>>` per
//! `design/04 §3.10`. The TOML-driven `tool-classifications.toml` is
//! V1.0 polish; until then the table below is the canonical source of
//! truth for the four buckets (`Safe | Restricted | Dangerous`) the
//! [`PermissionResolver`](crate::security::permission::PermissionResolver)
//! consumes.
//!
//! ## Buckets
//!
//! - [`ToolClass::Safe`] — read-only operations (`Read`, `Glob`,
//!   `Grep`). All four modes auto-approve.
//! - [`ToolClass::Restricted`] — file mutations (`Write`, `Edit`,
//!   `NotebookEdit`) and the shell (`Bash`) — Task 43's destructive
//!   intercept narrows the shell case further. `normal` asks; `auto`
//!   and `yolo` auto-approve. Network / MCP tools also live here in
//!   V0.1: project-trusted MCPs are a V1.0 concept and we keep all MCP
//!   tools `Restricted` until then.
//! - [`ToolClass::Dangerous`] — destructive operations (`Delete`).
//!   `auto` asks; `yolo` auto-approves only when
//!   `bypass_destructive_guard = true`.
//!
//! Tools not present in the table default to [`ToolClass::Restricted`]
//! — a conservative posture: an unknown agent-emitted tool name asks
//! before running in `normal`, auto-approves in `auto`/`yolo`, and
//! always asks in `strict`. The opposite default (`Safe`) would let a
//! mistyped or freshly-added tool slip past the gate.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::security::permission::ToolClass;

/// Inline classification table consulted by
/// [`PermissionResolver::classify`](crate::security::permission::PermissionResolver::classify).
///
/// Lookups are case-sensitive and exact. Parser packs are responsible
/// for normalising tool names before the resolver sees them (the
/// Claude Code pack emits the canonical names used by the Claude
/// CLI's tool-use protocol).
pub static TOOL_CLASSES: LazyLock<HashMap<&'static str, ToolClass>> = LazyLock::new(|| {
    let mut m = HashMap::new();

    // Read-only / safe tools (Claude Code built-ins).
    m.insert("Read", ToolClass::Safe);
    m.insert("Glob", ToolClass::Safe);
    m.insert("Grep", ToolClass::Safe);

    // File-mutating tools — Restricted.
    m.insert("Write", ToolClass::Restricted);
    m.insert("Edit", ToolClass::Restricted);
    m.insert("NotebookEdit", ToolClass::Restricted);

    // Shell. Task 43's destructive-command intercept will tighten the
    // Bash case further (e.g. `rm -rf` upgrades to Dangerous via
    // pattern match on the command line); the resolver-level
    // classification stays Restricted here so `normal` still asks.
    m.insert("Bash", ToolClass::Restricted);

    // Destructive. Generic `Delete` covers the canonical destructive
    // tool surface in V0.1; per-tool deny lists (`drop`, `rm`, …) live
    // in the parser pack and the Task 43 intercept.
    m.insert("Delete", ToolClass::Dangerous);

    m
});

/// Look up `tool` in [`TOOL_CLASSES`], falling back to
/// [`ToolClass::Restricted`] for unknown names (conservative posture
/// per the module docs).
pub fn classify_tool(tool: &str) -> ToolClass {
    TOOL_CLASSES
        .get(tool)
        .copied()
        .unwrap_or(ToolClass::Restricted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_tools_classify_per_table() {
        assert_eq!(classify_tool("Read"), ToolClass::Safe);
        assert_eq!(classify_tool("Glob"), ToolClass::Safe);
        assert_eq!(classify_tool("Grep"), ToolClass::Safe);
        assert_eq!(classify_tool("Write"), ToolClass::Restricted);
        assert_eq!(classify_tool("Edit"), ToolClass::Restricted);
        assert_eq!(classify_tool("NotebookEdit"), ToolClass::Restricted);
        assert_eq!(classify_tool("Bash"), ToolClass::Restricted);
        assert_eq!(classify_tool("Delete"), ToolClass::Dangerous);
    }

    #[test]
    fn unknown_tool_defaults_restricted() {
        assert_eq!(classify_tool("ExoticNewTool"), ToolClass::Restricted);
        assert_eq!(classify_tool(""), ToolClass::Restricted);
    }

    #[test]
    fn lookup_is_case_sensitive() {
        // The Claude Code pack emits "Read" (capital R); a lowercase
        // "read" is treated as unknown and falls through to Restricted.
        assert_eq!(classify_tool("read"), ToolClass::Restricted);
    }
}
